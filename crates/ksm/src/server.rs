//! One run of kube-state-metrics: config, stores, autosharding and the two
//! HTTP servers (Go: `pkg/app/server.go`, `pkg/metricshandler.Run`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Pod;
use ksm_core::filter::{AllowDenyList, OptInFilter};
use ksm_core::generator::{CompositeFilter, FamilyFilter};
use ksm_core::sharding::Sharding;
use kube::api::Api;
use kube::runtime::watcher;
use md5::{Digest, Md5};
use tokio_util::sync::CancellationToken;

use std::collections::BTreeMap;

use crate::builder::{Builder, CustomResourceStore, GvkStop};
use crate::client::{VERSION, create_client, go_arch};
use crate::crs::StoreConfigurer;
use crate::discovery::{CrDiscoverer, CrdCounters};
use crate::handler::{MetricsHandler, plain_error};
use crate::options::{Options, merge_field_selectors};
use crate::telemetry::{
    DEF_BUCKETS, HistogramVec, ListWatchMetrics, Registry, ShardingMetrics, Vector,
};
use crate::watch::WatchSettings;

/// Bound on how long in-flight requests may take once the servers shut down.
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// `version.Info()`.
pub fn version_info() -> String {
    format!(
        "(version={VERSION}, branch={}, revision={})",
        option_env!("KSM_BRANCH").unwrap_or(""),
        revision()
    )
}

pub fn revision() -> &'static str {
    option_env!("KSM_REVISION").unwrap_or("unknown")
}

/// `md5HashAsMetricValue`: the first 6 bytes of the MD5, little endian.
pub fn md5_hash_as_metric_value(data: &[u8]) -> f64 {
    let sum = Md5::digest(data);
    let mut buf = [0u8; 8];
    buf[..6].copy_from_slice(&sum[..6]);
    u64::from_le_bytes(buf) as f64
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

struct ConfigMetrics {
    hash: Vector,
    success: Vector,
    success_time: Vector,
}

/// Self-metrics registered at the start of every run.
pub struct SelfMetrics {
    pub registry: Registry,
    pub durations: HistogramVec,
    config: ConfigMetrics,
    pub list_watch: ListWatchMetrics,
    pub sharding: ShardingMetrics,
    pub crd_add: Vector,
    pub crd_update: Vector,
    pub crd_delete: Vector,
    pub crd_cache: Vector,
}

impl SelfMetrics {
    fn new() -> Self {
        let r = Registry::new();
        r.const_gauge(
            "kube_state_metrics_build_info",
            "A metric with a constant '1' value labeled by version, revision, branch, goversion from which kube_state_metrics was built, and the goos and goarch for the build.",
            &[
                ("branch", option_env!("KSM_BRANCH").unwrap_or("")),
                ("client_go_version", option_env!("KSM_KUBE_RS_VERSION").unwrap_or("kube-rs")),
                ("goarch", go_arch()),
                ("goos", std::env::consts::OS),
                ("goversion", option_env!("KSM_RUSTC_VERSION").unwrap_or("rust")),
                ("revision", revision()),
                ("tags", "unknown"),
                ("version", VERSION),
            ],
            1.0,
        );
        let durations = r.histogram_vec(
            "http_request_duration_seconds",
            "A histogram of requests for kube-state-metrics metrics handler.",
            &["method"],
            &[("handler", "metrics")],
            DEF_BUCKETS,
        );
        let config = ConfigMetrics {
            hash: r.gauge_vec(
                "kube_state_metrics_config_hash",
                "Hash of the currently loaded configuration.",
                &["type", "filename"],
            ),
            success: r.gauge_vec(
                "kube_state_metrics_last_config_reload_successful",
                "Whether the last configuration reload attempt was successful.",
                &["type", "filename"],
            ),
            success_time: r.gauge_vec(
                "kube_state_metrics_last_config_reload_success_timestamp_seconds",
                "Timestamp of the last successful configuration reload.",
                &["type", "filename"],
            ),
        };
        let crd_add = r.counter(
            "kube_state_metrics_custom_resource_state_add_events_total",
            "Number of times that the CRD informer triggered the add event.",
        );
        let crd_update = r.counter(
            "kube_state_metrics_custom_resource_state_update_events_total",
            "Number of times that the CRD informer triggered the update event.",
        );
        let crd_delete = r.counter(
            "kube_state_metrics_custom_resource_state_delete_events_total",
            "Number of times that the CRD informer triggered the remove event.",
        );
        let crd_cache = r.gauge(
            "kube_state_metrics_custom_resource_state_cache",
            "Net amount of CRDs affecting the cache currently.",
        );
        let list_watch = ListWatchMetrics::new(&r);
        let sharding = ShardingMetrics::new(&r);
        SelfMetrics {
            registry: r,
            durations,
            config,
            list_watch,
            sharding,
            crd_add,
            crd_update,
            crd_delete,
            crd_cache,
        }
    }
}

/// The text of an `os.PathError` cause as Go prints it.
pub fn go_io_error(e: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => "no such file or directory".to_owned(),
        ErrorKind::PermissionDenied => "permission denied".to_owned(),
        ErrorKind::IsADirectory => "is a directory".to_owned(),
        ErrorKind::NotADirectory => "not a directory".to_owned(),
        _ => {
            let s = e.to_string();
            let s = s.split(" (os error").next().unwrap_or(&s).to_owned();
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_lowercase().chain(c).collect(),
                None => s,
            }
        }
    }
}

