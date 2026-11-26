//! A deterministic stand-in for the Kubernetes API server (ADR-007, ADR-008).
//!
//! Serves the objects of fixture files (YAML or JSON, multi-document) or of a
//! generated synthetic cluster, read-only: list (with namespaces, field
//! selectors and paging), watch (held open; with `sendInitialEvents=true` it
//! sends every object then the initial-events-end bookmark), get, `/version`,
//! discovery and health endpoints. Enough for client-go (kube-state-metrics in
//! Go) and kube-rs (the Rust port) to sync and serve the same metrics.
//!
//! ```text
//! fake-apiserver --listen 127.0.0.1:6443 --fixtures testdata/fixtures [--generate pods=10000,nodes=100]
//!                [--kubeconfig-out /tmp/kubeconfig]
//!                [--auth-token TOKEN=USER[,GROUP...]]... [--auth-allow USER:VERB:PATH]...
//! ```
//!
//! TokenReview and SubjectAccessReview (POST) answer from `--auth-token` and
//! `--auth-allow` (`*` matches any verb or path): an unknown token is not
//! authenticated, a token `error` gets `status.error`, a user `deny-*` gets
//! `denied: true`, a user `both` gets `allowed` and `denied`.

mod generate;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};

/// A built-in or custom resource type.
#[derive(Clone, Debug)]
struct ResourceType {
    group: String,
    version: String,
    kind: String,
    plural: String,
    namespaced: bool,
}

impl ResourceType {
    fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }
}

/// `(group, version, kind, plural, namespaced)` of the built-in types kube-state-metrics watches.
const BUILTIN: &[(&str, &str, &str, &str, bool)] = &[
    ("", "v1", "ConfigMap", "configmaps", true),
    ("", "v1", "Endpoints", "endpoints", true),
    ("", "v1", "LimitRange", "limitranges", true),
    ("", "v1", "Namespace", "namespaces", false),
    ("", "v1", "Node", "nodes", false),
    ("", "v1", "PersistentVolume", "persistentvolumes", false),
    (
        "",
        "v1",
        "PersistentVolumeClaim",
        "persistentvolumeclaims",
        true,
    ),
    ("", "v1", "Pod", "pods", true),
    (
        "",
        "v1",
        "ReplicationController",
        "replicationcontrollers",
        true,
    ),
    ("", "v1", "ResourceQuota", "resourcequotas", true),
    ("", "v1", "Secret", "secrets", true),
    ("", "v1", "Service", "services", true),
    ("", "v1", "ServiceAccount", "serviceaccounts", true),
    (
        "admissionregistration.k8s.io",
        "v1",
        "MutatingAdmissionPolicy",
        "mutatingadmissionpolicies",
        false,
    ),
    (
        "admissionregistration.k8s.io",
        "v1",
        "MutatingAdmissionPolicyBinding",
        "mutatingadmissionpolicybindings",
        false,
    ),
    (
        "admissionregistration.k8s.io",
        "v1",
        "MutatingWebhookConfiguration",
        "mutatingwebhookconfigurations",
        false,
    ),
    (
        "admissionregistration.k8s.io",
        "v1",
        "ValidatingAdmissionPolicy",
        "validatingadmissionpolicies",
        false,
    ),
    (
        "admissionregistration.k8s.io",
        "v1",
        "ValidatingAdmissionPolicyBinding",
        "validatingadmissionpolicybindings",
        false,
    ),
    (
        "admissionregistration.k8s.io",
        "v1",
        "ValidatingWebhookConfiguration",
        "validatingwebhookconfigurations",
        false,
    ),
    (
        "apiextensions.k8s.io",
        "v1",
        "CustomResourceDefinition",
        "customresourcedefinitions",
        false,
    ),
    ("apps", "v1", "DaemonSet", "daemonsets", true),
    ("apps", "v1", "Deployment", "deployments", true),
    ("apps", "v1", "ReplicaSet", "replicasets", true),
    ("apps", "v1", "StatefulSet", "statefulsets", true),
    (
        "autoscaling",
        "v2",
        "HorizontalPodAutoscaler",
        "horizontalpodautoscalers",
        true,
    ),
    ("batch", "v1", "CronJob", "cronjobs", true),
    ("batch", "v1", "Job", "jobs", true),
    (
        "certificates.k8s.io",
        "v1",
        "CertificateSigningRequest",
        "certificatesigningrequests",
        false,
    ),
    ("coordination.k8s.io", "v1", "Lease", "leases", true),
    (
        "discovery.k8s.io",
        "v1",
        "EndpointSlice",
        "endpointslices",
        true,
    ),
    ("networking.k8s.io", "v1", "Ingress", "ingresses", true),
    (
        "networking.k8s.io",
        "v1",
        "IngressClass",
        "ingressclasses",
        false,
    ),
    (
        "networking.k8s.io",
        "v1",
        "NetworkPolicy",
        "networkpolicies",
        true,
    ),
    (
        "policy",
        "v1",
        "PodDisruptionBudget",
        "poddisruptionbudgets",
        true,
    ),
    (
        "rbac.authorization.k8s.io",
        "v1",
        "ClusterRole",
        "clusterroles",
        false,
    ),
    (
        "rbac.authorization.k8s.io",
        "v1",
        "ClusterRoleBinding",
        "clusterrolebindings",
        false,
    ),
    ("rbac.authorization.k8s.io", "v1", "Role", "roles", true),
    (
        "rbac.authorization.k8s.io",
        "v1",
        "RoleBinding",
        "rolebindings",
        true,
    ),
    (
        "storage.k8s.io",
        "v1",
        "StorageClass",
        "storageclasses",
        false,
    ),
    (
        "storage.k8s.io",
        "v1",
        "VolumeAttachment",
        "volumeattachments",
        false,
    ),
    (
        "storage.k8s.io",
        "v1",
        "VolumeAttributesClass",
        "volumeattributesclasses",
        false,
    ),
];

