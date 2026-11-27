//! Port of `internal/store/role.go`.

use k8s_openapi::api::rbac::v1::Role;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "roles";
pub const GO_TYPE: &str = "*v1.Role";

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Role>> {
    let mut f = Families::new(&["namespace", "role"], |r: &Role| {
        vec![ns(&r.metadata), name(&r.metadata)]
    });
    f.annotations("kube_role_annotations", Alpha, allow_annotations, |r| {
        &r.metadata
    })
    .labels("kube_role_labels", Alpha, allow_labels, |r| &r.metadata)
    .add(
        "kube_role_info",
        "Information about role.",
        Gauge,
        Alpha,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_role_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |r| created_metric(&r.metadata),
    )
    .add(
        "kube_role_metadata_resource_version",
        "Resource version representing a specific version of the role.",
        Gauge,
        Alpha,
        "",
        |r| rv_metric(&r.metadata),
    );
    f.build()
}
