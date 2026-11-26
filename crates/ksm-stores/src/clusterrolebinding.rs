//! Port of `internal/store/clusterrolebinding.go`.

use k8s_openapi::api::rbac::v1::ClusterRoleBinding;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "clusterrolebindings";
pub const GO_TYPE: &str = "*v1.ClusterRoleBinding";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<ClusterRoleBinding>> {
    let mut f = Families::new(&["clusterrolebinding"], |r: &ClusterRoleBinding| {
        vec![name(&r.metadata)]
    });
    f.annotations(
        "kube_clusterrolebinding_annotations",
        Alpha,
        allow_annotations,
        |r| &r.metadata,
    )
    .labels("kube_clusterrolebinding_labels", Alpha, allow_labels, |r| {
        &r.metadata
    })
    .add(
        "kube_clusterrolebinding_info",
        "Information about clusterrolebinding.",
        Gauge,
        Alpha,
        "",
        |r| {
            vec![mk(
                &["roleref_kind", "roleref_name"],
                vec![r.role_ref.kind.clone(), r.role_ref.name.clone()],
                1.0,
            )]
        },
    )
    .add(
        "kube_clusterrolebinding_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |r| created_metric(&r.metadata),
    )
    .add(
        "kube_clusterrolebinding_metadata_resource_version",
        "Resource version representing a specific version of the clusterrolebinding.",
        Gauge,
        Alpha,
        "",
        |r| rv_metric(&r.metadata),
    );
    f.build()
}
