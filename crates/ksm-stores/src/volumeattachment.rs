//! Port of `internal/store/volumeattachment.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::storage::v1::VolumeAttachment;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "volumeattachments";
pub const GO_TYPE: &str = "*v1.VolumeAttachment";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<VolumeAttachment>> {
    Vec::new()
}
