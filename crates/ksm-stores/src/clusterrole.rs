//! Port of `internal/store/clusterrole.go`.

use k8s_openapi::api::rbac::v1::ClusterRole;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "clusterroles";
pub const GO_TYPE: &str = "*v1.ClusterRole";

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<ClusterRole>> {
    let mut f = Families::new(&["clusterrole"], |r: &ClusterRole| vec![name(&r.metadata)]);
    f.annotations(
        "kube_clusterrole_annotations",
        Alpha,
        allow_annotations,
        |r| &r.metadata,
    )
    .labels("kube_clusterrole_labels", Alpha, allow_labels, |r| {
        &r.metadata
    })
    .add(
        "kube_clusterrole_info",
        "Information about cluster role.",
        Gauge,
        Alpha,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_clusterrole_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |r| created_metric(&r.metadata),
    )
    .add(
        "kube_clusterrole_metadata_resource_version",
        "Resource version representing a specific version of the cluster role.",
        Gauge,
        Alpha,
        "",
        |r| rv_metric(&r.metadata),
    );
    f.build()
}
