//! CRD discovery for custom resource state metrics (Go:
//! `internal/discovery`): a watcher on `apiextensions.k8s.io/v1`
//! CustomResourceDefinitions keeps a cache of the served
//! group/version/kind/plurals, resolves wildcard GVKs against it, and owns
//! one stop token per GVK that ends the watchers of a GVK leaving the cache.
//!
//! Go's maps iterate in random order; versions here are kept sorted, kinds
//! in insertion order.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use kube::api::ApiResource;
use kube::runtime::watcher;
use parking_lot::Mutex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::telemetry::Vector;
use crate::watch_dynamic::{Unstructured, api};

/// A served group/version/kind and its plural (`groupVersionKindPlural`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Gvkp {
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
}

impl Gvkp {
    /// `schema.GroupVersionKind.String()`: the key of the stop tokens.
    pub fn gvk_key(&self) -> String {
        gvk_key(&self.group, &self.version, &self.kind)
    }
}

/// `schema.GroupVersionKind.String()`.
pub fn gvk_key(group: &str, version: &str, kind: &str) -> String {
    format!("{group}/{version}, Kind={kind}")
}

/// A GVK's stop token; `id` tells a replaced token from the one it replaced
/// (Go compares the channels).
#[derive(Clone, Debug)]
pub struct StopHandle {
    pub id: u64,
    pub token: CancellationToken,
}

#[derive(Default)]
struct Cache {
    /// group → version → [(kind, plural)].
    map: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>>,
    stop: HashMap<String, StopHandle>,
    warned_missing: HashSet<String>,
    was_updated: bool,
    revision: u64,
}

/// Self-metrics of the CRD watcher.
#[derive(Clone)]
pub struct CrdCounters {
    pub add: Vector,
    pub update: Vector,
    pub delete: Vector,
    pub cache: Vector,
}

/// `CRDiscoverer`.
pub struct CrDiscoverer {
    cache: Mutex<Cache>,
    next_id: AtomicU64,
    counters: CrdCounters,
}

/// `extractGVKPs`: the served GVKPs of a CRD; `None` when the object cannot
/// be read (or a version failed to parse and none was read).
pub fn extract_gvkps(obj: &Value) -> Option<Vec<Gvkp>> {
    let name = obj["metadata"]["name"].as_str().unwrap_or("");
    let fail = |what: &str| {
        tracing::error!(crd = name, "{what}");
        None
    };
    let Some(spec) = obj.get("spec").filter(|s| s.is_object()) else {
        return fail("CRD has no spec object");
    };
    let Some(g) = spec.get("group").and_then(Value::as_str) else {
        return fail("CRD spec has no group");
    };
    let Some(names) = spec.get("names").filter(|s| s.is_object()) else {
        return fail("CRD spec has no names object");
    };
    let Some(k) = names.get("kind").and_then(Value::as_str) else {
        return fail("CRD spec has no kind");
    };
    let Some(p) = names.get("plural").and_then(Value::as_str) else {
        return fail("CRD spec has no plural");
    };
    let Some(versions) = spec.get("versions").and_then(Value::as_array) else {
        return fail("CRD spec has no versions list");
    };
    let mut out = Vec::with_capacity(versions.len());
    let mut parse_failed = false;
    for version in versions {
        if !version.is_object() {
            tracing::error!(crd = name, "CRD version is not an object");
            parse_failed = true;
            continue;
        }
        let Some(v) = version.get("name").and_then(Value::as_str) else {
            tracing::error!(crd = name, "CRD version has no name");
            parse_failed = true;
            continue;
        };
        if let Some(served) = version.get("served") {
            match served.as_bool() {
                None => {
                    tracing::error!(
                        crd = name,
                        version = v,
                        "CRD version has a non-boolean served field"
                    );
                    parse_failed = true;
                    continue;
                }
                Some(false) => {
                    tracing::debug!(
                        group = g,
                        version = v,
                        kind = k,
                        "skipping CRD version that is not served by the API server"
                    );
                    continue;
                }
                Some(true) => {}
            }
        }
        out.push(Gvkp {
            group: g.to_owned(),
            version: v.to_owned(),
            kind: k.to_owned(),
            plural: p.to_owned(),
        });
    }
    if out.is_empty() && parse_failed {
        return None;
    }
    Some(out)
}

