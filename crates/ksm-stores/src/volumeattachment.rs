//! Port of `internal/store/volumeattachment.go`.

use k8s_openapi::api::storage::v1::VolumeAttachment;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "volumeattachments";
pub const GO_TYPE: &str = "*v1.VolumeAttachment";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

/// Go's `volumeAttachmentMetricFamilies` takes no allow lists: the labels
/// family always exposes every label.
pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<VolumeAttachment>> {
    let mut f = Families::new(&["volumeattachment"], |va: &VolumeAttachment| {
        vec![name(&va.metadata)]
    });
    f.add(
        "kube_volumeattachment_labels",
        "Kubernetes labels converted to Prometheus labels.",
        Gauge,
        Alpha,
        "",
        |va| {
            let (k, v) = map_to_prometheus_labels(va.metadata.labels.iter().flatten(), "label");
            vec![Metric::new(k, v, 1.0)]
        },
    )
    .add(
        "kube_volumeattachment_info",
        "Information about volumeattachment.",
        Gauge,
        Alpha,
        "",
        |va| {
            vec![mk(
                &["attacher", "node"],
                vec![va.spec.attacher.clone(), va.spec.node_name.clone()],
                1.0,
            )]
        },
    )
    .add(
        "kube_volumeattachment_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |va| created_metric(&va.metadata),
    )
    .add(
        "kube_volumeattachment_spec_source_persistentvolume",
        "PersistentVolume source reference.",
        Gauge,
        Alpha,
        "",
        |va| match &va.spec.source.persistent_volume_name {
            Some(pv) => vec![m1("volumename", pv.clone(), 1.0)],
            None => Vec::new(),
        },
    )
    .add(
        "kube_volumeattachment_status_attached",
        "Information about volumeattachment.",
        Gauge,
        Alpha,
        "",
        |va| {
            vec![Metric::value(bool_f64(
                va.status.as_ref().is_some_and(|s| s.attached),
            ))]
        },
    )
    .add(
        "kube_volumeattachment_status_attachment_metadata",
        "volumeattachment metadata.",
        Gauge,
        Alpha,
        "",
        |va| {
            let md = va
                .status
                .as_ref()
                .and_then(|s| s.attachment_metadata.as_ref());
            let (k, v) = map_to_prometheus_labels(md.into_iter().flatten(), "metadata");
            vec![Metric::new(k, v, 1.0)]
        },
    );
    f.build()
}
