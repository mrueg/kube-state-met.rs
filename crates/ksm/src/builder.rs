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

/// What every build of the stores needs.
pub struct Builder {
    /// Enabled built-in resources, sorted and unique.
    pub enabled: Vec<String>,
    /// Namespaces to watch; `[""]` is all.
    pub namespaces: Vec<String>,
    pub filter: Arc<dyn FamilyFilter>,
    pub allow_annotations: BTreeMap<String, Vec<String>>,
    pub allow_labels: BTreeMap<String, Vec<String>>,
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

    /// Starts the watchers of every enabled resource and returns the writers,
    /// in resource-name order.
    pub fn build(&self, settings: &WatchSettings) -> Vec<Arc<MetricsWriter>> {
        if !settings.field_selector.is_empty() {
            tracing::info!(fieldSelector = %settings.field_selector, "FieldSelector is used");
        }
        let mut writers = Vec::new();
        for name in &self.enabled {
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
            tracing::info!(activeStoreNames = %self.enabled.join(","), "Active resources");
        }
        writers
    }
}
