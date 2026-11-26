//! Port of `internal/store/replicaset.go`.

use k8s_openapi::api::apps::v1::{ReplicaSet, ReplicaSetStatus};
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "replicasets";
pub const GO_TYPE: &str = "*v1.ReplicaSet";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

fn status(r: &ReplicaSet) -> Option<&ReplicaSetStatus> {
    r.status.as_ref()
}

/// A status count, 0 when unset (a plain `int32` in Go).
fn count(r: &ReplicaSet, f: fn(&ReplicaSetStatus) -> Option<i32>) -> Vec<Metric> {
    vec![Metric::value(f64::from(status(r).and_then(f).unwrap_or(0)))]
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<ReplicaSet>> {
    let mut f = Families::new(&["namespace", "replicaset"], |r: &ReplicaSet| {
        vec![ns(&r.metadata), name(&r.metadata)]
    });
    f.add(
        "kube_replicaset_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |r| created_metric(&r.metadata),
    )
    .add(
        "kube_replicaset_status_replicas",
        "The number of replicas per ReplicaSet.",
        Gauge,
        Stable,
        "",
        |r| count(r, |s| Some(s.replicas)),
    )
    .add(
        "kube_replicaset_status_fully_labeled_replicas",
        "The number of fully labeled replicas per ReplicaSet.",
        Gauge,
        Stable,
        "",
        |r| count(r, |s| s.fully_labeled_replicas),
    )
    .add(
        "kube_replicaset_status_ready_replicas",
        "The number of ready replicas per ReplicaSet.",
        Gauge,
        Stable,
        "",
        |r| count(r, |s| s.ready_replicas),
    )
    .add(
        "kube_replicaset_status_terminating_replicas",
        "The number of terminating replicas per ReplicaSet.",
        Gauge,
        Alpha,
        "",
        |r| opt_i32_metric(status(r).and_then(|s| s.terminating_replicas)),
    )
    .add(
        "kube_replicaset_status_observed_generation",
        "The generation observed by the ReplicaSet controller.",
        Gauge,
        Stable,
        "",
        |r| {
            let g = status(r).and_then(|s| s.observed_generation).unwrap_or(0);
            vec![Metric::value(i64_f64(g))]
        },
    )
    .add(
        "kube_replicaset_spec_replicas",
        "Number of desired pods for a ReplicaSet.",
        Gauge,
        Stable,
        "",
        |r| opt_i32_metric(r.spec.replicas),
    )
    .add(
        "kube_replicaset_metadata_generation",
        "Sequence number representing a specific generation of the desired state.",
        Gauge,
        Stable,
        "",
        |r| vec![Metric::value(i64_f64(r.metadata.generation.unwrap_or(0)))],
    )
    .add(
        "kube_replicaset_owner",
        "Information about the ReplicaSet's owner.",
        Gauge,
        Stable,
        "",
        |r| owner_metrics(&r.metadata, true),
    )
    .annotations(
        "kube_replicaset_annotations",
        Alpha,
        allow_annotations,
        |r| &r.metadata,
    )
    .labels("kube_replicaset_labels", Stable, allow_labels, |r| {
        &r.metadata
    });
    f.build()
}
