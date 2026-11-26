//! Port of `internal/store/persistentvolumeclaim.go`.

use k8s_openapi::api::core::v1::{
    PersistentVolumeClaim, PersistentVolumeClaimSpec, PersistentVolumeClaimStatus,
};
use ksm_core::{FamilyGenerator, Metric};

use crate::quantity;
use crate::util::*;

pub const NAME: &str = "persistentvolumeclaims";
pub const GO_TYPE: &str = "*v1.PersistentVolumeClaim";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

/// `v1.BetaStorageClassAnnotation`.
const BETA_STORAGE_CLASS_ANNOTATION: &str = "volume.beta.kubernetes.io/storage-class";

fn spec(p: &PersistentVolumeClaim) -> Option<&PersistentVolumeClaimSpec> {
    p.spec.as_ref()
}

fn status(p: &PersistentVolumeClaim) -> Option<&PersistentVolumeClaimStatus> {
    p.status.as_ref()
}

/// `getPersistentVolumeClaimClass`: the storage class name, `""` if none was
/// requested.
fn claim_class(p: &PersistentVolumeClaim) -> String {
    // Use beta annotation first
    if let Some(class) = p
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(BETA_STORAGE_CLASS_ANNOTATION))
    {
        return class.clone();
    }
    spec(p)
        .and_then(|sp| sp.storage_class_name.clone())
        .unwrap_or_default()
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<PersistentVolumeClaim>> {
    let mut f = Families::new(
        &["namespace", "persistentvolumeclaim"],
        |p: &PersistentVolumeClaim| vec![ns(&p.metadata), name(&p.metadata)],
    );
    f.labels(
        "kube_persistentvolumeclaim_labels",
        Stable,
        allow_labels,
        |p| &p.metadata,
    )
    .annotations(
        "kube_persistentvolumeclaim_annotations",
        Alpha,
        allow_annotations,
        |p| &p.metadata,
    )
    .add(
        "kube_persistentvolumeclaim_info",
        "Information about persistent volume claim.",
        Gauge,
        Stable,
        "",
        |p| {
            vec![mk(
                &["storageclass", "volumename", "volumemode"],
                vec![
                    claim_class(p),
                    spec(p).map(|sp| s(&sp.volume_name)).unwrap_or("").to_owned(),
                    spec(p).map(|sp| s(&sp.volume_mode)).unwrap_or("").to_owned(),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_persistentvolumeclaim_volume_attributes_class",
        "The name of the VolumeAttributesClass requested by the persistent volume claim.",
        Gauge,
        Alpha,
        "",
        |p| {
            spec(p)
                .and_then(|sp| sp.volume_attributes_class_name.as_ref())
                .map(|c| m1("volume_attributes_class", c.as_str(), 1.0))
                .into_iter()
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_status_current_volume_attributes_class",
        "The current VolumeAttributesClass applied to the persistent volume claim, as reported by the CSI driver.",
        Gauge,
        Alpha,
        "",
        |p| {
            status(p)
                .and_then(|st| st.current_volume_attributes_class_name.as_ref())
                .map(|c| m1("volume_attributes_class", c.as_str(), 1.0))
                .into_iter()
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_status_modify_volume_status",
        "Information about the status of an in-progress ModifyVolume operation on the persistent volume claim.",
        Gauge,
        Alpha,
        "",
        |p| {
            let Some(mvs) = status(p).and_then(|st| st.modify_volume_status.as_ref()) else {
                return Vec::new();
            };
            let target = s(&mvs.target_volume_attributes_class_name);
            ["Pending", "InProgress", "Infeasible"]
                .iter()
                .map(|st| {
                    mk(
                        &["status", "target_volume_attributes_class"],
                        vec![(*st).to_owned(), target.to_owned()],
                        bool_f64(mvs.status == *st),
                    )
                })
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_status_phase",
        "The phase the persistent volume claim is currently in.",
        Gauge,
        Stable,
        "",
        |p| {
            let phase = status(p).map(|st| s(&st.phase)).unwrap_or("");
            if phase.is_empty() {
                return Vec::new();
            }
            ["Lost", "Bound", "Pending"]
                .iter()
                .map(|ph| m1("phase", *ph, bool_f64(phase == *ph)))
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_resource_requests_storage_bytes",
        "The capacity of storage requested by the persistent volume claim.",
        Gauge,
        Stable,
        "",
        |p| {
            spec(p)
                .and_then(|sp| sp.resources.as_ref())
                .and_then(|r| r.requests.as_ref())
                .and_then(|r| r.get("storage"))
                .map(|q| Metric::value(quantity::value_f64(q)))
                .into_iter()
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_access_mode",
        "The access mode(s) specified by the persistent volume claim.",
        Gauge,
        Stable,
        "",
        |p| {
            spec(p)
                .and_then(|sp| sp.access_modes.as_ref())
                .into_iter()
                .flatten()
                .map(|m| m1("access_mode", m.as_str(), 1.0))
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_status_condition",
        "Information about status of different conditions of persistent volume claim.",
        Gauge,
        Alpha,
        "",
        |p| {
            status(p)
                .and_then(|st| st.conditions.as_ref())
                .into_iter()
                .flatten()
                .flat_map(|c| conditions_with(&["condition"], &[&c.type_], &c.status))
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_status_condition_last_transition_timestamp_seconds",
        "Unix timestamp of last transition of a persistent volume claim condition.",
        Gauge,
        Alpha,
        "",
        |p| {
            status(p)
                .and_then(|st| st.conditions.as_ref())
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let t = c.last_transition_time.as_ref()?;
                    if time_is_zero(Some(t)) {
                        return None;
                    }
                    Some(mk(
                        &["condition", "status"],
                        vec![c.type_.clone(), c.status.to_lowercase()],
                        time_f64(t),
                    ))
                })
                .collect()
        },
    )
    .add(
        "kube_persistentvolumeclaim_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |p| created_metric(&p.metadata),
    )
    .add(
        "kube_persistentvolumeclaim_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |p| deletion_metric(&p.metadata),
    );
    f.build()
}
