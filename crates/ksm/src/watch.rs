//! One watcher per store (Go: a client-go reflector feeding a MetricsStore,
//! with `pkg/sharding` and `pkg/watch` wrapped around its ListerWatcher).
//! Mapping per ADR-011 and docs/port/research/kube-watcher.md.

use std::fmt::Debug;
use std::sync::Arc;

use futures::StreamExt;
use k8s_openapi::{ClusterResourceScope, NamespaceResourceScope};
use ksm_core::generator::{FamilyGenerator, render};
use ksm_core::sharding::Sharding;
use ksm_core::store::{MetricsStore, Rendered};
use kube::Resource;
use kube::api::Api;
use kube::runtime::watcher;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use crate::telemetry::ListWatchMetrics;

/// Picks the API endpoint for a resource scope: namespaced resources in one
/// namespace use the namespaced path, everything else the cluster-wide one.
pub trait ScopeApi {
    fn api<K>(client: kube::Client, ns: &str) -> Api<K>
    where
        K: Resource<Scope = Self, DynamicType = ()>;
}

impl ScopeApi for NamespaceResourceScope {
    fn api<K>(client: kube::Client, ns: &str) -> Api<K>
    where
        K: Resource<Scope = Self, DynamicType = ()>,
    {
        if ns.is_empty() {
            Api::all(client)
        } else {
            Api::namespaced(client, ns)
        }
    }
}

impl ScopeApi for ClusterResourceScope {
    fn api<K>(client: kube::Client, _ns: &str) -> Api<K>
    where
        K: Resource<Scope = Self, DynamicType = ()>,
    {
        Api::all(client)
    }
}

/// Settings every watcher shares.
#[derive(Clone)]
pub struct WatchSettings {
    pub client: kube::Client,
    pub field_selector: String,
    pub sharding: Sharding,
    pub use_api_server_cache: bool,
    pub object_limit: i64,
    pub metrics: ListWatchMetrics,
    pub cancel: CancellationToken,
}

fn watcher_config(s: &WatchSettings) -> watcher::Config {
    let mut cfg = watcher::Config::default();
    if !s.field_selector.is_empty() {
        cfg = cfg.fields(&s.field_selector);
    }
    if s.object_limit > 0 {
        cfg = cfg.page_size(u32::try_from(s.object_limit).unwrap_or(u32::MAX));
    }
    if s.use_api_server_cache {
        cfg = cfg.any_semantic();
        if s.object_limit <= 0 {
            cfg.page_size = None;
        }
    }
    cfg
}

/// Starts a watcher feeding `store` with the rendered families of every
/// object of this shard, until the settings' token is cancelled.
pub fn spawn<K>(
    settings: &WatchSettings,
    namespace: &str,
    go_type: &'static str,
    families: Arc<Vec<FamilyGenerator<K>>>,
    store: Arc<MetricsStore>,
) where
    K: Resource<DynamicType = ()> + Clone + DeserializeOwned + Debug + Send + Sync + 'static,
    K::Scope: ScopeApi,
{
    let api: Api<K> = <K::Scope as ScopeApi>::api(settings.client.clone(), namespace);
    let cfg = watcher_config(settings);
    let s = settings.clone();
    tokio::spawn(async move {
        let stream = watcher(api, cfg);
        futures::pin_mut!(stream);
        let mut buffer: Vec<(String, String, Rendered)> = Vec::new();
        let limit_mode = s.object_limit > 0;
        if limit_mode {
            s.metrics
                .list_objects_limit
                .set(&[go_type], s.object_limit as f64);
        }
        loop {
            let ev = tokio::select! {
                () = s.cancel.cancelled() => break,
                ev = stream.next() => ev,
            };
            let Some(ev) = ev else { break };
            match ev {
                Ok(watcher::Event::Init) => buffer.clear(),
                Ok(watcher::Event::InitApply(obj)) => {
                    if let Some((uid, rv)) = keep(&s.sharding, &obj) {
                        buffer.push((uid, rv, render(&families, &obj)));
                    }
                }
                Ok(watcher::Event::InitDone) => {
                    let items = std::mem::take(&mut buffer);
                    if limit_mode {
                        // Go lists (and counts the list) only with an object limit;
                        // otherwise client-go streams the initial list over a watch.
                        s.metrics.list_total.inc(&["success", go_type]);
                        let n = items
                            .len()
                            .min(usize::try_from(s.object_limit).unwrap_or(usize::MAX));
                        s.metrics.list_objects.set(&[go_type], n as f64);
                    }
                    s.metrics.watch_total.inc(&["success", go_type]);
                    store.replace_rendered(items, "");
                }
                Ok(watcher::Event::Apply(obj)) => {
                    if let Some((uid, rv)) = keep(&s.sharding, &obj) {
                        store.add(&uid, &rv, || render(&families, &obj));
                    }
                }
                Ok(watcher::Event::Delete(obj)) => {
                    if let Some((uid, rv)) = keep(&s.sharding, &obj) {
                        store.delete(&uid, &rv);
                    }
                }
                Err(e) => {
                    match &e {
                        watcher::Error::InitialListFailed(_) if limit_mode => {
                            s.metrics.list_total.inc(&["error", go_type]);
                        }
                        _ => s.metrics.watch_total.inc(&["error", go_type]),
                    }
                    tracing::error!(resource = go_type, error = %e, "Failed to watch");
                    // kube's backoff resets on every event; slow down failing relists.
                    tokio::select! {
                        () = s.cancel.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                    }
                }
            }
        }
    });
}

/// UID and resource version of an object this shard keeps.
fn keep<K: Resource>(sharding: &Sharding, obj: &K) -> Option<(String, String)> {
    let meta = obj.meta();
    let uid = meta.uid.clone().unwrap_or_default();
    if !sharding.keep(&uid) {
        return None;
    }
    Some((uid, meta.resource_version.clone().unwrap_or_default()))
}
