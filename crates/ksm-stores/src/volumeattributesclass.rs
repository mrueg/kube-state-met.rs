//! Port of `internal/store/volumeattributesclass.go`.

use k8s_openapi::api::storage::v1::VolumeAttributesClass;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "volumeattributesclasses";
pub const GO_TYPE: &str = "*v1.VolumeAttributesClass";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<VolumeAttributesClass>> {
    let mut f = Families::new(&["volumeattributesclass"], |v: &VolumeAttributesClass| {
        vec![name(&v.metadata)]
    });
    f.add(
        "kube_volumeattributesclass_info",
        "Information about volumeattributesclass.",
        Gauge,
        Alpha,
        "",
        |v| vec![m1("driver_name", v.driver_name.clone(), 1.0)],
    )
    .add(
        "kube_volumeattributesclass_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |v| created_metric(&v.metadata),
    )
    .annotations(
        "kube_volumeattributesclass_annotations",
        Alpha,
        allow_annotations,
        |v| &v.metadata,
    )
    .labels(
        "kube_volumeattributesclass_labels",
        Alpha,
        allow_labels,
        |v| &v.metadata,
    );
    f.build()
}
