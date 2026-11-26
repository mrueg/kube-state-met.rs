//! Port of `internal/store/node.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::core::v1::Node;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "nodes";
pub const GO_TYPE: &str = "*v1.Node";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<Node>> {
    Vec::new()
}
