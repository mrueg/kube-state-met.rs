//! One watcher per store (Go: a client-go reflector feeding a MetricsStore,
//! with `pkg/sharding` and `pkg/watch` wrapped around its ListerWatcher).
//! Mapping per ADR-011 and docs/port/research/kube-watcher.md.

use std::fmt::Debug;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
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

use crate::backoff::Backoff;
use crate::telemetry::ListWatchMetrics;

/// Picks the API endpoint for a resource scope: namespaced resources in one
/// namespace use the namespaced path, everything else the cluster-wide one.
pub trait ScopeApi {
    /// Cluster-scoped: one store for all namespaces.
    const CLUSTER: bool;

    fn api<K>(client: kube::Client, ns: &str) -> Api<K>
    where
        K: Resource<Scope = Self, DynamicType = ()>;
}

impl ScopeApi for NamespaceResourceScope {
    const CLUSTER: bool = false;

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
    const CLUSTER: bool = true;

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

/// The events of a store's objects: kube-runtime's watcher, or with
/// `--object-limit` a one-page list and a watch (`crate::limited`).
/// Cluster-scoped built-in resources get no field selector: Go's
/// `create*ListWatch` for them drop it (custom resources always use it).
pub fn events<K>(
    api: Api<K>,
    s: &WatchSettings,
    go_type: &'static str,
    with_fields: bool,
) -> BoxStream<'static, watcher::Result<watcher::Event<K>>>
where
    K: Resource + Clone + DeserializeOwned + Debug + Send + 'static,
{
    let fields = (with_fields && !s.field_selector.is_empty()).then(|| s.field_selector.clone());
    if s.object_limit > 0 {
        let cfg = crate::limited::Limited {
            limit: u32::try_from(s.object_limit).unwrap_or(u32::MAX),
            fields,
            use_api_server_cache: s.use_api_server_cache,
            metrics: s.metrics.clone(),
            go_type,
        };
        return crate::limited::stream(api, cfg).boxed();
    }
    let mut cfg = watcher::Config::default();
    if let Some(f) = &fields {
        cfg = cfg.fields(f);
    }
    if s.use_api_server_cache {
        cfg = cfg.any_semantic();
        cfg.page_size = None;
    }
    watcher(api, cfg).boxed()
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
    let stream = events(api, settings, go_type, !<K::Scope as ScopeApi>::CLUSTER);
    let sharding = settings.sharding;
    tokio::spawn(drive(
        stream,
        Driver {
            settings: settings.clone(),
            go_type,
            resource: go_type.to_owned(),
            families,
            store,
            stop: CancellationToken::new(),
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
    /// Stops this watcher besides the settings' token (one that never fires
    /// when nothing else stops it).
    pub stop: CancellationToken,
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
            () = stop.cancelled() => {}
        }
    };
    let mut buffer: Vec<(String, Rendered)> = Vec::new();
    let mut pending: Vec<(String, T)> = Vec::new();
    let mut rendering: std::collections::VecDeque<Batch> = std::collections::VecDeque::new();
    // With an object limit the lister counts its own lists and watches.
    let limit_mode = s.object_limit > 0;
    let mut backoff = Backoff::default();
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
                rendering.clear();
            }
            Ok(watcher::Event::InitApply(obj)) => {
                if let Some(uid) = keep(&obj, true) {
                    pending.push((uid, convert(obj)));
                    if pending.len() >= RENDER_BATCH {
                        // Decode the next batch while this one renders; keep
                        // a few batches in flight at most.
                        if rendering.len() >= MAX_BATCHES_IN_FLIGHT
                            && let Some(b) = rendering.pop_front()
                        {
                            collect_batch(b, &mut buffer).await;
                        }
                        rendering.push_back(render_batch(&families, std::mem::take(&mut pending)));
                    }
                }
            }
            Ok(watcher::Event::InitDone) => {
                if !pending.is_empty() {
                    rendering.push_back(render_batch(&families, std::mem::take(&mut pending)));
                }
                while let Some(b) = rendering.pop_front() {
                    collect_batch(b, &mut buffer).await;
                }
                if !limit_mode {
                    // client-go streams the initial list over a watch.
                    s.metrics.watch_total.inc(&["success", go_type]);
                }
                store.replace(std::mem::take(&mut buffer));
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
                if !limit_mode {
                    s.metrics.watch_total.inc(&["error", go_type]);
                }
                tracing::error!(resource = %resource, error = %e, "Failed to watch");
                if !backoff.wait(stopped()).await {
                    break;
                }
            }
        }
    }
}

/// Objects rendered together during the initial sync.
const RENDER_BATCH: usize = 512;
/// Batches rendering while the next one is decoded.
const MAX_BATCHES_IN_FLIGHT: usize = 4;

type Batch = tokio::task::JoinHandle<Vec<(String, Rendered)>>;

/// Renders a batch of objects on all cores (Go renders them one by one in
/// the store's transformer; the result is the same).
fn render_batch<T: Send + Sync + 'static>(
    families: &Arc<Vec<FamilyGenerator<T>>>,
    batch: Vec<(String, T)>,
) -> Batch {
    let fams = Arc::clone(families);
    tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;
        batch
            .into_par_iter()
            .map(|(uid, obj)| {
                let r = render(&fams, &obj);
                (uid, r)
            })
            .collect()
    })
}

/// Waits for a rendered batch and appends it, in list order.
async fn collect_batch(batch: Batch, out: &mut Vec<(String, Rendered)>) {
    match batch.await {
        Ok(r) => out.extend(r),
        Err(e) => tracing::error!(error = %e, "rendering metrics failed"),
    }
}

/// The UID of an object this shard keeps.
pub fn keep(sharding: &Sharding, meta: &ObjectMeta) -> Option<String> {
    let uid = meta.uid.clone().unwrap_or_default();
    sharding.keep(&uid).then_some(uid)
}