/// Go's `filepath.Clean` for the label value (enough for config paths).
pub fn clean_path(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." if parts.last().is_some_and(|l| *l != "..") => {
                parts.pop();
            }
            ".." if abs => {}
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (abs, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

/// Runs kube-state-metrics until `cancel` fires (Go: `RunKubeStateMetrics`).
/// `reload` tells whether the cancellation is a config reload (no shutdown delay).
pub async fn run(
    mut opts: Options,
    cancel: CancellationToken,
    reload: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    let m = SelfMetrics::new();

    if !opts.config.is_empty() {
        let file = clean_path(&opts.config);
        match std::fs::read(&file) {
            Err(e) => {
                tracing::error!(error = %e, file = %opts.config, "encountered error while processing the file")
            }
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if let Err(e) = opts.merge_config_yaml(&text) {
                    tracing::error!(error = %e, "failed to unmarshal opts config file");
                    tracing::info!(
                        "misconfigured config detected, KSM will automatically reload on next write to the config"
                    );
                    tracing::info!("waiting for config to be fixed");
                    m.config.success.set(&["config", &file], 0.0);
                    cancel.cancelled().await;
                    return Ok(());
                }
                m.config.success.set(&["config", &file], 1.0);
                m.config.success_time.set(&["config", &file], unix_now());
                m.config
                    .hash
                    .set(&["config", &file], md5_hash_as_metric_value(&bytes));
                opts.configure_resources_and_metrics(&text);
                tracing::info!(file = %opts.config, "Using config file");
            }
        }
    }

    // Validates the kubeconfig before anything else, as Go does.
    crate::client::build_config(&opts.apiserver, &opts.kubeconfig)
        .await
        .map_err(|e| format!("failed to build config from flags: {e}"))?;

    // Loading custom resource state configuration from cli argument or config file.
    let crs_config = crate::crs::resolve_config(&opts)?;

    if !opts.custom_resource_config_file.is_empty() {
        let file = clean_path(&opts.custom_resource_config_file);
        match std::fs::read(&file) {
            Err(e) => {
                tracing::error!(error = %e, file = %opts.custom_resource_config_file, "encountered error while processing the file")
            }
            Ok(bytes) => {
                let t = "customresourceconfig";
                m.config.success.set(&[t, &file], 1.0);
                m.config.success_time.set(&[t, &file], unix_now());
                m.config
                    .hash
                    .set(&[t, &file], md5_hash_as_metric_value(&bytes));
            }
        }
    }

    let discoverer = crs_config.as_ref().map(|_| {
        Arc::new(CrDiscoverer::new(CrdCounters {
            add: m.crd_add.clone(),
            update: m.crd_update.clone(),
            delete: m.crd_delete.clone(),
            cache: m.crd_cache.clone(),
        }))
    });
    let gvk_stop: Option<GvkStop> = discoverer.as_ref().map(|d| {
        let d = Arc::clone(d);
        let f: GvkStop = Arc::new(move |gvk: &str| d.stop_for(gvk).map(|h| h.token));
        f
    });

    let mut builder = Builder {
        enabled: Vec::new(),
        namespaces: Vec::new(),
        filter: Arc::new(CompositeFilter::default()),
        allow_annotations: Default::default(),
        allow_labels: Default::default(),
        custom: Default::default(),
        gvk_stop,
    };
    let resources: Vec<String> = if opts.resources.is_empty() && !opts.custom_resources_only {
        tracing::info!("Used default resources");
        crate::options::DEFAULT_RESOURCES
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    } else if opts.custom_resources_only {
        tracing::info!("Used CRD resources only");
        Vec::new()
    } else {
        let r: Vec<String> = opts.resources.iter().cloned().collect();
        tracing::info!(resources = ?r, "Used resources");
        r
    };
    builder
        .set_enabled(&resources)
        .map_err(|e| format!("failed to set up resources: {e}"))?;

    let namespaces = opts.get_namespaces();
    let ns_selector = opts.exclude_ns_field_selector();
    let node_selector = if opts.track_unscheduled_pods {
        tracing::info!("Using spec.nodeName= to select unscheduable pods without node");
        "spec.nodeName=".to_owned()
    } else {
        opts.node_field_selector()
    };
    let merged = merge_field_selectors(&[ns_selector.as_str(), node_selector.as_str()])?;
    builder.namespaces = namespaces;

    let mut allow_deny = AllowDenyList::new(&opts.metric_allowlist, &opts.metric_denylist)
        .map_err(|e| e.to_string())?;
    allow_deny
        .parse()
        .map_err(|e| format!("error initializing the allowdeny list: {e}"))?;
    tracing::info!(allowDenyStatus = %allow_deny.status(), "Metric allow-denylisting");
    let opt_in = OptInFilter::new(&opts.metric_opt_in_list)
        .map_err(|e| format!("error initializing the opt-in metric list: {e}"))?;
    if opt_in.count() > 0 {
        tracing::info!(optInMetricsFamilyStatus = %opt_in.status(), "Metrics which were opted into");
    }
    let filters: Vec<Box<dyn FamilyFilter>> = vec![Box::new(allow_deny), Box::new(opt_in)];
    builder.filter = Arc::new(CompositeFilter { filters });

    let client = create_client(&opts.apiserver, &opts.kubeconfig).await?;

    builder.allow_annotations = builder
        .expand_allow_list(&opts.annotations_allow_list)
        .map_err(|e| format!("failed to set up annotations allowlist: {e}"))?;
    tracing::info!(annotationsAllowList = ?opts.annotations_allow_list, "Using annotations allowlist");
    builder.allow_labels = builder
        .expand_allow_list(&opts.labels_allow_list)
        .map_err(|e| format!("failed to set up labels allowlist: {e}"))?;
    tracing::info!(labelsAllowList = ?opts.labels_allow_list, "Using labels allowlist");

    let handler = Arc::new(MetricsHandler::new(opts.enable_gzip_encoding));
    let stores_cancel = CancellationToken::new();
    // Stops the stores and discovery on every return, errors included.
    let _stores_guard = stores_cancel.clone().drop_guard();
    let settings = WatchSettings {
        client: client.clone(),
        field_selector: merged,
        sharding: Sharding::new(opts.shard, opts.total_shards),
        use_api_server_cache: opts.use_api_server_cache,
        object_limit: opts.object_limit,
        metrics: m.list_watch.clone(),
        cancel: stores_cancel.child_token(),
    };
    let shard_ctl = ShardControl {
        builder: Arc::new(builder),
        settings: parking_lot::Mutex::new(settings),
        handler: Arc::clone(&handler),
        metrics: m.sharding.clone(),
        current: parking_lot::Mutex::new(None),
        parent: stores_cancel.clone(),
        build_lock: parking_lot::Mutex::new(()),
    };
    let shard_ctl = Arc::new(shard_ctl);

    // A missing CRS config holds off on all custom resource operations.
    if let (Some(text), Some(discoverer)) = (crs_config, discoverer) {
        // The CRD discovery feeding the stores stops with them.
        let resources = ksm_crs::from_config(&text)?;
        discoverer.start(client.clone(), stores_cancel.clone());
        let configurer: Arc<dyn StoreConfigurer> = shard_ctl.clone();
        crate::crs::poll_for_cache_updates(
            resources,
            discoverer,
            configurer,
            stores_cancel.clone(),
        );
    }

    let metrics_app = metrics_router(Arc::clone(&handler), m.durations.clone(), client.clone());
    let telemetry_app = telemetry_router(m.registry.clone());
    let metrics_addr = listen_addr(&opts.host, opts.port)?;
    let telemetry_addr = listen_addr(&opts.telemetry_host, opts.telemetry_port)?;
    let metrics_listener = tokio::net::TcpListener::bind(metrics_addr)
        .await
        .map_err(|e| format!("run server group error: listen tcp {metrics_addr}: bind: {e}"))?;
    let telemetry_listener = tokio::net::TcpListener::bind(telemetry_addr)
        .await
        .map_err(|e| format!("run server group error: listen tcp {telemetry_addr}: bind: {e}"))?;

    // `web.ListenAndServe`'s configureServer: a bad --tls-config ends the run.
    let tls = if opts.tls_config.is_empty() {
        None
    } else {
        Some(
            crate::web::configure(&opts.tls_config)
                .map_err(|e| format!("run server group error: {e}"))?,
        )
    };

    let servers_stop = CancellationToken::new();
    let (s1, s2) = (servers_stop.clone(), servers_stop.clone());
    tracing::info!(telemetryAddress = %telemetry_addr, "Started kube-state-metrics self metrics server");
    let telemetry = spawn_server(telemetry_listener, telemetry_app, &opts.tls_config, tls, s1);
    tracing::info!(metricsServerAddress = %metrics_addr, "Started metrics server");
    let metrics_srv = spawn_server(metrics_listener, metrics_app, &opts.tls_config, tls, s2);

    let sharding_task = {
        let ctl = Arc::clone(&shard_ctl);
        let opts = opts.clone();
        let client = client.clone();
        let stop = stores_cancel.clone();
        tokio::spawn(async move { ctl.run(&opts, client, stop).await })
    };

    let mut result = Ok(());
    tokio::select! {
        () = cancel.cancelled() => {
            let delay = if reload.load(std::sync::atomic::Ordering::SeqCst) { Duration::ZERO } else { opts.shutdown_delay };
            if !delay.is_zero() {
                tracing::info!(delay = ?delay, "Delaying shutdown, so service discovery can stop sending scrapes first");
                tokio::time::sleep(delay).await;
            }
        }
        r = sharding_task => {
            if let Ok(Err(e)) = r {
                result = Err(format!("run server group error: {e}"));
            }
        }
    }
    servers_stop.cancel();
    let _ = tokio::time::timeout(SERVER_SHUTDOWN_TIMEOUT, async {
        let _ = metrics_srv.await;
        let _ = telemetry.await;
    })
    .await;
    stores_cancel.cancel();
    if result.is_ok() {
        tracing::info!("Exited");
    }
    result
}

