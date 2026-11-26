//! Port of `internal/store/configmap.go`.

use k8s_openapi::api::core::v1::ConfigMap;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "configmaps";
pub const GO_TYPE: &str = "*v1.ConfigMap";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<ConfigMap>> {
    let mut f = Families::new(&["namespace", "configmap"], |c: &ConfigMap| {
        vec![ns(&c.metadata), name(&c.metadata)]
    });
    f.annotations(
        "kube_configmap_annotations",
        Alpha,
        allow_annotations,
        |c| &c.metadata,
    )
    .labels("kube_configmap_labels", Stable, allow_labels, |c| {
        &c.metadata
    })
    .add(
        "kube_configmap_info",
        "Information about configmap.",
        Gauge,
        Stable,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_configmap_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |c| created_metric(&c.metadata),
    )
    .add(
        "kube_configmap_metadata_resource_version",
        "Resource version representing a specific version of the configmap.",
        Gauge,
        Alpha,
        "",
        |c| rv_metric(&c.metadata),
    );
    f.build()
}
