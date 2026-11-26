//! Builds one metrics writer per enabled resource and starts its watchers
//! (Go: `internal/store/builder.go`).

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use ksm_core::generator::{FamilyFilter, FamilyGenerator, extract_headers, filter_families};
use ksm_core::store::{Headers, MetricsStore, MetricsWriter};
use ksm_stores::for_each_resource;
use kube::Resource;
use serde::de::DeserializeOwned;

use crate::watch::{ScopeApi, WatchSettings, spawn};
use crate::watch_dynamic::{GO_TYPE as CR_GO_TYPE, spawn_dynamic};

/// A custom resource store factory (Go: the `customResourceMetrics`
/// registry factory registered under its GVR string).
pub struct CustomResourceStore {
    pub api_resource: kube::api::ApiResource,
    /// The resolved custom resource state configuration.
    pub resource: ksm_crs::Resource,
}

/// Returns the current stop token of a GVK (`GetGVKStopChan`).
pub type GvkStop = Arc<dyn Fn(&str) -> Option<tokio_util::sync::CancellationToken> + Send + Sync>;

/// What every build of the stores needs.
pub struct Builder {
    /// Enabled built-in resources, sorted and unique.
    pub enabled: Vec<String>,
    /// Namespaces to watch; `[""]` is all.
    pub namespaces: Vec<String>,
    pub filter: Arc<dyn FamilyFilter>,
    pub allow_annotations: BTreeMap<String, Vec<String>>,
    pub allow_labels: BTreeMap<String, Vec<String>>,
    /// Enabled custom resources by GVR string (`ReplaceEnabledCustomResources`).
    pub custom: parking_lot::RwLock<BTreeMap<String, Arc<CustomResourceStore>>>,
    /// Stop tokens of the discovered GVKs.
    pub gvk_stop: Option<GvkStop>,
}

/// Error text of an unknown resource (Go lists the available ones in map order;
/// here they are sorted).
pub fn unknown_resource(name: &str) -> String {
    format!(
        "resource {name} does not exist. Available resources: {}",
        ksm_stores::resource_names().join(",")
    )
}

impl Builder {
    /// `WithEnabledResources`: checks every name, sorts and dedups.
    pub fn set_enabled(&mut self, resources: &[String]) -> Result<(), String> {
        let known = ksm_stores::resource_names();
        for r in resources {
            if !known.contains(&r.as_str()) {
                return Err(unknown_resource(r));
            }
        }
        self.enabled.extend(resources.iter().cloned());
        self.enabled.sort();
        self.enabled.dedup();
        Ok(())
    }

    /// `allowList`: checks the resources of an allow list; `*` applies the
    /// list to every enabled resource.
    pub fn expand_allow_list(
        &self,
        list: &BTreeMap<String, Vec<String>>,
    ) -> Result<BTreeMap<String, Vec<String>>, String> {
        if list.is_empty() {
            return Ok(BTreeMap::new());
        }
        let known = ksm_stores::resource_names();
        for r in list.keys() {
            if r != "*" && !known.contains(&r.as_str()) {
                return Err(unknown_resource(r));
            }
        }
        Ok(match list.get("*") {
            Some(all) => self
                .enabled
                .iter()
                .map(|r| (r.clone(), all.clone()))
                .collect(),
            None => list.clone(),
        })
    }