impl CrDiscoverer {
    pub fn new(counters: CrdCounters) -> Self {
        CrDiscoverer {
            cache: Mutex::new(Cache::default()),
            next_id: AtomicU64::new(1),
            counters,
        }
    }

    fn new_handle(&self) -> StopHandle {
        StopHandle {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            token: CancellationToken::new(),
        }
    }

    /// `AppendToMap`: true when a kind was added or its plural changed.
    fn append(&self, c: &mut Cache, gvkps: &[Gvkp]) -> bool {
        let mut changed = false;
        for g in gvkps {
            let kinds = c
                .map
                .entry(g.group.clone())
                .or_default()
                .entry(g.version.clone())
                .or_default();
            let key = g.gvk_key();
            match kinds.iter_mut().find(|(k, _)| *k == g.kind) {
                Some((_, plural)) => {
                    if *plural != g.plural {
                        plural.clone_from(&g.plural);
                        changed = true;
                        if let Some(old) = c.stop.remove(&key) {
                            old.token.cancel();
                        }
                        c.stop.insert(key.clone(), self.new_handle());
                    }
                }
                None => {
                    kinds.push((g.kind.clone(), g.plural.clone()));
                    changed = true;
                }
            }
            c.stop.entry(key).or_insert_with(|| self.new_handle());
        }
        if changed {
            c.revision += 1;
        }
        changed
    }

    /// `RemoveFromMap`: true when a kind was removed.
    fn remove(c: &mut Cache, gvkps: &[Gvkp]) -> bool {
        let mut changed = false;
        for g in gvkps {
            let Some(versions) = c.map.get_mut(&g.group) else {
                continue;
            };
            let Some(kinds) = versions.get_mut(&g.version) else {
                continue;
            };
            let Some(i) = kinds.iter().position(|(k, _)| *k == g.kind) else {
                continue;
            };
            if let Some(old) = c.stop.remove(&g.gvk_key()) {
                old.token.cancel();
            }
            changed = true;
            kinds.remove(i);
            if kinds.is_empty() {
                versions.remove(&g.version);
                if versions.is_empty() {
                    c.map.remove(&g.group);
                }
            }
        }
        if changed {
            c.revision += 1;
        }
        changed
    }

    fn on_add(&self, gvkps: Option<&[Gvkp]>) {
        let gvkps = gvkps.unwrap_or_default();
        {
            let mut c = self.cache.lock();
            if self.append(&mut c, gvkps) {
                c.was_updated = true;
            }
        }
        self.counters.add.inc(&[]);
        self.counters.cache.add(&[], 1.0);
    }

    /// `applyCRDUpdate`: only the GVKPs that changed.
    fn on_update(&self, old: Option<&[Gvkp]>, new: Option<&[Gvkp]>) {
        let old = old.unwrap_or_default();
        {
            let mut c = self.cache.lock();
            if let Some(new) = new {
                let oset: HashSet<&Gvkp> = old.iter().collect();
                let nset: HashSet<&Gvkp> = new.iter().collect();
                if oset != nset || old.len() != new.len() {
                    let removed: Vec<Gvkp> =
                        old.iter().filter(|g| !nset.contains(g)).cloned().collect();
                    let added: Vec<Gvkp> =
                        new.iter().filter(|g| !oset.contains(g)).cloned().collect();
                    let mut changed = Self::remove(&mut c, &removed);
                    changed |= self.append(&mut c, &added);
                    if changed {
                        c.was_updated = true;
                    }
                }
            }
        }
        self.counters.update.inc(&[]);
    }

    fn on_delete(&self, gvkps: Option<&[Gvkp]>) {
        let gvkps = gvkps.unwrap_or_default();
        {
            let mut c = self.cache.lock();
            if Self::remove(&mut c, gvkps) {
                c.was_updated = true;
            }
        }
        self.counters.delete.inc(&[]);
        self.counters.cache.add(&[], -1.0);
    }

