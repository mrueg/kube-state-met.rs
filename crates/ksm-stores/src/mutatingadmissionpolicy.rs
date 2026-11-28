//! Port of `internal/store/mutatingadmissionpolicy.go`.

use k8s_openapi::api::admissionregistration::v1::MutatingAdmissionPolicy;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "mutatingadmissionpolicies";
pub const GO_TYPE: &str = "*v1.MutatingAdmissionPolicy";

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<MutatingAdmissionPolicy>> {
    let mut f = Families::new(
        &["mutatingadmissionpolicy"],
        |p: &MutatingAdmissionPolicy| vec![name(&p.metadata)],
    );
    f.add(
        "kube_mutatingadmissionpolicy_info",
        "Information about the MutatingAdmissionPolicy.",
        Gauge,
        Alpha,
        "",
        |p| {
            let spec = p.spec.as_ref();
            let param_kind = spec.and_then(|s| s.param_kind.as_ref());
            vec![mk(
                &[
                    "param_api_version",
                    "param_kind",
                    "failure_policy",
                    "reinvocation_policy",
                ],
                values![
                    param_kind.map_or("", |k| s(&k.api_version)),
                    param_kind.map_or("", |k| s(&k.kind)),
                    spec.map_or("", |sp| s(&sp.failure_policy)),
                    spec.map_or("", |sp| s(&sp.reinvocation_policy)),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_mutatingadmissionpolicy_created",
        "Unix creation timestamp.",
        Gauge,
        Alpha,
        "",
        |p| created_metric(&p.metadata),
    );
    f.build()
}
