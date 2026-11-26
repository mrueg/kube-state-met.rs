//! Port of `internal/store/validatingadmissionpolicybinding.go`.

use k8s_openapi::api::admissionregistration::v1::ValidatingAdmissionPolicyBinding;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "validatingadmissionpolicybindings";
pub const GO_TYPE: &str = "*v1.ValidatingAdmissionPolicyBinding";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<ValidatingAdmissionPolicyBinding>> {
    let mut f = Families::new(
        &["validatingadmissionpolicybinding"],
        |b: &ValidatingAdmissionPolicyBinding| vec![name(&b.metadata)],
    );
    f.add(
        "kube_validatingadmissionpolicybinding_info",
        "Information about the ValidatingAdmissionPolicyBinding.",
        Gauge,
        Alpha,
        "",
        |b| {
            let param_ref = b.spec.param_ref.as_ref();
            vec![mk(
                &[
                    "policy_name",
                    "param_name",
                    "param_namespace",
                    "param_not_found_action",
                ],
                vec![
                    b.spec.policy_name.clone(),
                    param_ref.map_or("", |r| s(&r.name)).to_owned(),
                    param_ref.map_or("", |r| s(&r.namespace)).to_owned(),
                    param_ref
                        .map_or("", |r| s(&r.parameter_not_found_action))
                        .to_owned(),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_validatingadmissionpolicybinding_created",
        "Unix creation timestamp.",
        Gauge,
        Alpha,
        "",
        |b| created_metric(&b.metadata),
    )
    .add(
        "kube_validatingadmissionpolicybinding_validation_action",
        "Validation actions for the ValidatingAdmissionPolicyBinding.",
        Gauge,
        Alpha,
        "",
        |b| {
            b.spec
                .validation_actions
                .iter()
                .map(|a| m1("action", a.clone(), 1.0))
                .collect()
        },
    );
    f.build()
}
