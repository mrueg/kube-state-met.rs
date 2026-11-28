//! Port of `internal/store/job.go`.

use k8s_openapi::api::batch::v1::{Job, JobCondition, JobSpec, JobStatus};
use ksm_core::labels::condition_metrics_keyed;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "jobs";
pub const GO_TYPE: &str = "*v1.Job";

/// `jobFailureReasons`: the reasons of the stable `kube_job_status_failed`.
const JOB_FAILURE_REASONS: [&str; 3] = ["BackoffLimitExceeded", "DeadlineExceeded", "Evicted"];

/// `jobFailedConditionReasons`.
const JOB_FAILED_CONDITION_REASONS: [&str; 5] = [
    "BackoffLimitExceeded",
    "DeadlineExceeded",
    "PodFailurePolicy",
    "MaxFailedIndexesExceeded",
    "FailedIndexes",
];

fn spec(j: &Job) -> Option<&JobSpec> {
    j.spec.as_ref()
}

fn status(j: &Job) -> Option<&JobStatus> {
    j.status.as_ref()
}

fn conditions(j: &Job) -> &[JobCondition] {
    status(j)
        .and_then(|s| s.conditions.as_deref())
        .unwrap_or_default()
}

/// A status count, 0 when unset (a plain `int32` in Go).
fn count(j: &Job, f: fn(&JobStatus) -> Option<i32>) -> f64 {
    f64::from(status(j).and_then(f).unwrap_or(0))
}

/// The `condition` samples of every condition of one type.
fn condition_family(j: &Job, type_: &str) -> Vec<Metric<'static>> {
    conditions(j)
        .iter()
        .filter(|c| c.type_ == type_)
        .flat_map(|c| condition_metrics_keyed("condition", &c.status))
        .collect()
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Job>> {
    let mut f = Families::new(&["namespace", "job_name"], |j: &Job| {
        vec![ns(&j.metadata), name(&j.metadata)]
    });
    f.annotations("kube_job_annotations", Alpha, allow_annotations, |j| {
        &j.metadata
    })
    .labels("kube_job_labels", Stable, allow_labels, |j| &j.metadata)
    .add(
        "kube_job_info",
        "Information about job.",
        Gauge,
        Stable,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_job_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |j| created_metric(&j.metadata),
    )
    .add(
        "kube_job_spec_parallelism",
        "The maximum desired number of pods the job should run at any given time.",
        Gauge,
        Stable,
        "",
        |j| opt_i32_metric(spec(j).and_then(|s| s.parallelism)),
    )
    .add(
        "kube_job_spec_completions",
        "The desired number of successfully finished pods the job should be run with.",
        Gauge,
        Stable,
        "",
        |j| opt_i32_metric(spec(j).and_then(|s| s.completions)),
    )
    .add(
        "kube_job_spec_active_deadline_seconds",
        "The duration in seconds relative to the startTime that the job may be active before the system tries to terminate it.",
        Gauge,
        Stable,
        "",
        |j| {
            spec(j)
                .and_then(|s| s.active_deadline_seconds)
                .map(|v| vec![Metric::value(i64_f64(v))])
                .unwrap_or_default()
        },
    )
    .add(
        "kube_job_status_succeeded",
        "The number of pods which reached Phase Succeeded.",
        Gauge,
        Stable,
        "",
        |j| vec![Metric::value(count(j, |s| s.succeeded))],
    )
    .add(
        "kube_job_status_failed",
        "The number of pods which reached Phase Failed and the reason for failure.",
        Gauge,
        Stable,
        "",
        |j| {
            let failed = count(j, |s| s.failed);
            if failed == 0.0 {
                return vec![Metric::value(failed)];
            }
            let mut ms = Vec::new();
            let mut reason_known = false;
            for c in conditions(j).iter().filter(|c| c.type_ == "Failed") {
                for reason in JOB_FAILURE_REASONS {
                    let matches = s(&c.reason) == reason;
                    reason_known = reason_known || matches;
                    ms.push(m1("reason", reason, bool_f64(matches)));
                }
            }
            if !reason_known {
                ms.push(m1("reason", "", failed));
            }
            ms
        },
    )
    .add(
        "kube_job_status_failure_reason",
        "The reason a Job's Failed condition is currently set, if any. Emitted only for the reason that is actually set; a missing series does not mean the reason is false. An unrecognized reason is reported as Other.",
        Gauge,
        Alpha,
        "",
        |j| {
            let mut seen: Vec<&str> = Vec::new();
            let mut ms = Vec::new();
            for c in conditions(j) {
                let reason = s(&c.reason);
                if c.type_ != "Failed" || c.status != "True" || reason.is_empty() {
                    continue;
                }
                let reason = if JOB_FAILED_CONDITION_REASONS.contains(&reason) {
                    reason
                } else {
                    "Other"
                };
                if seen.contains(&reason) {
                    continue;
                }
                seen.push(reason);
                ms.push(m1("reason", reason, 1.0));
            }
            ms
        },
    )
    .add(
        "kube_job_status_active",
        "The number of actively running pods.",
        Gauge,
        Stable,
        "",
        |j| vec![Metric::value(count(j, |s| s.active))],
    )
    .add(
        "kube_job_status_ready",
        "The number of ready pods that belong to this Job.",
        Gauge,
        Alpha,
        "",
        |j| vec![Metric::value(count(j, |s| s.ready))],
    )
    .add(
        "kube_job_complete",
        "The job has completed its execution.",
        Gauge,
        Stable,
        "",
        |j| condition_family(j, "Complete"),
    )
    .add(
        "kube_job_failed",
        "The job has failed its execution.",
        Gauge,
        Stable,
        "",
        |j| condition_family(j, "Failed"),
    )
    .add(
        "kube_job_status_start_time",
        "StartTime represents time when the job was acknowledged by the Job Manager.",
        Gauge,
        Stable,
        "",
        |j| opt_time_metric(status(j).and_then(|s| s.start_time.as_ref())),
    )
    .add(
        "kube_job_status_completion_time",
        "CompletionTime represents time when the job was completed.",
        Gauge,
        Stable,
        "",
        |j| opt_time_metric(status(j).and_then(|s| s.completion_time.as_ref())),
    )
    .add(
        "kube_job_status_suspended",
        "The number of pods which reached Phase Suspended.",
        Gauge,
        Alpha,
        "",
        |j| {
            conditions(j)
                .iter()
                .filter(|c| c.type_ == "Suspended")
                .map(|c| Metric::value(bool_f64(c.status == "True")))
                .collect()
        },
    )
    .add(
        "kube_job_owner",
        "Information about the Job's owner.",
        Gauge,
        Stable,
        "",
        |j| owner_metrics(&j.metadata, true),
    );
    f.build()
}