/// Serves `app` until `stop`: plain axum without a web config, else the
/// exporter-toolkit behavior of [`crate::web`].
fn spawn_server(
    listener: tokio::net::TcpListener,
    app: Router,
    web_config: &str,
    tls: Option<bool>,
    stop: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let path = web_config.to_owned();
    tokio::spawn(async move {
        match tls {
            None => {
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async move { stop.cancelled().await })
                    .await;
            }
            Some(tls) => crate::web::serve(listener, app, path, tls, stop).await,
        }
    })
}

fn listen_addr(host: &str, port: i64) -> Result<SocketAddr, String> {
    let joined = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    if let Ok(a) = joined.parse() {
        return Ok(a);
    }
    std::net::ToSocketAddrs::to_socket_addrs(&joined)
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| format!("run server group error: listen tcp {joined}: address invalid"))
}

/// Owns the current sharding configuration and the stores built for it.
struct ShardControl {
    builder: Arc<Builder>,
    settings: parking_lot::Mutex<WatchSettings>,
    handler: Arc<MetricsHandler>,
    metrics: ShardingMetrics,
    current: parking_lot::Mutex<Option<(i32, i64, CancellationToken)>>,
    parent: CancellationToken,
    /// Serializes rebuilds (Go: the metrics handler's mutex).
    build_lock: parking_lot::Mutex<()>,
}

