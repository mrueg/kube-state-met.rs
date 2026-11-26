//! Port of `internal/store/certificatesigningrequest.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::certificates::v1::CertificateSigningRequest;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "certificatesigningrequests";
pub const GO_TYPE: &str = "*v1.CertificateSigningRequest";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<CertificateSigningRequest>> {
    Vec::new()
}
