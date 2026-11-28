//! Port of `internal/store/mutatingadmissionpolicybinding.go`.

use k8s_openapi::api::admissionregistration::v1::MutatingAdmissionPolicyBinding;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "mutatingadmissionpolicybindings";
pub const GO_TYPE: &str = "*v1.MutatingAdmissionPolicyBinding";

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<MutatingAdmissionPolicyBinding>> {
    let mut f = Families::new(
        &["mutatingadmissionpolicybinding"],
        |b: &MutatingAdmissionPolicyBinding| vec![name(&b.metadata)],
    );
    f.add(
        "kube_mutatingadmissionpolicybinding_info",
        "Information about the MutatingAdmissionPolicyBinding.",
        Gauge,
        Alpha,
        "",
        |b| {
            let spec = b.spec.as_ref();
            let param_ref = spec.and_then(|sp| sp.param_ref.as_ref());
            vec![mk(
                &[
                    "policy_name",
                    "param_name",
                    "param_namespace",
                    "param_not_found_action",
                ],
                values![
                    spec.map_or("", |sp| s(&sp.policy_name)),
                    param_ref.map_or("", |r| s(&r.name)),
                    param_ref.map_or("", |r| s(&r.namespace)),
                    param_ref.map_or("", |r| s(&r.parameter_not_found_action)),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_mutatingadmissionpolicybinding_created",
        "Unix creation timestamp.",
        Gauge,
        Alpha,
        "",
        |b| created_metric(&b.metadata),
    );
    f.build()
}
