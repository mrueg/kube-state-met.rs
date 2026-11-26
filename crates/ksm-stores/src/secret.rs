//! Port of `internal/store/secret.go`.

use k8s_openapi::api::core::v1::Secret;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "secrets";
pub const GO_TYPE: &str = "*v1.Secret";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Secret>> {
    let mut f = Families::new(&["namespace", "secret"], |o: &Secret| {
        vec![ns(&o.metadata), name(&o.metadata)]
    });
    f.add(
        "kube_secret_info",
        "Information about secret.",
        Gauge,
        Stable,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_secret_type",
        "Type about secret.",
        Gauge,
        Stable,
        "",
        |o| vec![m1("type", s(&o.type_), 1.0)],
    )
    .annotations("kube_secret_annotations", Alpha, allow_annotations, |o| {
        &o.metadata
    })
    .labels("kube_secret_labels", Stable, allow_labels, |o| &o.metadata)
    .add(
        "kube_secret_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |o| created_metric(&o.metadata),
    )
    .add(
        "kube_secret_metadata_resource_version",
        "Resource version representing a specific version of secret.",
        Gauge,
        Alpha,
        "",
        |o| rv_metric(&o.metadata),
    )
    .add(
        "kube_secret_owner",
        "Information about the Secret's owner.",
        Gauge,
        Alpha,
        "",
        |o| owner_metrics(&o.metadata),
    );
    f.build()
}