impl StoreConfigurer for ShardControl {
    /// `ConfigureStore` with `ReplaceEnabledCustomResources`: rebuilds the
    /// writers unless autosharding has not configured them yet.
    fn replace_custom_resources(&self, stores: BTreeMap<String, Arc<CustomResourceStore>>) {
        let _g = self.build_lock.lock();
        *self.builder.custom.write() = stores;
        let cur = self.current.lock().as_ref().map(|(s, t, _)| (*s, *t));
        if let Some((shard, total)) = cur {
            self.build_writers(shard, total);
        }
    }
}

impl ShardControl {
    /// `ConfigureSharding`: rebuilds every store for this shard.
    fn configure(&self, shard: i32, total: i64) {
        if total != 1 {
            tracing::info!(
                shard,
                totalShards = total,
                "Configuring sharding of this instance to be shard index (zero-indexed) out of total shards"
            );
        }
        self.metrics.ordinal.reset();
        self.metrics
            .ordinal
            .set(&[&shard.to_string()], f64::from(shard));
        self.metrics.total.set(&[], total as f64);
        let _g = self.build_lock.lock();
        self.build_writers(shard, total);
    }

    /// `buildWritersLocked`: stops the old watchers and builds new writers.
    fn build_writers(&self, shard: i32, total: i64) {
        let token = self.parent.child_token();
        let writers = {
            let mut s = self.settings.lock();
            s.sharding = Sharding::new(shard, total);
            s.cancel = token.clone();
            self.builder.build(&s)
        };
        let old = self.current.lock().replace((shard, total, token));
        self.handler.set_writers(writers);
        if let Some((_, _, t)) = old {
            t.cancel();
        }
    }