    /// `ResolveGVKToGVKPs`: the cached GVKPs a configured GVK (version and
    /// kind may be empty or `*`) stands for.
    pub fn resolve(&self, group: &str, version: &str, kind: &str) -> Result<Vec<Gvkp>, String> {
        if group.is_empty() || group == "*" {
            // `%v` of a schema.GroupVersionKind: its String method.
            return Err(format!(
                "group is required in the defined GVK {group}/{version}, Kind={kind}"
            ));
        }
        let has_version = !version.is_empty() && version != "*";
        let has_kind = !kind.is_empty() && kind != "*";
        let mk = |v: &str, k: &str, p: &str| Gvkp {
            group: group.to_owned(),
            version: v.to_owned(),
            kind: k.to_owned(),
            plural: p.to_owned(),
        };
        let mut out = Vec::new();
        let mut warn_missing = false;
        {
            let mut c = self.cache.lock();
            let versions = c.map.get(group);
            if has_version && has_kind {
                let found = versions
                    .and_then(|vs| vs.get(version))
                    .and_then(|ks| ks.iter().find(|(k, _)| k == kind))
                    .map(|(_, p)| p.clone());
                let key = gvk_key(group, version, kind);
                match found {
                    Some(p) => {
                        c.warned_missing.remove(&key);
                        out.push(mk(version, kind, &p));
                    }
                    None => warn_missing = c.warned_missing.insert(key),
                }
            } else {
                for (v, kinds) in versions.into_iter().flatten() {
                    if has_version && v != version {
                        continue;
                    }
                    for (k, p) in kinds {
                        if has_kind && k != kind {
                            continue;
                        }
                        out.push(mk(v, k, p));
                    }
                }
            }
        }
        if warn_missing {
            tracing::info!(
                gvk = %format!("{group}/{version}, Kind={kind}"),
                "Configured custom resource was not found in the cluster, no metrics will be generated for it until a CRD serving this version is installed"
            );
        }
        Ok(out)
    }

    /// `GetStopChanForGVK`.
    pub fn stop_for(&self, gvk: &str) -> Option<StopHandle> {
        self.cache.lock().stop.get(gvk).cloned()
    }

    /// Whether the cache changed since the last applied rebuild, its
    /// revision and (when changed) its stop tokens.
    pub fn observe(&self) -> Option<(u64, HashMap<String, StopHandle>)> {
        let c = self.cache.lock();
        c.was_updated.then(|| (c.revision, c.stop.clone()))
    }

    /// Clears the update flag if the cache is still at `revision`.
    pub fn applied(&self, revision: u64) {
        let mut c = self.cache.lock();
        if c.revision == revision {
            c.was_updated = false;
        }
    }

