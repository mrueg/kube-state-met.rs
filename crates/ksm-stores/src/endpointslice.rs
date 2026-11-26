//! Port of `internal/store/endpointslice.go`. NOT PORTED YET: generates no families.

use k8s_openapi::api::discovery::v1::EndpointSlice;
use ksm_core::FamilyGenerator;

pub const NAME: &str = "endpointslices";
pub const GO_TYPE: &str = "*v1.EndpointSlice";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = false;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<EndpointSlice>> {
    Vec::new()
}
