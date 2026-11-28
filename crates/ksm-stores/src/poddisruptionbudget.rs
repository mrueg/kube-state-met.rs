//! Port of `internal/store/poddisruptionbudget.go`.

use k8s_openapi::api::policy::v1::{PodDisruptionBudget, PodDisruptionBudgetStatus};
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "poddisruptionbudgets";
pub const GO_TYPE: &str = "*v1.PodDisruptionBudget";

/// A status field as one sample; Go reads the zero value of a missing status.
fn status_metric(
    p: &PodDisruptionBudget,
    f: fn(&PodDisruptionBudgetStatus) -> f64,
) -> Vec<Metric<'_>> {
    vec![Metric::value(p.status.as_ref().map_or(0.0, f))]
}

fn observed_generation(st: &PodDisruptionBudgetStatus) -> f64 {
    i64_f64(st.observed_generation.unwrap_or(0))
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<PodDisruptionBudget>> {
    let mut f = Families::new(
        &["namespace", "poddisruptionbudget"],
        |o: &PodDisruptionBudget| vec![ns(&o.metadata), name(&o.metadata)],
    );
    f.annotations(
        "kube_poddisruptionbudget_annotations",
        Alpha,
        allow_annotations,
        |o| &o.metadata,
    )
    .labels(
        "kube_poddisruptionbudget_labels",
        Alpha,
        allow_labels,
        |o| &o.metadata,
    )
    .add(
        "kube_poddisruptionbudget_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |o| created_metric(&o.metadata),
    )
    .add(
        "kube_poddisruptionbudget_status_current_healthy",
        "Current number of healthy pods",
        Gauge,
        Stable,
        "",
        |o| status_metric(o, |st| f64::from(st.current_healthy.unwrap_or(0))),
    )
    .add(
        "kube_poddisruptionbudget_status_desired_healthy",
        "Minimum desired number of healthy pods",
        Gauge,
        Stable,
        "",
        |o| status_metric(o, |st| f64::from(st.desired_healthy.unwrap_or(0))),
    )
    .add(
        "kube_poddisruptionbudget_status_pod_disruptions_allowed",
        "Number of pod disruptions that are currently allowed",
        Gauge,
        Stable,
        "",
        |o| status_metric(o, |st| f64::from(st.disruptions_allowed.unwrap_or(0))),
    )
    .add(
        "kube_poddisruptionbudget_status_expected_pods",
        "Total number of pods counted by this disruption budget",
        Gauge,
        Stable,
        "",
        |o| status_metric(o, |st| f64::from(st.expected_pods.unwrap_or(0))),
    )
    .add(
        "kube_poddisruptionbudget_status_observed_generation",
        "Most recent generation observed when updating this PDB status",
        Gauge,
        Stable,
        "",
        |o| status_metric(o, observed_generation),
    )
    .add(
        "kube_poddisruptionbudget_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |o| deletion_metric(&o.metadata),
    );
    f.build()
}