    fn unchanged(&self, shard: i32, total: i64) -> bool {
        self.current
            .lock()
            .as_ref()
            .is_some_and(|(s, t, _)| *s == shard && *t == total)
    }

    async fn run(
        &self,
        opts: &Options,
        client: kube::Client,
        stop: CancellationToken,
    ) -> Result<(), String> {
        if opts.pod.is_empty() || opts.namespace.is_empty() {
            tracing::info!("Autosharding disabled");
            self.configure(opts.shard, opts.total_shards);
            stop.cancelled().await;
            return Ok(());
        }
        tracing::info!(pod = %format!("{}/{}", opts.namespace, opts.pod), "Autosharding enabled with pod");
        tracing::info!("Auto detecting sharding settings");
        let ss_name = detect_stateful_set(&client, &opts.pod, &opts.namespace)
            .await
            .map_err(|e| format!("detect StatefulSet: {e}"))?;
        let api: Api<StatefulSet> = Api::namespaced(client, &opts.namespace);
        let cfg = watcher::Config::default().fields(&format!("metadata.name={ss_name}"));
        let stream = watcher(api, cfg);
        futures::pin_mut!(stream);
        loop {
            let ev = tokio::select! {
                () = stop.cancelled() => return Ok(()),
                ev = stream.next() => ev,
            };
            let Some(ev) = ev else { return Ok(()) };
            let ss = match ev {
                Ok(watcher::Event::Apply(ss) | watcher::Event::InitApply(ss)) => ss,
                Ok(_) => continue,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to watch StatefulSet");
                    continue;
                }
            };
            if ss.metadata.name.as_deref() != Some(ss_name.as_str()) {
                continue;
            }
            match sharding_from_stateful_set(&ss, &opts.pod) {
                Ok((shard, total)) => {
                    if !self.unchanged(shard, total) {
                        self.configure(shard, total);
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Detected sharding settings from StatefulSet")
                }
            }
        }
    }
}

fn sharding_from_stateful_set(ss: &StatefulSet, pod: &str) -> Result<(i32, i64), String> {
    let name = ss.metadata.name.as_deref().unwrap_or("");
    let nominal =
        detect_nominal_from_pod(name, pod).map_err(|e| format!("detecting Pod nominal: {e}"))?;
    let total = ss.spec.replicas.map_or(1, i64::from);
    Ok((nominal, total))
}

/// `detectNominalFromPod`.
pub fn detect_nominal_from_pod(ss_name: &str, pod: &str) -> Result<i32, String> {
    let prefix = format!("{ss_name}-");
    let nominal = pod.strip_prefix(&prefix).unwrap_or(pod);
    nominal.parse::<i32>().map_err(|_| {
        format!(
            "failed to detect shard index for Pod {pod} of StatefulSet {ss_name}, parsed {nominal}: strconv.ParseInt: parsing {nominal:?}: invalid syntax"
        )
    })
}

