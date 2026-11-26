//! Port of `internal/store/mutatingwebhookconfiguration.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::admissionregistration::v1::MutatingWebhookConfiguration;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "mutatingwebhookconfigurations";
pub const GO_TYPE: &str = "*v1.MutatingWebhookConfiguration";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<MutatingWebhookConfiguration>> {
    Vec::new()
}
