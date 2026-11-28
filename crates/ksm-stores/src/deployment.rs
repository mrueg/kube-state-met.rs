//! Port of `internal/store/deployment.go`.

use k8s_openapi::api::apps::v1::{Deployment, DeploymentStatus, RollingUpdateDeployment};
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "deployments";
pub const GO_TYPE: &str = "*v1.Deployment";

/// `allowedDeploymentReasons`; any other reason is reported as `unknown`.
const ALLOWED_DEPLOYMENT_REASONS: [&str; 11] = [
    "ReplicaSetUpdated",
    "ReplicaSetCreateError",
    "NewReplicaSetCreated",
    "FoundNewReplicaSet",
    "NewReplicaSetAvailable",
    "ProgressDeadlineExceeded",
    "DeploymentPaused",
    "DeploymentResumed",
    "MinimumReplicasAvailable",
    "MinimumReplicasUnavailable",
    "",
];

fn status(d: &Deployment) -> Option<&DeploymentStatus> {
    d.status.as_ref()
}

/// A status count, 0 when unset (a plain `int32` in Go).
fn count(d: &Deployment, f: fn(&DeploymentStatus) -> Option<i32>) -> Vec<Metric<'_>> {
    vec![Metric::value(f64::from(status(d).and_then(f).unwrap_or(0)))]
}

fn rolling_update(d: &Deployment) -> Option<&RollingUpdateDeployment> {
    d.spec
        .strategy
        .as_ref()
        .and_then(|s| s.rolling_update.as_ref())
}

/// The rolling update's max surge or max unavailable, scaled by the desired
/// replicas; none without a rolling update or replicas, or on a bad value.
fn rolling_update_metric(d: &Deployment, surge: bool) -> Vec<Metric<'_>> {
    let (Some(ru), Some(replicas)) = (rolling_update(d), d.spec.replicas) else {
        return Vec::new();
    };
    let v = if surge {
        ru.max_surge.as_ref()
    } else {
        ru.max_unavailable.as_ref()
    };
    scaled_value_from_int_or_percent(v, i64::from(replicas), surge)
        .map(|v| vec![Metric::value(i64_f64(v))])
        .unwrap_or_default()
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Deployment>> {
    let mut f = Families::new(&["namespace", "deployment"], |d: &Deployment| {
        vec![ns(&d.metadata), name(&d.metadata)]
    });
    f.add(
        "kube_deployment_owner",
        "Information about the Deployment's owner.",
        Gauge,
        Alpha,
        "",
        |d| owner_metrics(&d.metadata, false),
    )
    .add(
        "kube_deployment_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |d| created_metric(&d.metadata),
    )
    .add(
        "kube_deployment_status_replicas",
        "The number of replicas per deployment.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.replicas),
    )
    .add(
        "kube_deployment_status_replicas_ready",
        "The number of ready replicas per deployment.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.ready_replicas),
    )
    .add(
        "kube_deployment_status_replicas_available",
        "The number of available replicas per deployment.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.available_replicas),
    )
    .add(
        "kube_deployment_status_replicas_unavailable",
        "The number of unavailable replicas per deployment.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.unavailable_replicas),
    )
    .add(
        "kube_deployment_status_replicas_updated",
        "The number of updated replicas per deployment.",
        Gauge,
        Stable,
        "",
        |d| count(d, |s| s.updated_replicas),
    )
    .add(
        "kube_deployment_status_terminating_replicas",
        "The number of terminating replicas per deployment.",
        Gauge,
        Alpha,
        "",
        |d| opt_i32_metric(status(d).and_then(|s| s.terminating_replicas)),
    )
    .add(
        "kube_deployment_status_observed_generation",
        "The generation observed by the deployment controller.",
        Gauge,
        Stable,
        "",
        |d| {
            let g = status(d).and_then(|s| s.observed_generation).unwrap_or(0);
            vec![Metric::value(i64_f64(g))]
        },
    )
    .add(
        "kube_deployment_status_condition",
        "The current status conditions of a deployment.",
        Gauge,
        Stable,
        "",
        |d| {
            status(d)
                .and_then(|s| s.conditions.as_deref())
                .unwrap_or_default()
                .iter()
                .flat_map(|c| {
                    let reason = s(&c.reason);
                    let reason = if ALLOWED_DEPLOYMENT_REASONS.contains(&reason) {
                        reason
                    } else {
                        "unknown"
                    };
                    conditions_with(&["reason", "condition"], &[reason, s(&c.type_)], s(&c.status))
                })
                .collect()
        },
    )
    .add(
        "kube_deployment_spec_replicas",
        "Number of desired pods for a deployment.",
        Gauge,
        Stable,
        "",
        |d| opt_i32_metric(d.spec.replicas),
    )
    .add(
        "kube_deployment_spec_paused",
        "Whether the deployment is paused and will not be processed by the deployment controller.",
        Gauge,
        Stable,
        "",
        |d| vec![Metric::value(bool_f64(d.spec.paused == Some(true)))],
    )
    .add(
        "kube_deployment_spec_strategy_rollingupdate_max_unavailable",
        "Maximum number of unavailable replicas during a rolling update of a deployment.",
        Gauge,
        Stable,
        "",
        |d| rolling_update_metric(d, false),
    )
    .add(
        "kube_deployment_spec_strategy_rollingupdate_max_surge",
        "Maximum number of replicas that can be scheduled above the desired number of replicas during a rolling update of a deployment.",
        Gauge,
        Stable,
        "",
        |d| rolling_update_metric(d, true),
    )
    .add(
        "kube_deployment_metadata_generation",
        "Sequence number representing a specific generation of the desired state.",
        Gauge,
        Stable,
        "",
        |d| generation_metric(&d.metadata),
    )
    .add(
        "kube_deployment_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |d| deletion_metric(&d.metadata),
    )
    .annotations(
        "kube_deployment_annotations",
        Alpha,
        allow_annotations,
        |d| &d.metadata,
    )
    .labels("kube_deployment_labels", Stable, allow_labels, |d| {
        &d.metadata
    });
    f.build()
}
