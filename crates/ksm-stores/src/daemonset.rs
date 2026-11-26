//! Port of `internal/store/daemonset.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::apps::v1::DaemonSet;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "daemonsets";
pub const GO_TYPE: &str = "*v1.DaemonSet";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<DaemonSet>> {
    Vec::new()
}
