//! Port of `internal/store/rolebinding.go`.

use k8s_openapi::api::rbac::v1::RoleBinding;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "rolebindings";
pub const GO_TYPE: &str = "*v1.RoleBinding";

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<RoleBinding>> {
    let mut f = Families::new(&["namespace", "rolebinding"], |r: &RoleBinding| {
        vec![ns(&r.metadata), name(&r.metadata)]
    });
    f.annotations(
        "kube_rolebinding_annotations",
        Alpha,
        allow_annotations,
        |r| &r.metadata,
    )
    .labels("kube_rolebinding_labels", Alpha, allow_labels, |r| {
        &r.metadata
    })
    .add(
        "kube_rolebinding_info",
        "Information about rolebinding.",
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
        "kube_rolebinding_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |r| created_metric(&r.metadata),
    )
    .add(
        "kube_rolebinding_metadata_resource_version",
        "Resource version representing a specific version of the rolebinding.",
        Gauge,
        Alpha,
        "",
        |r| rv_metric(&r.metadata),
    );
    f.build()
}
