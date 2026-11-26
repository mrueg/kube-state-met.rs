//! Port of `internal/store/validatingadmissionpolicybinding.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::admissionregistration::v1::ValidatingAdmissionPolicyBinding;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "validatingadmissionpolicybindings";
pub const GO_TYPE: &str = "*v1.ValidatingAdmissionPolicyBinding";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<ValidatingAdmissionPolicyBinding>> {
    Vec::new()
}