async fn detect_stateful_set(client: &kube::Client, pod: &str, ns: &str) -> Result<String, String> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    let p = pods
        .get(pod)
        .await
        .map_err(|e| format!("retrieve pod {pod} for sharding: {e}"))?;
    for o in p.metadata.owner_references.unwrap_or_default() {
        if o.api_version != "apps/v1" || o.kind != "StatefulSet" || o.controller != Some(true) {
            continue;
        }
        let ss: Api<StatefulSet> = Api::namespaced(client.clone(), ns);
        ss.get(&o.name)
            .await
            .map_err(|e| format!("retrieve shard's StatefulSet: {ns}/{}: {e}", o.name))?;
        return Ok(o.name);
    }
    Err(format!(
        "no suitable statefulset found for auto detecting sharding for Pod {ns}/{pod}"
    ))
}

fn landing(page: &'static str) -> Response {
    let body = page.replace("{{VERSION}}", &version_info());
    ([(header::CONTENT_TYPE, "text/html; charset=UTF-8")], body).into_response()
}

async fn fallback(req: Request, page: &'static str) -> Response {
    if req.uri().path() == "/" {
        landing(page)
    } else {
        plain_error(StatusCode::NOT_FOUND, "404 page not found")
    }
}

fn ok_text() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "OK",
    )
        .into_response()
}

fn method_label(m: &http::Method) -> String {
    match m.as_str() {
        "GET" | "HEAD" | "POST" | "PUT" | "DELETE" | "CONNECT" | "OPTIONS" | "PATCH" | "TRACE" => {
            m.as_str().to_lowercase()
        }
        _ => "unknown".to_owned(),
    }
}

fn metrics_router(
    handler: Arc<MetricsHandler>,
    durations: HistogramVec,
    client: kube::Client,
) -> Router {
    let metrics = move |req: Request| {
        let handler = Arc::clone(&handler);
        let durations = durations.clone();
        async move {
            let start = Instant::now();
            let method = method_label(req.method());
            let query = req.uri().query().map(str::to_owned);
            let resp = handler.serve(query, req.headers().clone()).await;
            durations.observe(&[&method], start.elapsed().as_secs_f64());
            resp
        }
    };
    let livez = move || {
        let client = client.clone();
        async move {
            let req = http::Request::get("/livez").body(Vec::new());
            let ok = match req {
                Ok(r) => matches!(
                    tokio::time::timeout(Duration::from_secs(10), client.request_text(r)).await,
                    Ok(Ok(_))
                ),
                Err(_) => false,
            };
            if ok {
                ok_text()
            } else {
                tracing::warn!("Failed to contact API server for /livez");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    "Service Unavailable",
                )
                    .into_response()
            }
        }
    };
    Router::new()
        .route("/metrics", axum::routing::any(metrics))
        .route("/livez", axum::routing::any(livez))
        .route("/healthz", axum::routing::any(|| async { ok_text() }))
        .fallback(|req: Request| fallback(req, include_str!("landing/metrics.html")))
}

fn telemetry_router(registry: Registry) -> Router {
    let reg = registry.clone();
    let metrics = move || {
        let reg = reg.clone();
        async move {
            let body = reg.render(&crate::process::families());
            (
                [(
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8; escaping=underscores",
                )],
                body,
            )
                .into_response()
        }
    };
    let readyz = move || {
        let reg = registry.clone();
        async move {
            if reg.count() == 0 {
                (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response()
            } else {
                (StatusCode::OK, "OK").into_response()
            }
        }
    };
    Router::new()
        .route("/metrics", axum::routing::any(metrics))
        .route("/readyz", axum::routing::any(readyz))
        .fallback(|req: Request| fallback(req, include_str!("landing/telemetry.html")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal() {
        assert_eq!(detect_nominal_from_pod("ksm", "ksm-3"), Ok(3));
        assert!(detect_nominal_from_pod("ksm", "other-x").is_err());
    }

    #[test]
    fn clean() {
        assert_eq!(clean_path("/a/./b/../c//"), "/a/c");
        assert_eq!(clean_path("a/.."), ".");
        assert_eq!(clean_path("config.yaml"), "config.yaml");
    }

    #[test]
    fn md5_value() {
        // md5("") = d41d8cd98f00b204...; first 6 bytes little endian.
        let want = u64::from_le_bytes([0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0, 0]) as f64;
        assert_eq!(md5_hash_as_metric_value(b""), want);
    }
}
