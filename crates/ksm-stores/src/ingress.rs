//! Port of `internal/store/ingress.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::networking::v1::Ingress;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "ingresses";
pub const GO_TYPE: &str = "*v1.Ingress";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<Ingress>> {
    Vec::new()
}
