//! Port of `internal/store/horizontalpodautoscaler.go`.

use k8s_openapi::api::autoscaling::v2::{
    HPAScalingRules, HorizontalPodAutoscaler, HorizontalPodAutoscalerBehavior,
    HorizontalPodAutoscalerStatus,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use ksm_core::{FamilyGenerator, Metric};

use crate::quantity;
use crate::util::*;

pub const NAME: &str = "horizontalpodautoscalers";
pub const GO_TYPE: &str = "*v2.HorizontalPodAutoscaler";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

/// `targetMetricLabels`.
const TARGET_METRIC_LABELS: &[&str] = &["metric_name", "metric_target_type", "container"];

/// The `value` / `utilization` / `average` samples of one metric target or
/// current status (Go's `metricMap`, keyed by `metricTargetType`).
fn target_metrics(
    metric_name: &str,
    container_name: &str,
    value: Option<&Quantity>,
    average_value: Option<&Quantity>,
    average_utilization: Option<i32>,
) -> Vec<Metric> {
    let mut entries: Vec<(&str, f64)> = Vec::with_capacity(3);
    if let Some(v) = value {
        entries.push(("value", quantity::ksm_f64(v)));
    }
    if let Some(u) = average_utilization {
        entries.push(("utilization", f64::from(u)));
    }
    if let Some(a) = average_value {
        entries.push(("average", quantity::ksm_f64(a)));
    }
    entries
        .into_iter()
        .map(|(ty, v)| {
            mk(
                TARGET_METRIC_LABELS,
                vec![
                    metric_name.to_owned(),
                    ty.to_owned(),
                    container_name.to_owned(),
                ],
                v,
            )
        })
        .collect()
}

fn status(a: &HorizontalPodAutoscaler) -> Option<&HorizontalPodAutoscalerStatus> {
    a.status.as_ref()
}

fn behavior(a: &HorizontalPodAutoscaler) -> Option<&HorizontalPodAutoscalerBehavior> {
    a.spec.behavior.as_ref()
}

/// The tolerance sample of a scaling rule, none when unset.
fn tolerance_metric(rules: Option<&HPAScalingRules>) -> Vec<Metric> {
    rules
        .and_then(|r| r.tolerance.as_ref())
        .map(|t| Metric::value(quantity::from_k8s(t).as_approximate_f64()))
        .into_iter()
        .collect()
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<HorizontalPodAutoscaler>> {
    let mut f = Families::new(
        &["namespace", "horizontalpodautoscaler"],
        |a: &HorizontalPodAutoscaler| vec![ns(&a.metadata), name(&a.metadata)],
    );
    f.add(
        "kube_horizontalpodautoscaler_info",
        "Information about this autoscaler.",
        Gauge,
        Alpha,
        "",
        |a| {
            let r = &a.spec.scale_target_ref;
            let kind = r.kind.clone();
            let name = r.name.clone();
            let api_version = s(&r.api_version);
            if api_version.is_empty() {
                vec![mk(
                    &["scaletargetref_kind", "scaletargetref_name"],
                    vec![kind, name],
                    1.0,
                )]
            } else {
                vec![mk(
                    &[
                        "scaletargetref_api_version",
                        "scaletargetref_kind",
                        "scaletargetref_name",
                    ],
                    vec![api_version.to_owned(), kind, name],
                    1.0,
                )]
            }
        },
    )
    .add(
        "kube_horizontalpodautoscaler_metadata_generation",
        "The generation observed by the HorizontalPodAutoscaler controller.",
        Gauge,
        Stable,
        "",
        |a| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "Go converts int64 to float64 the same way"
            )]
            let g = a.metadata.generation.unwrap_or(0) as f64;
            vec![Metric::value(g)]
        },
    )
    .add(
        "kube_horizontalpodautoscaler_spec_max_replicas",
        "Upper limit for the number of pods that can be set by the autoscaler; cannot be smaller than MinReplicas.",
        Gauge,
        Stable,
        "",
        |a| {
            vec![Metric::value(f64::from(a.spec.max_replicas))]
        },
    )
    .add(
        "kube_horizontalpodautoscaler_spec_min_replicas",
        "Lower limit for the number of pods that can be set by the autoscaler, default 1.",
        Gauge,
        Stable,
        "",
        |a| {
            vec![Metric::value(i32_or(
                a.spec.min_replicas,
                1,
            ))]
        },
    )
    .add(
        "kube_horizontalpodautoscaler_spec_target_metric",
        "The metric specifications used by this autoscaler when calculating the desired replica count.",
        Gauge,
        Alpha,
        "",
        |a| {
            let mut ms = Vec::new();
            for m in a.spec.metrics.iter().flatten() {
                // validateMetricSpec requires the source named by Type, but
                // don't rely on it.
                let (metric_name, target, container) = match m.type_.as_str() {
                    "Object" => match &m.object {
                        Some(o) => (o.metric.name.as_str(), &o.target, ""),
                        None => continue,
                    },
                    "Pods" => match &m.pods {
                        Some(p) => (p.metric.name.as_str(), &p.target, ""),
                        None => continue,
                    },
                    "Resource" => match &m.resource {
                        Some(r) => (r.name.as_str(), &r.target, ""),
                        None => continue,
                    },
                    "ContainerResource" => match &m.container_resource {
                        Some(c) => (c.name.as_str(), &c.target, c.container.as_str()),
                        None => continue,
                    },
                    "External" => match &m.external {
                        Some(e) => (e.metric.name.as_str(), &e.target, ""),
                        None => continue,
                    },
                    // Skip unsupported metric type
                    _ => continue,
                };
                ms.extend(target_metrics(
                    metric_name,
                    container,
                    target.value.as_ref(),
                    target.average_value.as_ref(),
                    target.average_utilization,
                ));
            }
            ms
        },
    )
    .add(
        "kube_horizontalpodautoscaler_spec_behavior_scale_down_tolerance",
        "The tolerance on the ratio between the current and desired metric value below which no scale down occurs.",
        Gauge,
        Alpha,
        "",
        |a| tolerance_metric(behavior(a).and_then(|b| b.scale_down.as_ref())),
    )
    .add(
        "kube_horizontalpodautoscaler_spec_behavior_scale_up_tolerance",
        "The tolerance on the ratio between the current and desired metric value below which no scale up occurs.",
        Gauge,
        Alpha,
        "",
        |a| tolerance_metric(behavior(a).and_then(|b| b.scale_up.as_ref())),
    )
    .add(
        "kube_horizontalpodautoscaler_status_target_metric",
        "The current metric status used by this autoscaler when calculating the desired replica count.",
        Gauge,
        Stable,
        "",
        |a| {
            let mut ms = Vec::new();
            for m in status(a)
                .and_then(|st| st.current_metrics.as_ref())
                .into_iter()
                .flatten()
            {
                // status.currentMetrics is not validated: the source matching
                // Type may be absent; skip such an entry.
                let (metric_name, current, container) = match m.type_.as_str() {
                    "Object" => match &m.object {
                        Some(o) => (o.metric.name.as_str(), &o.current, ""),
                        None => continue,
                    },
                    "Pods" => match &m.pods {
                        Some(p) => (p.metric.name.as_str(), &p.current, ""),
                        None => continue,
                    },
                    "Resource" => match &m.resource {
                        Some(r) => (r.name.as_str(), &r.current, ""),
                        None => continue,
                    },
                    "ContainerResource" => match &m.container_resource {
                        Some(c) => (c.name.as_str(), &c.current, c.container.as_str()),
                        None => continue,
                    },
                    "External" => match &m.external {
                        Some(e) => (e.metric.name.as_str(), &e.current, ""),
                        None => continue,
                    },
                    // Skip unsupported metric type
                    _ => continue,
                };
                ms.extend(target_metrics(
                    metric_name,
                    container,
                    current.value.as_ref(),
                    current.average_value.as_ref(),
                    current.average_utilization,
                ));
            }
            ms
        },
    )
    .add(
        "kube_horizontalpodautoscaler_status_current_replicas",
        "Current number of replicas of pods managed by this autoscaler.",
        Gauge,
        Stable,
        "",
        |a| {
            vec![Metric::value(i32_or(
                status(a).and_then(|st| st.current_replicas),
                0,
            ))]
        },
    )
    .add(
        "kube_horizontalpodautoscaler_status_desired_replicas",
        "Desired number of replicas of pods managed by this autoscaler.",
        Gauge,
        Stable,
        "",
        |a| {
            vec![Metric::value(f64::from(
                status(a).map_or(0, |st| st.desired_replicas),
            ))]
        },
    )
    .annotations(
        "kube_horizontalpodautoscaler_annotations",
        Alpha,
        allow_annotations,
        |a| &a.metadata,
    )
    .labels(
        "kube_horizontalpodautoscaler_labels",
        Stable,
        allow_labels,
        |a| &a.metadata,
    )
    .add(
        "kube_horizontalpodautoscaler_status_condition",
        "The condition of this autoscaler.",
        Gauge,
        Stable,
        "",
        |a| {
            status(a)
                .and_then(|st| st.conditions.as_ref())
                .into_iter()
                .flatten()
                .flat_map(|c| conditions_with(&["condition"], &[&c.type_], &c.status))
                .collect()
        },
    )
    .add(
        "kube_horizontalpodautoscaler_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |a| created_metric(&a.metadata),
    )
    .add(
        "kube_horizontalpodautoscaler_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |a| deletion_metric(&a.metadata),
    );
    f.build()
}
