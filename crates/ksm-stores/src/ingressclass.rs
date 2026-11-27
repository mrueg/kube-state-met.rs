//! Port of `internal/store/ingressclass.go`.

use k8s_openapi::api::networking::v1::IngressClass;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "ingressclasses";
pub const GO_TYPE: &str = "*v1.IngressClass";

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<IngressClass>> {
    let mut f = Families::new(&["ingressclass"], |o: &IngressClass| {
        vec![name(&o.metadata)]
    });
    f.add(
        "kube_ingressclass_info",
        "Information about ingressclass.",
        Gauge,
        Alpha,
        "",
        |o| {
            let controller = o
                .spec
                .as_ref()
                .and_then(|sp| sp.controller.as_deref())
                .unwrap_or("");
            vec![m1("controller", controller, 1.0)]
        },
    )
    .add(
        "kube_ingressclass_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |o| created_metric(&o.metadata),
    )
    .annotations(
        "kube_ingressclass_annotations",
        Alpha,
        allow_annotations,
        |o| &o.metadata,
    )
    .labels("kube_ingressclass_labels", Alpha, allow_labels, |o| {
        &o.metadata
    });
    f.build()
}
