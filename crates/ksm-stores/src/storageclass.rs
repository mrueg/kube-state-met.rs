//! Port of `internal/store/storageclass.go`.

use k8s_openapi::api::storage::v1::StorageClass;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "storageclasses";
pub const GO_TYPE: &str = "*v1.StorageClass";

/// `v1.PersistentVolumeReclaimDelete`, used when the reclaim policy is unset.
const DEFAULT_RECLAIM_POLICY: &str = "Delete";
/// `storagev1.VolumeBindingImmediate`, used when the binding mode is unset.
const DEFAULT_VOLUME_BINDING_MODE: &str = "Immediate";

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<StorageClass>> {
    let mut f = Families::new(&["storageclass"], |s: &StorageClass| {
        vec![name(&s.metadata)]
    });
    f.add(
        "kube_storageclass_info",
        "Information about storageclass.",
        Gauge,
        Stable,
        "",
        |s| {
            vec![mk(
                &["provisioner", "reclaim_policy", "volume_binding_mode"],
                vec![
                    s.provisioner.clone(),
                    s.reclaim_policy
                        .as_deref()
                        .unwrap_or(DEFAULT_RECLAIM_POLICY)
                        .to_owned(),
                    s.volume_binding_mode
                        .as_deref()
                        .unwrap_or(DEFAULT_VOLUME_BINDING_MODE)
                        .to_owned(),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_storageclass_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |s| created_metric(&s.metadata),
    )
    .annotations(
        "kube_storageclass_annotations",
        Alpha,
        allow_annotations,
        |s| &s.metadata,
    )
    .labels("kube_storageclass_labels", Stable, allow_labels, |s| {
        &s.metadata
    });
    f.build()
}
