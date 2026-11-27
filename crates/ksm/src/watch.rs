//! One watcher per store (Go: a client-go reflector feeding a MetricsStore,
//! with `pkg/sharding` and `pkg/watch` wrapped around its ListerWatcher).
//! Mapping per ADR-011 and docs/port/research/kube-watcher.md.

use std::fmt::Debug;
use std::sync::Arc;

use futures::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
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

/// The watcher configuration of the settings (field selector, page size,
/// `resourceVersion=0` lists).
pub fn watcher_config(s: &WatchSettings) -> watcher::Config {
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
    let stream = watcher(api, watcher_config(settings));
    let sharding = settings.sharding;
    tokio::spawn(drive(
        stream,
        Driver {
            settings: settings.clone(),
            go_type,
            resource: go_type.to_owned(),
            families,
            store,
            stop: None,
        },
        move |obj: &K, _initial| keep(&sharding, obj.meta()),
        |obj| obj,
    ));
}

/// What a watcher feeds and how it reports.
pub struct Driver<T> {
    pub settings: WatchSettings,
    /// The type label of the list/watch metrics.
    pub go_type: &'static str,
    /// The resource named in logs.
    pub resource: String,
    pub families: Arc<Vec<FamilyGenerator<T>>>,
    pub store: Arc<MetricsStore>,
    /// Stops this watcher besides the settings' token.
    pub stop: Option<CancellationToken>,
}

/// Feeds a watcher's events into the store (ADR-011): the initial list is
/// buffered and rendered on all cores, then replaces the store; later
/// events add or delete. `keep` gives the UID of an object this watcher
/// stores (`initial`: part of the initial list), `convert` the value the
/// families read.
pub async fn drive<K, T, S>(
    stream: S,
    d: Driver<T>,
    keep: impl Fn(&K, bool) -> Option<String>,
    convert: impl Fn(K) -> T,
) where
    S: futures::Stream<Item = watcher::Result<watcher::Event<K>>>,
    T: Send + Sync + 'static,
{
    futures::pin_mut!(stream);
    let Driver {
        settings: s,
        go_type,
        resource,
        families,
        store,
        stop,
    } = d;
    let stopped = || async {
        tokio::select! {
            () = s.cancel.cancelled() => {}
            () = async { match &stop { Some(t) => t.cancelled().await, None => std::future::pending().await } } => {}
        }
    };
    let mut buffer: Vec<(String, Rendered)> = Vec::new();
    let mut pending: Vec<(String, T)> = Vec::new();
    let limit_mode = s.object_limit > 0;
    if limit_mode {
        s.metrics
            .list_objects_limit
            .set(&[go_type], s.object_limit as f64);
    }
    loop {
        let ev = tokio::select! {
            () = stopped() => break,
            ev = stream.next() => ev,
        };
        let Some(ev) = ev else { break };
        match ev {
            Ok(watcher::Event::Init) => {
                buffer.clear();
                pending.clear();
            }
            Ok(watcher::Event::InitApply(obj)) => {
                if let Some(uid) = keep(&obj, true) {
                    pending.push((uid, convert(obj)));
                    if pending.len() >= RENDER_BATCH {
                        render_batch(&families, &mut pending, &mut buffer).await;
                    }
                }
            }
            Ok(watcher::Event::InitDone) => {
                render_batch(&families, &mut pending, &mut buffer).await;
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
                store.replace(items);
            }
            Ok(watcher::Event::Apply(obj)) => {
                if let Some(uid) = keep(&obj, false) {
                    let v = convert(obj);
                    store.add(&uid, || render(&families, &v));
                }
            }
            Ok(watcher::Event::Delete(obj)) => {
                if let Some(uid) = keep(&obj, false) {
                    store.delete(&uid);
                }
            }
            Err(e) => {
                match &e {
                    watcher::Error::InitialListFailed(_) if limit_mode => {
                        s.metrics.list_total.inc(&["error", go_type]);
                    }
                    _ => s.metrics.watch_total.inc(&["error", go_type]),
                }
                tracing::error!(resource = %resource, error = %e, "Failed to watch");
                // kube's backoff resets on every event; slow down failing relists.
                tokio::select! {
                    () = stopped() => break,
                    () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                }
            }
        }
    }
}

/// Objects rendered together during the initial sync.
const RENDER_BATCH: usize = 512;

/// Renders the buffered objects on all cores (Go renders them one by one in
/// the store's transformer; the result is the same).
async fn render_batch<T: Send + Sync + 'static>(
    families: &Arc<Vec<FamilyGenerator<T>>>,
    pending: &mut Vec<(String, T)>,
    out: &mut Vec<(String, Rendered)>,
) {
    if pending.is_empty() {
        return;
    }
    let batch = std::mem::take(pending);
    let fams = Arc::clone(families);
    let rendered = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;
        batch
            .into_par_iter()
            .map(|(uid, obj)| {
                let r = render(&fams, &obj);
                (uid, r)
            })
            .collect::<Vec<_>>()
    })
    .await;
    match rendered {
        Ok(r) => out.extend(r),
        Err(e) => tracing::error!(error = %e, "rendering metrics failed"),
    }
}

/// The UID of an object this shard keeps.
pub fn keep(sharding: &Sharding, meta: &ObjectMeta) -> Option<String> {
    let uid = meta.uid.clone().unwrap_or_default();
    sharding.keep(&uid).then_some(uid)
}
