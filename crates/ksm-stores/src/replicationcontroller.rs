//! Port of `internal/store/replicationcontroller.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::core::v1::ReplicationController;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "replicationcontrollers";
pub const GO_TYPE: &str = "*v1.ReplicationController";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<ReplicationController>> {
    Vec::new()
}
