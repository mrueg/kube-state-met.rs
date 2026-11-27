//! Port of `internal/store/replicationcontroller.go`.

use k8s_openapi::api::core::v1::{ReplicationController, ReplicationControllerStatus};
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "replicationcontrollers";
pub const GO_TYPE: &str = "*v1.ReplicationController";

fn status(r: &ReplicationController) -> Option<&ReplicationControllerStatus> {
    r.status.as_ref()
}

/// A status count, 0 when unset (a plain `int32` in Go).
fn count(
    r: &ReplicationController,
    f: fn(&ReplicationControllerStatus) -> Option<i32>,
) -> Vec<Metric> {
    vec![Metric::value(f64::from(status(r).and_then(f).unwrap_or(0)))]
}

/// Go's families take no allow lists: replication controllers have no
/// `_annotations` or `_labels` family.
pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<ReplicationController>> {
    let mut f = Families::new(
        &["namespace", "replicationcontroller"],
        |r: &ReplicationController| vec![ns(&r.metadata), name(&r.metadata)],
    );
    f.add(
        "kube_replicationcontroller_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |r| created_metric(&r.metadata),
    )
    .add(
        "kube_replicationcontroller_status_replicas",
        "The number of replicas per ReplicationController.",
        Gauge,
        Stable,
        "",
        |r| count(r, |s| Some(s.replicas)),
    )
    .add(
        "kube_replicationcontroller_status_fully_labeled_replicas",
        "The number of fully labeled replicas per ReplicationController.",
        Gauge,
        Stable,
        "",
        |r| count(r, |s| s.fully_labeled_replicas),
    )
    .add(
        "kube_replicationcontroller_status_ready_replicas",
        "The number of ready replicas per ReplicationController.",
        Gauge,
        Stable,
        "",
        |r| count(r, |s| s.ready_replicas),
    )
    .add(
        "kube_replicationcontroller_status_available_replicas",
        "The number of available replicas per ReplicationController.",
        Gauge,
        Stable,
        "",
        |r| count(r, |s| s.available_replicas),
    )
    .add(
        "kube_replicationcontroller_status_observed_generation",
        "The generation observed by the ReplicationController controller.",
        Gauge,
        Stable,
        "",
        |r| {
            let g = status(r).and_then(|s| s.observed_generation).unwrap_or(0);
            vec![Metric::value(i64_f64(g))]
        },
    )
    .add(
        "kube_replicationcontroller_spec_replicas",
        "Number of desired pods for a ReplicationController.",
        Gauge,
        Stable,
        "",
        |r| opt_i32_metric(r.spec.as_ref().and_then(|s| s.replicas)),
    )
    .add(
        "kube_replicationcontroller_metadata_generation",
        "Sequence number representing a specific generation of the desired state.",
        Gauge,
        Stable,
        "",
        |r| generation_metric(&r.metadata),
    )
    .add(
        "kube_replicationcontroller_owner",
        "Information about the ReplicationController's owner.",
        Gauge,
        Alpha,
        "",
        |r| owner_metrics(&r.metadata, true),
    );
    f.build()
}
