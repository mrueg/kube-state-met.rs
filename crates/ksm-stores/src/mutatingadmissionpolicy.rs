//! Port of `internal/store/mutatingadmissionpolicy.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::admissionregistration::v1::MutatingAdmissionPolicy;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "mutatingadmissionpolicies";
pub const GO_TYPE: &str = "*v1.MutatingAdmissionPolicy";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<MutatingAdmissionPolicy>> {
    Vec::new()
}