    /// `StartDiscovery`: watches CRDs until `cancel` fires.
    pub fn start(self: &Arc<Self>, client: kube::Client, cancel: CancellationToken) {
        let ar = ApiResource {
            group: "apiextensions.k8s.io".into(),
            version: "v1".into(),
            api_version: "apiextensions.k8s.io/v1".into(),
            kind: "CustomResourceDefinition".into(),
            plural: "customresourcedefinitions".into(),
        };
        let api = api(client, &ar, "");
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let stream = watcher(api, watcher::Config::default());
            futures::pin_mut!(stream);
            // The informer's store, reduced to what the handlers read: name →
            // the GVKPs of the last object seen (`None`: it could not be read).
            // Keeping whole CRDs would hold every OpenAPI schema in memory.
            let mut known: HashMap<String, Option<Vec<Gvkp>>> = HashMap::new();
            let mut relisted: HashSet<String> = HashSet::new();
            let mut backoff = crate::backoff::Backoff::default();
            loop {
                let ev = tokio::select! {
                    () = cancel.cancelled() => {
                        tracing::info!("context cancelled, stopping discovery");
                        break;
                    }
                    ev = stream.next() => ev,
                };
                let Some(ev) = ev else { break };
                match ev {
                    Ok(watcher::Event::Init) => relisted.clear(),
                    Ok(watcher::Event::InitApply(o)) => {
                        relisted.insert(o.name().to_owned());
                        this.upsert(&mut known, o);
                    }
                    Ok(watcher::Event::InitDone) => {
                        let gone: Vec<String> = known
                            .keys()
                            .filter(|k| !relisted.contains(*k))
                            .cloned()
                            .collect();
                        for k in gone {
                            if let Some(old) = known.remove(&k) {
                                this.on_delete(old.as_deref());
                            }
                        }
                        relisted.clear();
                    }
                    Ok(watcher::Event::Apply(o)) => this.upsert(&mut known, o),
                    Ok(watcher::Event::Delete(o)) => {
                        let old = known
                            .remove(o.name())
                            .unwrap_or_else(|| extract_gvkps(&o.raw));
                        this.on_delete(old.as_deref());
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to watch CustomResourceDefinitions");
                        if !backoff.wait(cancel.cancelled()).await {
                            break;
                        }
                    }
                }
            }
        });
    }

    fn upsert(&self, known: &mut HashMap<String, Option<Vec<Gvkp>>>, o: Unstructured) {
        let new = extract_gvkps(&o.raw);
        match known.get_mut(o.name()) {
            Some(old) => {
                self.on_update(old.as_deref(), new.as_deref());
                *old = new;
            }
            None => {
                self.on_add(new.as_deref());
                known.insert(o.name().to_owned(), new);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::Registry;
    use serde_json::json;

    fn disc() -> CrDiscoverer {
        let r = Registry::new();
        CrDiscoverer::new(CrdCounters {
            add: r.counter("a", "a"),
            update: r.counter("u", "u"),
            delete: r.counter("d", "d"),
            cache: r.gauge("c", "c"),
        })
    }

    fn crd(group: &str, kind: &str, plural: &str, versions: &[(&str, bool)]) -> Value {
        let vs: Vec<Value> = versions
            .iter()
            .map(|(v, s)| json!({"name": v, "served": s}))
            .collect();
        json!({"metadata": {"name": format!("{plural}.{group}")},
            "spec": {"group": group, "names": {"kind": kind, "plural": plural}, "versions": vs}})
    }

    #[test]
    fn extract_skips_unserved() {
        let g = extract_gvkps(&crd("g", "Foo", "foos", &[("v1", true), ("v2", false)])).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].version, "v1");
        assert!(extract_gvkps(&json!({"spec": 1})).is_none());
        let none = extract_gvkps(&crd("g", "Foo", "foos", &[("v1", false)])).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn resolve_wildcards_and_updates() {
        let d = disc();
        d.on_add(extract_gvkps(&crd("g", "Foo", "foos", &[("v1", true), ("v2", true)])).as_deref());
        d.on_add(extract_gvkps(&crd("g", "Bar", "bars", &[("v1", true)])).as_deref());
        assert_eq!(d.resolve("g", "*", "*").unwrap().len(), 3);
        assert_eq!(d.resolve("g", "v1", "").unwrap().len(), 2);
        assert_eq!(d.resolve("g", "*", "Foo").unwrap().len(), 2);
        assert_eq!(d.resolve("g", "v2", "Foo").unwrap()[0].plural, "foos");
        assert!(d.resolve("g", "v3", "Foo").unwrap().is_empty());
        assert!(d.resolve("", "v1", "Foo").is_err());
        let (rev, stops) = d.observe().unwrap();
        d.applied(rev);
        assert!(d.observe().is_none());
        // A status-only update changes nothing.
        let foo = extract_gvkps(&crd("g", "Foo", "foos", &[("v1", true), ("v2", true)]));
        d.on_update(foo.as_deref(), foo.as_deref());
        assert!(d.observe().is_none());
        // Unserving v2 stops its watchers.
        let tok = stops["g/v2, Kind=Foo"].token.clone();
        d.on_update(
            foo.as_deref(),
            extract_gvkps(&crd("g", "Foo", "foos", &[("v1", true), ("v2", false)])).as_deref(),
        );
        assert!(tok.is_cancelled());
        assert!(d.observe().is_some());
        assert_eq!(d.resolve("g", "*", "Foo").unwrap().len(), 1);
        // A plural change replaces the token.
        let before = d.stop_for("g/v1, Kind=Bar").unwrap();
        d.on_update(
            extract_gvkps(&crd("g", "Bar", "bars", &[("v1", true)])).as_deref(),
            extract_gvkps(&crd("g", "Bar", "barz", &[("v1", true)])).as_deref(),
        );
        let after = d.stop_for("g/v1, Kind=Bar").unwrap();
        assert!(before.token.is_cancelled());
        assert_ne!(before.id, after.id);
        d.on_delete(extract_gvkps(&crd("g", "Bar", "barz", &[("v1", true)])).as_deref());
        assert!(after.token.is_cancelled());
        assert!(d.resolve("g", "v1", "Bar").unwrap().is_empty());
    }
}