    fn build_one<K>(
        &self,
        settings: &WatchSettings,
        name: &'static str,
        go_type: &'static str,
        cluster_scoped: bool,
        families: fn(&[String], &[String]) -> Vec<FamilyGenerator<K>>,
    ) -> MetricsWriter
    where
        K: Resource<DynamicType = ()> + Clone + DeserializeOwned + Debug + Send + Sync + 'static,
        K::Scope: ScopeApi,
    {
        let empty = Vec::new();
        let ann = self.allow_annotations.get(name).unwrap_or(&empty);
        let lab = self.allow_labels.get(name).unwrap_or(&empty);
        let fams = filter_families(self.filter.as_ref(), families(ann, lab));
        let headers = Arc::new(Headers::new(extract_headers(&fams)));
        let fams = Arc::new(fams);
        let all = [String::new()];
        let namespaces: &[String] = if cluster_scoped {
            &all
        } else {
            &self.namespaces
        };
        let stores = namespaces
            .iter()
            .map(|ns| {
                let dropped = settings.metrics.objects_dropped.clone();
                let store = Arc::new(MetricsStore::new(
                    settings.object_limit,
                    Some(Box::new(move || dropped.inc(&[go_type]))),
                ));
                spawn::<K>(settings, ns, go_type, Arc::clone(&fams), Arc::clone(&store));
                store
            })
            .collect();
        MetricsWriter::new(name, headers, stores)
    }

    /// `buildCustomResourceStores`: one store per namespace (Go uses the
    /// namespaces whatever the scope of the custom resource).
    fn build_custom(
        &self,
        settings: &WatchSettings,
        name: &str,
        cr: &CustomResourceStore,
    ) -> Option<MetricsWriter> {
        let fams = match cr.resource.families() {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(error = %e, resourceName = name, "Failed to build custom resource families");
                return None;
            }
        };
        let fams = filter_families(self.filter.as_ref(), fams);
        let headers = Arc::new(Headers::new(extract_headers(&fams)));
        let fams = Arc::new(fams);
        let gvk = crate::discovery::gvk_key(
            &cr.api_resource.group,
            &cr.api_resource.version,
            &cr.api_resource.kind,
        );
        // No token (the GVK left discovery): Go's nil channel never closes.
        let stop = self
            .gvk_stop
            .as_ref()
            .and_then(|f| f(&gvk))
            .unwrap_or_default();
        let stores = self
            .namespaces
            .iter()
            .map(|ns| {
                let dropped = settings.metrics.objects_dropped.clone();
                let store = Arc::new(MetricsStore::new(
                    settings.object_limit,
                    Some(Box::new(move || dropped.inc(&[CR_GO_TYPE]))),
                ));
                spawn_dynamic(
                    settings,
                    &cr.api_resource,
                    ns,
                    Arc::clone(&fams),
                    Arc::clone(&store),
                    stop.clone(),
                );
                store
            })
            .collect();
        Some(MetricsWriter::new(name, headers, stores))
    }

    /// Starts the watchers of every enabled resource and returns the writers,
    /// in resource-name order (built-in names and custom resource GVR
    /// strings sorted together, as Go's `enabledResources`).
    pub fn build(&self, settings: &WatchSettings) -> Vec<Arc<MetricsWriter>> {
        if !settings.field_selector.is_empty() {
            tracing::info!(fieldSelector = %settings.field_selector, "FieldSelector is used");
        }
        let custom = self.custom.read().clone();
        let mut names: Vec<&String> = self.enabled.iter().chain(custom.keys()).collect();
        names.sort();
        names.dedup();
        let mut writers = Vec::new();
        for name in names {
            if let Some(cr) = custom.get(name.as_str()) {
                if let Some(w) = self.build_custom(settings, name, cr) {
                    writers.push(Arc::new(w));
                }
                continue;
            }
            macro_rules! dispatch {
                ($($module:ident, $ty:ty, $gofn:literal;)*) => {
                    match name.as_str() {
                        $(ksm_stores::$module::NAME => Some(self.build_one::<$ty>(
                            settings,
                            ksm_stores::$module::NAME,
                            ksm_stores::$module::GO_TYPE,
                            ksm_stores::$module::CLUSTER_SCOPED,
                            ksm_stores::$module::families,
                        )),)*
                        _ => None,
                    }
                };
            }
            if let Some(w) = for_each_resource!(dispatch) {
                writers.push(Arc::new(w));
            }
        }
        if !writers.is_empty() {
            let active: Vec<&str> = writers.iter().map(|w| w.resource_name.as_str()).collect();
            tracing::info!(activeStoreNames = %active.join(","), "Active resources");
        }
        writers
    }
}
