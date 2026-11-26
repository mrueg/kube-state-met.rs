//! Port of `internal/store/clusterrolebinding.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::rbac::v1::ClusterRoleBinding;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "clusterrolebindings";
pub const GO_TYPE: &str = "*v1.ClusterRoleBinding";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<ClusterRoleBinding>> {
    Vec::new()
}
