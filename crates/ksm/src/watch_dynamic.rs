//! Watchers of custom resources (Go: a client-go reflector over the dynamic
//! client, `*unstructured.Unstructured`, feeding a MetricsStore). Same event
//! mapping as `watch.rs` (ADR-011); objects are kept as the raw JSON map, as
//! Go's unstructured objects are.

use std::borrow::Cow;
use std::sync::Arc;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ksm_core::generator::FamilyGenerator;
use ksm_core::store::MetricsStore;
use kube::api::{Api, ApiResource};
use kube::core::DynamicResourceScope;
use kube::runtime::watcher;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::watch::{Driver, WatchSettings, drive, keep, watcher_config};

/// Go's `reflect.TypeOf(expectedType).String()` for custom resources.
pub const GO_TYPE: &str = "*unstructured.Unstructured";

/// An object of a kind only known at runtime, kept as Go's unstructured
/// map. Only the metadata fields the watcher and sharding need are parsed.
#[derive(Clone, Debug)]
pub struct Unstructured {
    meta: ObjectMeta,
    /// The whole object as delivered by the API server.
    pub raw: Value,
}

impl<'de> Deserialize<'de> for Unstructured {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        let m = &raw["metadata"];
        let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_owned);
        let meta = ObjectMeta {
            name: s("name"),
            namespace: s("namespace"),
            uid: s("uid"),
            resource_version: s("resourceVersion"),
            ..ObjectMeta::default()
        };
        Ok(Unstructured { meta, raw })
    }
}

impl kube::Resource for Unstructured {
    type DynamicType = ApiResource;
    type Scope = DynamicResourceScope;

    fn kind(dt: &ApiResource) -> Cow<'_, str> {
        dt.kind.as_str().into()
    }
    fn group(dt: &ApiResource) -> Cow<'_, str> {
        dt.group.as_str().into()
    }
    fn version(dt: &ApiResource) -> Cow<'_, str> {
        dt.version.as_str().into()
    }
    fn api_version(dt: &ApiResource) -> Cow<'_, str> {
        dt.api_version.as_str().into()
    }
    fn plural(dt: &ApiResource) -> Cow<'_, str> {
        dt.plural.as_str().into()
    }
    fn meta(&self) -> &ObjectMeta {
        &self.meta
    }
    fn meta_mut(&mut self) -> &mut ObjectMeta {
        &mut self.meta
    }
}

impl Unstructured {
    /// The object as Go's unstructured map: list items without
    /// `apiVersion`/`kind` get the list's, as `UnstructuredList` does.
    fn into_value(mut self, ar: &ApiResource) -> Value {
        if let Value::Object(m) = &mut self.raw {
            m.entry("apiVersion")
                .or_insert_with(|| Value::String(ar.api_version.clone()));
            m.entry("kind")
                .or_insert_with(|| Value::String(ar.kind.clone()));
        }
        self.raw
    }

    /// The object's `metadata.name`.
    pub fn name(&self) -> &str {
        self.meta.name.as_deref().unwrap_or("")
    }
}

/// The API of `ar` in `namespace` (`""` is all), as the dynamic client's
/// `Resource(gvr).Namespace(ns)`.
pub fn api(client: kube::Client, ar: &ApiResource, namespace: &str) -> Api<Unstructured> {
    if namespace.is_empty() {
        Api::all_with(client, ar)
    } else {
        Api::namespaced_with(client, namespace, ar)
    }
}

/// Starts a watcher of the custom resource `ar` feeding `store`, until the
/// settings' token or `gvk_stop` (the CRD left discovery) is cancelled.
pub fn spawn_dynamic(
    settings: &WatchSettings,
    ar: &ApiResource,
    namespace: &str,
    families: Arc<Vec<FamilyGenerator<Value>>>,
    store: Arc<MetricsStore>,
    gvk_stop: CancellationToken,
) {
    let stream = watcher(
        api(settings.client.clone(), ar, namespace),
        watcher_config(settings),
    );
    let check_listed = settings.object_limit <= 0;
    let sharding = settings.sharding;
    let (ar1, ar2) = (ar.clone(), ar.clone());
    tokio::spawn(drive(
        stream,
        Driver {
            settings: settings.clone(),
            go_type: GO_TYPE,
            resource: ar.plural.clone(),
            families,
            store,
            stop: Some(gvk_stop),
        },
        // client-go's reflector skips objects of another apiVersion/kind in
        // watch events when its expected type carries a GVK (a listed object,
        // without WatchList, is not checked).
        move |obj: &Unstructured, initial| {
            if (!initial || check_listed) && !has_gvk(obj, &ar1) {
                return None;
            }
            keep(&sharding, &obj.meta)
        },
        move |obj| obj.into_value(&ar2),
    ));
}

/// Whether `obj` has `ar`'s apiVersion and kind (logged when not).
fn has_gvk(obj: &Unstructured, ar: &ApiResource) -> bool {
    let field = |k: &str| obj.raw.get(k).and_then(Value::as_str).unwrap_or("");
    let same = field("apiVersion") == ar.api_version && field("kind") == ar.kind;
    if !same {
        tracing::error!(
            expected = %format!("{}, Kind={}", ar.api_version, ar.kind),
            got = %format!("{}, Kind={}", field("apiVersion"), field("kind")),
            "expected gvk, but watch event object had another gvk"
        );
    }
    same
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstructured_keeps_raw_and_fills_type() {
        let o: Unstructured = serde_json::from_str(
            r#"{"metadata":{"name":"a","namespace":"n","uid":"u","resourceVersion":"7","creationTimestamp":"2024-01-02T03:04:05Z"},"spec":{"x":1}}"#,
        )
        .unwrap();
        assert_eq!(o.name(), "a");
        let ar = ApiResource {
            group: "g".into(),
            version: "v1".into(),
            api_version: "g/v1".into(),
            kind: "Foo".into(),
            plural: "foos".into(),
        };
        let v = o.into_value(&ar);
        assert_eq!(v["apiVersion"], "g/v1");
        assert_eq!(v["kind"], "Foo");
        assert_eq!(v["metadata"]["creationTimestamp"], "2024-01-02T03:04:05Z");
    }
}
