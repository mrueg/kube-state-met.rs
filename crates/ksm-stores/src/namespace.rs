//! Port of `internal/store/namespace.go`.

use k8s_openapi::api::core::v1::Namespace;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "namespaces";
pub const GO_TYPE: &str = "*v1.Namespace";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Namespace>> {
    let mut f = Families::new(&["namespace"], |o: &Namespace| vec![name(&o.metadata)]);
    f.add(
        "kube_namespace_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |o| created_metric(&o.metadata),
    )
    .annotations(
        "kube_namespace_annotations",
        Alpha,
        allow_annotations,
        |o| &o.metadata,
    )
    .labels("kube_namespace_labels", Stable, allow_labels, |o| {
        &o.metadata
    })
    .add(
        "kube_namespace_status_phase",
        "kubernetes namespace status phase.",
        Gauge,
        Stable,
        "",
        |o| {
            let phase = o
                .status
                .as_ref()
                .and_then(|st| st.phase.as_deref())
                .unwrap_or("");
            vec![
                m1("phase", "Active", bool_f64(phase == "Active")),
                m1("phase", "Terminating", bool_f64(phase == "Terminating")),
            ]
        },
    )
    .add(
        "kube_namespace_status_condition",
        "The condition of a namespace.",
        Gauge,
        Alpha,
        "",
        |o| {
            o.status
                .as_ref()
                .and_then(|st| st.conditions.as_ref())
                .into_iter()
                .flatten()
                .flat_map(|c| conditions_with(&["condition"], &[c.type_.as_str()], &c.status))
                .collect()
        },
    );
    f.build()
}
