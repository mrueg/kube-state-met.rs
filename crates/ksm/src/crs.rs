//! Custom resource state metrics wiring (Go: `pkg/app/server.go`
//! `resolveCustomResourceConfig`, `customresourcestate.FromConfig`,
//! `internal/discovery` `PollForCacheUpdates`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kube::api::ApiResource;
use tokio_util::sync::CancellationToken;

use crate::builder::CustomResourceStore;
use crate::discovery::{CrDiscoverer, gvk_key};
use crate::options::Options;

/// `discovery.Interval`.
pub const INTERVAL: Duration = Duration::from_secs(3);
/// `maxRetryBackoff`.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(300);

/// `resolveCustomResourceConfig`: the configuration text, `None` when there
/// is none (or the file is missing and that is allowed).
pub fn resolve_config(opts: &Options) -> Result<Option<String>, String> {
    if !opts.custom_resource_config.is_empty() {
        return Ok(Some(opts.custom_resource_config.clone()));
    }
    let file = &opts.custom_resource_config_file;
    if file.is_empty() {
        return Ok(None);
    }
    let path = crate::server::clean_path(file);
    match std::fs::read(&path) {
        Ok(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
        Err(e) => {
            let msg = crate::server::read_file_error(&path, &e);
            if opts.continue_without_custom_resource_config_file
                && e.kind() == std::io::ErrorKind::NotFound
            {
                tracing::warn!(
                    "Failed to open Custom Resource State Metrics file {file}: {msg}, ignoring"
                );
                return Ok(None);
            }
            Err(format!(
                "unable to open Custom Resource State Metrics file: {msg}"
            ))
        }
    }
}

/// `GroupVersionResource.String()`.
pub fn gvr_string(group: &str, version: &str, resource: &str) -> String {
    format!("{group}/{version}, Resource={resource}")
}

/// One resolved custom resource (Go: a `customResourceMetrics` factory),
/// keyed by its GVR string.
pub struct Factory {
    pub gvr: String,
    pub gvk: String,
    pub store: CustomResourceStore,
}

/// `FromConfig`'s factory generator: resolves every configured resource
/// against the discovery cache.
pub fn factories(
    resources: &[ksm_crs::Resource],
    discoverer: &CrDiscoverer,
) -> Result<Vec<Factory>, String> {
    let mut resolved = Vec::new();
    for r in resources {
        let gvk = r.gvk();
        match discoverer.resolve(&gvk.group, &gvk.version, &gvk.kind) {
            Err(e) => {
                tracing::error!(error = %e, gvk = %gvk, "failed to resolve GVK");
                continue;
            }
            Ok(set) => {
                if set.is_empty() {
                    tracing::info!(gvk = %gvk, "no matching CRD found for configured GVK, skipping");
                }
                for g in set {
                    resolved.push(r.resolved(
                        ksm_crs::GroupVersionKind {
                            group: g.group,
                            version: g.version,
                            kind: g.kind,
                        },
                        g.plural,
                    ));
                }
            }
        }
    }
    let mut out: Vec<Factory> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for r in resolved {
        let gvk = r.gvk().clone();
        r.family_names()
            .map_err(|e| format!("failed to create metrics factory for {gvk}: {e}"))?;
        let plural = r.get_resource_name();
        let gvr = gvr_string(&gvk.group, &gvk.version, &plural);
        if !seen.insert(gvr.clone()) {
            tracing::info!(GVR = %gvr, "reloaded factory");
        }
        let api_version = if gvk.group.is_empty() {
            gvk.version.clone()
        } else {
            format!("{}/{}", gvk.group, gvk.version)
        };
        out.push(Factory {
            gvr,
            gvk: gvk_key(&gvk.group, &gvk.version, &gvk.kind),
            store: CustomResourceStore {
                api_resource: ApiResource {
                    group: gvk.group.clone(),
                    version: gvk.version.clone(),
                    api_version,
                    kind: gvk.kind.clone(),
                    plural,
                },
                resource: r,
            },
        });
    }
    Ok(out)
}

/// `retryBackoff`.
fn retry_backoff(failures: u32) -> Duration {
    let mut b = INTERVAL;
    let mut i = 1;
    while i < failures && b < MAX_RETRY_BACKOFF {
        b *= 2;
        i += 1;
    }
    b.min(MAX_RETRY_BACKOFF)
}

/// Applies a new set of custom resource stores (Go: `ConfigureStore`).
pub trait StoreConfigurer: Send + Sync {
    fn replace_custom_resources(&self, stores: BTreeMap<String, Arc<CustomResourceStore>>);
}

/// `cacheReconciler`.
struct Reconciler {
    resources: Vec<ksm_crs::Resource>,
    discoverer: Arc<CrDiscoverer>,
    store: Arc<dyn StoreConfigurer>,
    /// Go's `lastApplied`: nil and empty compare equal (`maps.Equal`), so the
    /// first reconcile that resolves nothing changes nothing.
    last_applied: HashMap<String, Option<u64>>,
    failures: u32,
    failed_revision: u64,
    next_retry: Instant,
}

impl Reconciler {
    fn reconcile(&mut self) {
        let Some((revision, stops)) = self.discoverer.observe() else {
            return;
        };
        if self.failures > 0 && revision == self.failed_revision && Instant::now() < self.next_retry
        {
            return;
        }
        if revision != self.failed_revision {
            self.failures = 0;
        }
        match self.generate(revision, &stops) {
            Err(e) => {
                self.failures += 1;
                self.failed_revision = revision;
                let backoff = retry_backoff(self.failures);
                self.next_retry = Instant::now() + backoff;
                tracing::error!(error = %e, retryIn = ?backoff, consecutiveFailures = self.failures, "failed to update custom resource stores");
            }
            Ok(applied) => {
                self.failures = 0;
                if applied {
                    tracing::info!("discovery finished, cache updated");
                }
            }
        }
    }

    fn generate(
        &mut self,
        revision: u64,
        stops: &HashMap<String, crate::discovery::StopHandle>,
    ) -> Result<bool, String> {
        let factories = factories(&self.resources, &self.discoverer)?;
        let resolved: HashMap<String, Option<u64>> = factories
            .iter()
            .map(|f| (f.gvr.clone(), stops.get(&f.gvk).map(|h| h.id)))
            .collect();
        if self.last_applied == resolved {
            self.discoverer.applied(revision);
            tracing::debug!(
                "discovery cache changed but enabled custom resources are unchanged; skipping store rebuild"
            );
            return Ok(false);
        }
        let stores: BTreeMap<String, Arc<CustomResourceStore>> = factories
            .into_iter()
            .map(|f| (f.gvr, Arc::new(f.store)))
            .collect();
        self.store.replace_custom_resources(stores);
        self.last_applied = resolved;
        self.discoverer.applied(revision);
        Ok(true)
    }
}

/// `PollForCacheUpdates`: every [`INTERVAL`], rebuilds the custom resource
/// stores when the discovery cache changed, until `cancel` fires.
pub fn poll_for_cache_updates(
    resources: Vec<ksm_crs::Resource>,
    discoverer: Arc<CrDiscoverer>,
    store: Arc<dyn StoreConfigurer>,
    cancel: CancellationToken,
) {
    let mut r = Reconciler {
        resources,
        discoverer,
        store,
        last_applied: HashMap::new(),
        failures: 0,
        failed_revision: 0,
        next_retry: Instant::now(),
    };
    tokio::spawn(async move {
        let mut t = tokio::time::interval_at(tokio::time::Instant::now() + INTERVAL, INTERVAL);
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    tracing::info!("context cancelled");
                    return;
                }
                _ = t.tick() => r.reconcile(),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff() {
        assert_eq!(retry_backoff(1), Duration::from_secs(3));
        assert_eq!(retry_backoff(2), Duration::from_secs(6));
        assert_eq!(retry_backoff(4), Duration::from_secs(24));
        assert_eq!(retry_backoff(30), MAX_RETRY_BACKOFF);
    }

    #[test]
    fn gvr() {
        assert_eq!(
            gvr_string("myteam.io", "v1", "foos"),
            "myteam.io/v1, Resource=foos"
        );
    }
}