struct Cluster {
    types: Vec<ResourceType>,
    /// Objects by `(group, plural)`, in fixture order.
    objects: BTreeMap<(String, String), Vec<Value>>,
    resource_version: u64,
    git_version: String,
    /// token → (user, groups).
    tokens: BTreeMap<String, (String, Vec<String>)>,
    /// (user, verb, path) allowed.
    allow: Vec<(String, String, String)>,
}

type Shared = Arc<Cluster>;

fn builtin_types() -> Vec<ResourceType> {
    BUILTIN
        .iter()
        .map(|(g, v, k, p, n)| ResourceType {
            group: (*g).into(),
            version: (*v).into(),
            kind: (*k).into(),
            plural: (*p).into(),
            namespaced: *n,
        })
        .collect()
}

impl Cluster {
    fn new(git_version: String) -> Self {
        Cluster {
            types: builtin_types(),
            objects: BTreeMap::new(),
            resource_version: 1000,
            git_version,
            tokens: BTreeMap::new(),
            allow: Vec::new(),
        }
    }

    fn type_of(&self, api_version: &str, kind: &str) -> Option<&ResourceType> {
        self.types
            .iter()
            .find(|t| t.api_version() == api_version && t.kind == kind)
    }

    /// Registers the types a CustomResourceDefinition declares.
    fn learn_crd(&mut self, crd: &Value) {
        let spec = &crd["spec"];
        let group = spec["group"].as_str().unwrap_or_default().to_owned();
        let kind = spec["names"]["kind"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let plural = spec["names"]["plural"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let namespaced = spec["scope"].as_str() != Some("Cluster");
        for v in spec["versions"].as_array().into_iter().flatten() {
            if v["served"].as_bool() == Some(false) {
                continue;
            }
            self.types.push(ResourceType {
                group: group.clone(),
                version: v["name"].as_str().unwrap_or_default().to_owned(),
                kind: kind.clone(),
                plural: plural.clone(),
                namespaced,
            });
        }
    }

    fn add(&mut self, mut obj: Value) -> Result<(), String> {
        let api_version = obj["apiVersion"].as_str().unwrap_or_default().to_owned();
        let kind = obj["kind"].as_str().unwrap_or_default().to_owned();
        if kind.ends_with("List") && obj["items"].is_array() {
            let items = obj["items"].as_array().cloned().unwrap_or_default();
            for mut item in items {
                if item.get("apiVersion").is_none() {
                    item["apiVersion"] = json!(api_version);
                }
                if item.get("kind").is_none() {
                    item["kind"] = json!(kind.trim_end_matches("List"));
                }
                self.add(item)?;
            }
            return Ok(());
        }
        if kind == "CustomResourceDefinition" {
            self.learn_crd(&obj);
        }
        let t = self
            .type_of(&api_version, &kind)
            .ok_or_else(|| format!("unknown type {api_version} {kind}"))?
            .clone();
        self.resource_version += 1;
        let meta = obj["metadata"]
            .as_object_mut()
            .ok_or("object without metadata")?;
        if t.namespaced && !meta.contains_key("namespace") {
            meta.insert("namespace".into(), json!("default"));
        }
        meta.entry("resourceVersion")
            .or_insert(json!(self.resource_version.to_string()));
        if !meta.contains_key("uid") {
            let id = format!(
                "{}-{}-{}",
                t.plural,
                meta.get("namespace").and_then(Value::as_str).unwrap_or("_"),
                meta.get("name").and_then(Value::as_str).unwrap_or("")
            );
            meta.insert("uid".into(), json!(generate::uid_for(&id)));
        }
        self.objects
            .entry((t.group.clone(), t.plural.clone()))
            .or_default()
            .push(obj);
        Ok(())
    }

    fn load_path(&mut self, path: &std::path::Path) -> Result<(), String> {
        if path.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .collect();
            entries.sort();
            // CRDs first, so their custom resources are known.
            entries.sort_by_key(|p| !p.to_string_lossy().contains("crd"));
            for p in entries {
                self.load_path(&p)?;
            }
            return Ok(());
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "yaml" | "yml" | "json") {
            return Ok(());
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for doc in serde_yaml_ng::Deserializer::from_str(&text) {
            let v: Value = serde::Deserialize::deserialize(doc)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if v.is_null() {
                continue;
            }
            self.add(v)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ListParams {
    watch: Option<String>,
    field_selector: Option<String>,
    label_selector: Option<String>,
    limit: Option<usize>,
    #[serde(rename = "continue")]
    cont: Option<String>,
    send_initial_events: Option<String>,
    timeout_seconds: Option<u64>,
}

/// Value of a dotted field path (`metadata.namespace`, `spec.nodeName`).
fn field<'a>(obj: &'a Value, path: &str) -> &'a str {
    let mut v = obj;
    for p in path.split('.') {
        v = &v[p];
    }
    v.as_str().unwrap_or("")
}

/// Field selector terms `a=b`, `a==b`, `a!=b`, joined by commas.
fn matches_fields(obj: &Value, selector: &str) -> bool {
    selector.split(',').filter(|t| !t.is_empty()).all(|term| {
        if let Some((k, v)) = term.split_once("!=") {
            field(obj, k) != v
        } else if let Some((k, v)) = term.split_once("==").or_else(|| term.split_once('=')) {
            field(obj, k) == v
        } else {
            true
        }
    })
}

/// Equality label selector terms (`a=b`, `a!=b`, `a`).
fn matches_labels(obj: &Value, selector: &str) -> bool {
    let labels = &obj["metadata"]["labels"];
    selector.split(',').filter(|t| !t.is_empty()).all(|term| {
        if let Some((k, v)) = term.split_once("!=") {
            labels[k].as_str() != Some(v)
        } else if let Some((k, v)) = term.split_once("==").or_else(|| term.split_once('=')) {
            labels[k].as_str() == Some(v)
        } else {
            labels.get(term).is_some()
        }
    })
}

fn status(code: StatusCode, reason: &str, message: &str) -> Response {
    let body = json!({"kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
        "message": message, "reason": reason, "code": code.as_u16()});
    (
        code,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn json_response(v: &Value) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

fn selected<'a>(
    c: &'a Cluster,
    t: &ResourceType,
    ns: Option<&str>,
    p: &ListParams,
) -> Vec<&'a Value> {
    c.objects
        .get(&(t.group.clone(), t.plural.clone()))
        .map(|v| v.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|o| ns.is_none_or(|ns| field(o, "metadata.namespace") == ns))
        .filter(|o| {
            p.field_selector
                .as_deref()
                .is_none_or(|s| matches_fields(o, s))
        })
        .filter(|o| {
            p.label_selector
                .as_deref()
                .is_none_or(|s| matches_labels(o, s))
        })
        .collect()
}

async fn collection(
    c: Shared,
    group: String,
    version: String,
    ns: Option<String>,
    plural: String,
    p: ListParams,
) -> Response {
    let Some(t) = c
        .types
        .iter()
        .find(|t| t.group == group && t.version == version && t.plural == plural)
        .cloned()
    else {
        return status(
            StatusCode::NOT_FOUND,
            "NotFound",
            &format!("the server could not find the requested resource ({plural})"),
        );
    };
    let rv = c.resource_version.to_string();
    if p.watch.as_deref() == Some("true") || p.watch.as_deref() == Some("1") {
        let initial = p.send_initial_events.as_deref() == Some("true");
        let objs: Vec<Value> = if initial {
            selected(&c, &t, ns.as_deref(), &p)
                .into_iter()
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let timeout = Duration::from_secs(p.timeout_seconds.unwrap_or(1800).min(3600));
        let api_version = t.api_version();
        let kind = t.kind.clone();
        let stream = async_stream::stream! {
            for o in objs {
                yield Ok::<_, std::io::Error>(format!("{}\n", json!({"type": "ADDED", "object": o})));
            }
            if initial {
                let bm = json!({"type": "BOOKMARK", "object": {"kind": kind, "apiVersion": api_version,
                    "metadata": {"resourceVersion": rv, "annotations": {"k8s.io/initial-events-end": "true"}}}});
                yield Ok(format!("{bm}\n"));
            }
            tokio::time::sleep(timeout).await;
        };
        return Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from_stream(stream))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }
    let all = selected(&c, &t, ns.as_deref(), &p);
    let start: usize = p.cont.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
    let limit = p.limit.filter(|l| *l > 0).unwrap_or(usize::MAX);
    let end = start.saturating_add(limit).min(all.len());
    let items: Vec<&Value> = all.get(start..end).unwrap_or_default().to_vec();
    let mut meta = json!({"resourceVersion": rv});
    if end < all.len() {
        meta["continue"] = json!(end.to_string());
        meta["remainingItemCount"] = json!(all.len() - end);
    }
    json_response(
        &json!({"kind": format!("{}List", t.kind), "apiVersion": t.api_version(), "metadata": meta, "items": items}),
    )
}

async fn object(
    c: Shared,
    group: String,
    version: String,
    ns: Option<String>,
    plural: String,
    name: String,
) -> Response {
    let Some(t) = c
        .types
        .iter()
        .find(|t| t.group == group && t.version == version && t.plural == plural)
    else {
        return status(
            StatusCode::NOT_FOUND,
            "NotFound",
            "the server could not find the requested resource",
        );
    };
    let found = c
        .objects
        .get(&(t.group.clone(), t.plural.clone()))
        .and_then(|v| {
            v.iter().find(|o| {
                field(o, "metadata.name") == name
                    && ns
                        .as_deref()
                        .is_none_or(|ns| field(o, "metadata.namespace") == ns)
            })
        });
    match found {
        Some(o) => json_response(o),
        None => status(
            StatusCode::NOT_FOUND,
            "NotFound",
            &format!("{plural} \"{name}\" not found"),
        ),
    }
}

/// TokenReview / SubjectAccessReview from the `--auth-*` flags.
fn review(c: &Cluster, group: &str, resource: &str, body: &str) -> Response {
    let Ok(mut req) = serde_json::from_str::<Value>(body) else {
        return status(StatusCode::BAD_REQUEST, "BadRequest", "invalid body");
    };
    match (group, resource) {
        ("authentication.k8s.io", "tokenreviews") => {
            let token = req["spec"]["token"].as_str().unwrap_or("");
            req["status"] = match c.tokens.get(token) {
                Some((user, groups)) => json!({"authenticated": true,
                    "user": {"username": user, "uid": format!("uid-{user}"), "groups": groups}}),
                None if token == "error" => {
                    json!({"authenticated": false, "error": "token review failed"})
                }
                None => json!({"authenticated": false}),
            };
            json_response(&req)
        }
        ("authorization.k8s.io", "subjectaccessreviews") => {
            let spec = &req["spec"];
            let user = spec["user"].as_str().unwrap_or("");
            let verb = spec["nonResourceAttributes"]["verb"].as_str().unwrap_or("");
            let path = spec["nonResourceAttributes"]["path"].as_str().unwrap_or("");
            let allowed = c
                .allow
                .iter()
                .any(|(u, v, p)| u == user && (v == "*" || v == verb) && (p == "*" || p == path));
            req["status"] = if user == "both" {
                json!({"allowed": true, "denied": true})
            } else if user.starts_with("deny-") {
                json!({"allowed": false, "denied": true, "reason": "denied by fake"})
            } else {
                json!({"allowed": allowed})
            };
            json_response(&req)
        }
        _ => status(
            StatusCode::METHOD_NOT_ALLOWED,
            "MethodNotAllowed",
            "read-only fake",
        ),
    }
}

fn resource_list(c: &Cluster, group: &str, version: &str) -> Value {
    let resources: Vec<Value> = c
        .types
        .iter()
        .filter(|t| t.group == group && t.version == version)
        .map(|t| json!({"name": t.plural, "singularName": t.kind.to_lowercase(), "namespaced": t.namespaced,
            "kind": t.kind, "verbs": ["get", "list", "watch"]}))
        .collect();
    let gv = if group.is_empty() {
        version.to_owned()
    } else {
        format!("{group}/{version}")
    };
    json!({"kind": "APIResourceList", "apiVersion": "v1", "groupVersion": gv, "resources": resources})
}

fn router(c: Shared) -> Router {
    Router::new()
        .route("/version", get(|State(c): State<Shared>| async move {
            json_response(&json!({"major": "1", "minor": "37", "gitVersion": c.git_version, "gitCommit": "fake",
                "gitTreeState": "clean", "buildDate": "2026-01-01T00:00:00Z", "goVersion": "go1.27",
                "compiler": "gc", "platform": "linux/amd64"}))
        }))
        .route("/healthz", get(|| async { "ok" }))
        .route("/livez", get(|| async { "ok" }))
        .route("/readyz", get(|| async { "ok" }))
        .route("/api", get(|| async { json_response(&json!({"kind": "APIVersions", "versions": ["v1"],
            "serverAddressByClientCIDRs": [{"clientCIDR": "0.0.0.0/0", "serverAddress": "127.0.0.1"}]})) }))
        .route("/api/v1", get(|State(c): State<Shared>| async move { json_response(&resource_list(&c, "", "v1")) }))
        .route("/apis", get(|State(c): State<Shared>| async move {
            let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for t in c.types.iter().filter(|t| !t.group.is_empty()) {
                let v = groups.entry(&t.group).or_default();
                if !v.contains(&t.version.as_str()) { v.push(&t.version); }
            }
            let groups: Vec<Value> = groups.iter().map(|(g, vs)| {
                let versions: Vec<Value> = vs.iter().map(|v| json!({"groupVersion": format!("{g}/{v}"), "version": v})).collect();
                json!({"name": g, "versions": versions, "preferredVersion": versions[0]})
            }).collect();
            json_response(&json!({"kind": "APIGroupList", "apiVersion": "v1", "groups": groups}))
        }))
        .route("/apis/{g}/{v}", get(|State(c): State<Shared>, Path((g, v)): Path<(String, String)>| async move {
            json_response(&resource_list(&c, &g, &v))
        }))
        .route("/api/v1/{r}", get(|State(c): State<Shared>, Path(r): Path<String>, Query(p): Query<ListParams>| async move {
            collection(c, String::new(), "v1".into(), None, r, p).await
        }))
        .route("/api/v1/{r}/{name}", get(|State(c): State<Shared>, Path((r, n)): Path<(String, String)>| async move {
            object(c, String::new(), "v1".into(), None, r, n).await
        }))
        .route("/api/v1/namespaces/{ns}/{r}", get(|State(c): State<Shared>, Path((ns, r)): Path<(String, String)>, Query(p): Query<ListParams>| async move {
            collection(c, String::new(), "v1".into(), Some(ns), r, p).await
        }))
        .route("/api/v1/namespaces/{ns}/{r}/{name}", get(|State(c): State<Shared>, Path((ns, r, n)): Path<(String, String, String)>| async move {
            object(c, String::new(), "v1".into(), Some(ns), r, n).await
        }))
        .route("/apis/{g}/{v}/{r}", get(|State(c): State<Shared>, Path((g, v, r)): Path<(String, String, String)>, Query(p): Query<ListParams>| async move {
            collection(c, g, v, None, r, p).await
        }).post(|State(c): State<Shared>, Path((g, _v, r)): Path<(String, String, String)>, body: String| async move {
            review(&c, &g, &r, &body)
        }))
        .route("/apis/{g}/{v}/{r}/{name}", get(|State(c): State<Shared>, Path((g, v, r, n)): Path<(String, String, String, String)>| async move {
            object(c, g, v, None, r, n).await
        }))
        .route("/apis/{g}/{v}/namespaces/{ns}/{r}", get(|State(c): State<Shared>, Path((g, v, ns, r)): Path<(String, String, String, String)>, Query(p): Query<ListParams>| async move {
            collection(c, g, v, Some(ns), r, p).await
        }))
        .route("/apis/{g}/{v}/namespaces/{ns}/{r}/{name}", get(|State(c): State<Shared>, Path((g, v, ns, r, n)): Path<(String, String, String, String, String)>| async move {
            object(c, g, v, Some(ns), r, n).await
        }))
        .with_state(c)
}

fn usage() -> ! {
    eprintln!(
        "usage: fake-apiserver [--listen ADDR] [--fixtures PATH]... [--generate kind=n,...] [--seed N] [--kubeconfig-out PATH] [--git-version V] [--auth-token TOKEN=USER[,GROUP...]]... [--auth-allow USER:VERB:PATH]..."
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let mut listen: SocketAddr = ([127, 0, 0, 1], 0).into();
    let mut fixtures = Vec::new();
    let mut generate_spec = None;
    let mut seed = 1u64;
    let mut kubeconfig_out = None;
    let mut git_version = "v1.37.0-fake".to_owned();
    let mut auth_tokens = BTreeMap::new();
    let mut auth_allow = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--listen" => listen = val().parse().unwrap_or_else(|_| usage()),
            "--fixtures" => fixtures.push(val()),
            "--generate" => generate_spec = Some(val()),
            "--seed" => seed = val().parse().unwrap_or_else(|_| usage()),
            "--kubeconfig-out" => kubeconfig_out = Some(val()),
            "--git-version" => git_version = val(),
            "--auth-token" => {
                let v = val();
                let (tok, rest) = v.split_once('=').unwrap_or_else(|| usage());
                let mut parts = rest.split(',').map(str::to_owned);
                let user = parts.next().unwrap_or_default();
                auth_tokens.insert(tok.to_owned(), (user, parts.collect()));
            }
            "--auth-allow" => {
                let v = val();
                let mut parts = v.splitn(3, ':').map(str::to_owned);
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(u), Some(verb), Some(p)) => auth_allow.push((u, verb, p)),
                    _ => usage(),
                }
            }
            _ => usage(),
        }
    }
    let mut cluster = Cluster::new(git_version);
    cluster.tokens = auth_tokens;
    cluster.allow = auth_allow;
    for f in &fixtures {
        if let Err(e) = cluster.load_path(std::path::Path::new(f)) {
            eprintln!("fake-apiserver: {e}");
            std::process::exit(1);
        }
    }
    if let Some(spec) = generate_spec {
        for obj in generate::cluster(&spec, seed).unwrap_or_else(|e| {
            eprintln!("fake-apiserver: {e}");
            std::process::exit(2)
        }) {
            if let Err(e) = cluster.add(obj) {
                eprintln!("fake-apiserver: {e}");
                std::process::exit(1);
            }
        }
    }
    let total: usize = cluster.objects.values().map(Vec::len).sum();
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .unwrap_or_else(|e| {
            eprintln!("fake-apiserver: bind {listen}: {e}");
            std::process::exit(1)
        });
    let addr = listener.local_addr().unwrap_or(listen);
    if let Some(path) = kubeconfig_out {
        let kc = format!(
            "apiVersion: v1\nkind: Config\nclusters:\n- name: fake\n  cluster:\n    server: http://{addr}\ncontexts:\n- name: fake\n  context:\n    cluster: fake\n    user: fake\ncurrent-context: fake\nusers:\n- name: fake\n  user:\n    token: fake\n"
        );
        if let Err(e) = std::fs::write(&path, kc) {
            eprintln!("fake-apiserver: write {path}: {e}");
            std::process::exit(1);
        }
    }
    eprintln!("fake-apiserver: serving {total} objects on http://{addr}");
    if let Err(e) = axum::serve(listener, router(Arc::new(cluster))).await {
        eprintln!("fake-apiserver: {e}");
        std::process::exit(1);
    }
}
