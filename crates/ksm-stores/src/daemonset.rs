//! Port of `internal/store/daemonset.go`.

use k8s_openapi::api::apps::v1::{DaemonSet, DaemonSetStatus};
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "daemonsets";
pub const GO_TYPE: &str = "*v1.DaemonSet";

/// A status count, 0 when unset (a plain `int32` in Go).
fn count(d: &DaemonSet, f: fn(&DaemonSetStatus) -> Option<i32>) -> Vec<Metric<'_>> {
    vec![Metric::value(f64::from(
        d.status.as_ref().and_then(f).unwrap_or(0),
    ))]
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<DaemonSet>> {
    let mut f = Families::new(&["namespace", "daemonset"], |d: &DaemonSet| {
        vec![ns(&d.metadata), name(&d.metadata)]
    });
    f.add(
        "kube_daemonset_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |d| created_metric(&d.metadata),
    )
    .add(
        "kube_daemonset_status_current_number_scheduled",
        "The number of nodes running at least one daemon pod and are supposed to.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| Some(s.current_number_scheduled)),
    )
    .add(
        "kube_daemonset_status_desired_number_scheduled",
        "The number of nodes that should be running the daemon pod.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| Some(s.desired_number_scheduled)),
    )
    .add(
        "kube_daemonset_status_number_available",
        "The number of nodes that should be running the daemon pod and have one or more of the daemon pod running and available",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.number_available),
    )
    .add(
        "kube_daemonset_status_number_misscheduled",
        "The number of nodes running a daemon pod but are not supposed to.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| Some(s.number_misscheduled)),
    )
    .add(
        "kube_daemonset_status_number_ready",
        "The number of nodes that should be running the daemon pod and have one or more of the daemon pod running and ready.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| Some(s.number_ready)),
    )
    .add(
        "kube_daemonset_status_number_unavailable",
        "The number of nodes that should be running the daemon pod and have none of the daemon pod running and available",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.number_unavailable),
    )
    .add(
        "kube_daemonset_status_observed_generation",
        "The most recent generation observed by the daemon set controller.",
        Gauge,
        Stable,
        "",
        |d| {
            let g = d.status.as_ref().and_then(|s| s.observed_generation);
            vec![Metric::value(i64_f64(g.unwrap_or(0)))]
        },
    )
    .add(
        "kube_daemonset_status_updated_number_scheduled",
        "The total number of nodes that are running updated daemon pod",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.updated_number_scheduled),
    )
    .add(
        "kube_daemonset_metadata_generation",
        "Sequence number representing a specific generation of the desired state.",
        Gauge,
        Stable,
        "",
        |d| generation_metric(&d.metadata),
    )
    .add(
        "kube_daemonset_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |d| deletion_metric(&d.metadata),
    )
    .annotations(
        "kube_daemonset_annotations",
        Alpha,
        allow_annotations,
        |d| &d.metadata,
    )
    .labels("kube_daemonset_labels", Stable, allow_labels, |d| {
        &d.metadata
    });
    f.build()
}
