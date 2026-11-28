//! Port of `internal/store/statefulset.go`.

use k8s_openapi::api::apps::v1::{StatefulSet, StatefulSetStatus};
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "statefulsets";
pub const GO_TYPE: &str = "*v1.StatefulSet";

fn status(s: &StatefulSet) -> Option<&StatefulSetStatus> {
    s.status.as_ref()
}

/// A status count, 0 when unset (a plain `int32` in Go).
fn count(s: &StatefulSet, f: fn(&StatefulSetStatus) -> Option<i32>) -> Vec<Metric<'_>> {
    vec![Metric::value(f64::from(status(s).and_then(f).unwrap_or(0)))]
}

/// A status revision as the `revision` label.
fn revision(s: &StatefulSet, f: fn(&StatefulSetStatus) -> &Option<String>) -> Vec<Metric<'_>> {
    let rev = status(s).map_or("", |st| self::s(f(st)));
    vec![m1("revision", rev, 1.0)]
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<StatefulSet>> {
    let mut f = Families::new(&["namespace", "statefulset"], |s: &StatefulSet| {
        vec![ns(&s.metadata), name(&s.metadata)]
    });
    f.add(
        "kube_statefulset_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |s| created_metric(&s.metadata),
    )
    .add(
        "kube_statefulset_status_replicas",
        "The number of replicas per StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| count(s, |st| Some(st.replicas)),
    )
    .add(
        "kube_statefulset_status_replicas_available",
        "The number of available replicas per StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| count(s, |st| st.available_replicas),
    )
    .add(
        "kube_statefulset_status_replicas_current",
        "The number of current replicas per StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| count(s, |st| st.current_replicas),
    )
    .add(
        "kube_statefulset_status_replicas_ready",
        "The number of ready replicas per StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| count(s, |st| st.ready_replicas),
    )
    .add(
        "kube_statefulset_status_replicas_updated",
        "The number of updated replicas per StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| count(s, |st| st.updated_replicas),
    )
    .add(
        "kube_statefulset_status_observed_generation",
        "The generation observed by the StatefulSet controller.",
        Gauge,
        Stable,
        "",
        |s| {
            let g = status(s).and_then(|st| st.observed_generation).unwrap_or(0);
            vec![Metric::value(i64_f64(g))]
        },
    )
    .add(
        "kube_statefulset_replicas",
        "Number of desired pods for a StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| opt_i32_metric(s.spec.replicas),
    )
    .add(
        "kube_statefulset_ordinals_start",
        "Start ordinal of the StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| {
            s.spec
                .ordinals
                .as_ref()
                .map(|o| vec![Metric::value(f64::from(o.start.unwrap_or(0)))])
                .unwrap_or_default()
        },
    )
    .add(
        "kube_statefulset_metadata_generation",
        "Sequence number representing a specific generation of the desired state for the StatefulSet.",
        Gauge,
        Stable,
        "",
        |s| generation_metric(&s.metadata),
    )
    .add(
        "kube_statefulset_persistentvolumeclaim_retention_policy",
        "Count of retention policy for StatefulSet template PVCs",
        Gauge,
        Stable,
        "",
        |s| {
            let policy = s.spec.persistent_volume_claim_retention_policy.as_ref();
            let deleted = policy.map_or("", |p| self::s(&p.when_deleted));
            let scaled = policy.map_or("", |p| self::s(&p.when_scaled));
            if deleted.is_empty() && scaled.is_empty() {
                return Vec::new();
            }
            vec![mk(
                &["when_deleted", "when_scaled"],
                values![deleted, scaled],
                1.0,
            )]
        },
    )
    .annotations(
        "kube_statefulset_annotations",
        Stable,
        allow_annotations,
        |s| &s.metadata,
    )
    .labels("kube_statefulset_labels", Stable, allow_labels, |s| {
        &s.metadata
    })
    .add(
        "kube_statefulset_status_current_revision",
        "Indicates the version of the StatefulSet used to generate Pods in the sequence [0,currentReplicas).",
        Gauge,
        Stable,
        "",
        |s| revision(s, |st| &st.current_revision),
    )
    .add(
        "kube_statefulset_status_update_revision",
        "Indicates the version of the StatefulSet used to generate Pods in the sequence [replicas-updatedReplicas,replicas)",
        Gauge,
        Stable,
        "",
        |s| revision(s, |st| &st.update_revision),
    )
    .add(
        "kube_statefulset_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Stable,
        "",
        |s| deletion_metric(&s.metadata),
    );
    f.build()
}
