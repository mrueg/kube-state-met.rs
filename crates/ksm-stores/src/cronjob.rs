//! Port of `internal/store/cronjob.go`.

use k8s_openapi::api::batch::v1::{CronJob, CronJobSpec, CronJobStatus};
use ksm_core::{FamilyGenerator, Metric};

use crate::cron;
use crate::util::*;

pub const NAME: &str = "cronjobs";
pub const GO_TYPE: &str = "*v1.CronJob";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

fn spec(j: &CronJob) -> &CronJobSpec {
    &j.spec
}

fn schedule(j: &CronJob) -> &str {
    &spec(j).schedule
}

fn time_zone(j: &CronJob) -> Option<&str> {
    spec(j).time_zone.as_deref()
}

fn status(j: &CronJob) -> Option<&CronJobStatus> {
    j.status.as_ref()
}

fn suspended(j: &CronJob) -> bool {
    spec(j).suspend == Some(true)
}

/// `getNextScheduledTime`: Unix seconds of the next run after the last
/// schedule time, or after creation when it never ran.
fn next_scheduled_time(j: &CronJob) -> Result<i64, String> {
    let sched = cron::parse_schedule(schedule(j), time_zone(j))?;
    let last = status(j).and_then(|s| s.last_schedule_time.as_ref());
    if !time_is_zero(last)
        && let Some(t) = last
    {
        return Ok(sched.next_unix(t.0.as_nanosecond()));
    }
    let created = j.metadata.creation_timestamp.as_ref();
    if !time_is_zero(created)
        && let Some(t) = created
    {
        return Ok(sched.next_unix(t.0.as_nanosecond()));
    }
    Err("createdTime and lastScheduleTime are both zero".into())
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts the length to float64 the same way"
)]
fn len_f64(n: usize) -> f64 {
    n as f64
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<CronJob>> {
    let mut f = Families::new(&["namespace", "cronjob"], |j: &CronJob| {
        vec![ns(&j.metadata), name(&j.metadata)]
    });
    f.annotations(
        "kube_cronjob_annotations",
        Alpha,
        allow_annotations,
        |j| &j.metadata,
    )
    .labels("kube_cronjob_labels", Stable, allow_labels, |j| &j.metadata)
    .add(
        "kube_cronjob_info",
        "Info about cronjob.",
        Gauge,
        Stable,
        "",
        |j| {
            vec![mk(
                &["schedule", "concurrency_policy", "timezone"],
                vec![
                    schedule(j).to_owned(),
                    s(&spec(j).concurrency_policy).to_owned(),
                    time_zone(j).unwrap_or("local").to_owned(),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_cronjob_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |j| created_metric(&j.metadata),
    )
    .add(
        "kube_cronjob_status_active",
        "Active holds pointers to currently running jobs.",
        Gauge,
        Stable,
        "",
        |j| {
            let n = status(j).and_then(|s| s.active.as_ref()).map_or(0, Vec::len);
            vec![Metric::value(len_f64(n))]
        },
    )
    .add(
        "kube_cronjob_status_last_schedule_time",
        "LastScheduleTime keeps information of when was the last time the job was successfully scheduled.",
        Gauge,
        Stable,
        "",
        |j| opt_time_metric(status(j).and_then(|s| s.last_schedule_time.as_ref())),
    )
    .add(
        "kube_cronjob_status_last_successful_time",
        "LastSuccessfulTime keeps information of when was the last time the job was completed successfully.",
        Gauge,
        Stable,
        "",
        |j| opt_time_metric(status(j).and_then(|s| s.last_successful_time.as_ref())),
    )
    .add(
        "kube_cronjob_spec_suspend",
        "Suspend flag tells the controller to suspend subsequent executions.",
        Gauge,
        Stable,
        "",
        |j| vec![Metric::value(bool_f64(suspended(j)))],
    )
    .add(
        "kube_cronjob_spec_starting_deadline_seconds",
        "Deadline in seconds for starting the job if it misses scheduled time for any reason.",
        Gauge,
        Stable,
        "",
        |j| {
            spec(j)
                .starting_deadline_seconds
                .map(|v| vec![Metric::value(i64_f64(v))])
                .unwrap_or_default()
        },
    )
    .add(
        "kube_cronjob_next_schedule_time",
        "Next time the cronjob should be scheduled. The time after lastScheduleTime, or after the cron job's creation time if it's never been scheduled. Use this to determine if the job is delayed.",
        Gauge,
        Stable,
        "",
        |j| match next_scheduled_time(j) {
            Ok(next) if !suspended(j) => vec![Metric::value(i64_f64(next))],
            _ => Vec::new(),
        },
    )
    .add(
        "kube_cronjob_schedule_invalid",
        "Emitted with value 1 for cronjobs whose schedule, in its configured timezone, cannot be parsed.",
        Gauge,
        Alpha,
        "",
        |j| {
            if cron::parse_schedule(schedule(j), time_zone(j)).is_err() {
                vec![Metric::value(1.0)]
            } else {
                Vec::new()
            }
        },
    )
    .add(
        "kube_cronjob_metadata_resource_version",
        "Resource version representing a specific version of the cronjob.",
        Gauge,
        Stable,
        "",
        |j| rv_metric(&j.metadata),
    )
    .add(
        "kube_cronjob_spec_successful_job_history_limit",
        "Successful job history limit tells the controller how many completed jobs should be preserved.",
        Gauge,
        Alpha,
        "",
        |j| opt_i32_metric(spec(j).successful_jobs_history_limit),
    )
    .add(
        "kube_cronjob_spec_failed_job_history_limit",
        "Failed job history limit tells the controller how many failed jobs should be preserved.",
        Gauge,
        Alpha,
        "",
        |j| opt_i32_metric(spec(j).failed_jobs_history_limit),
    );
    f.build()
}
