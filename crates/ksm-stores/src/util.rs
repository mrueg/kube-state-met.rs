//! Helpers shared by the resource stores (Go: `internal/store/utils.go` and
//! the per-resource `wrapXFunc` functions).

use std::sync::Arc;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta, Time};
use ksm_core::Stability;
use ksm_core::generator::FamilyGenerator;
use ksm_core::metric::{Family, Metric, MetricType};

pub use ksm_core::Stability::{Alpha, Beta, Stable};
pub use ksm_core::labels::{
    allowed_labels_metric, bool_f64, condition_metrics, create_prometheus_label_keys_values,
    go_parse_float, map_to_prometheus_labels, resource_version_metric,
};
pub use ksm_core::metric::MetricType::{Counter, Gauge, Info, StateSet};

/// Values of a resource's default labels, borrowed from the object.
pub type DefaultLabels<K> = for<'a> fn(&'a K) -> Vec<&'a str>;

/// Builds a resource's family list; every sample gets the resource's default
/// labels first (namespace, name, ...), as `wrapXFunc` does in Go.
pub struct Families<K: 'static> {
    keys: &'static [&'static str],
    values: DefaultLabels<K>,
    out: Vec<FamilyGenerator<K>>,
}

impl<K: 'static> Families<K> {
    pub fn new(keys: &'static [&'static str], values: DefaultLabels<K>) -> Self {
        Families {
            keys,
            values,
            out: Vec::new(),
        }
    }

    fn wrap(
        &self,
        f: impl Fn(&K) -> Vec<Metric> + Send + Sync + 'static,
    ) -> impl Fn(&K) -> Family + Send + Sync + 'static {
        let keys = self.keys;
        let values = self.values;
        move |obj: &K| {
            let mut metrics = f(obj);
            if !keys.is_empty() {
                let v = values(obj);
                for m in &mut metrics {
                    m.prepend_labels(keys, &v);
                }
            }
            Family::new(metrics)
        }
    }

    /// Adds a family.
    pub fn add(
        &mut self,
        name: &str,
        help: &str,
        metric_type: MetricType,
        stability: Stability,
        deprecated: &str,
        f: impl Fn(&K) -> Vec<Metric> + Send + Sync + 'static,
    ) -> &mut Self {
        let g = FamilyGenerator::new(name, help, metric_type, stability, deprecated, self.wrap(f));
        self.out.push(g);
        self
    }

    /// Adds an opt-in family.
    pub fn opt_in(
        &mut self,
        name: &str,
        help: &str,
        metric_type: MetricType,
        stability: Stability,
        deprecated: &str,
        f: impl Fn(&K) -> Vec<Metric> + Send + Sync + 'static,
    ) -> &mut Self {
        let g =
            FamilyGenerator::opt_in(name, help, metric_type, stability, deprecated, self.wrap(f));
        self.out.push(g);
        self
    }

    /// Adds the `_annotations` family for an annotation allow list.
    pub fn annotations(
        &mut self,
        name: &str,
        stability: Stability,
        allow: &[String],
        meta: fn(&K) -> &ObjectMeta,
    ) -> &mut Self {
        let allow: Arc<[String]> = allow.into();
        self.add(
            name,
            "Kubernetes annotations converted to Prometheus labels.",
            Gauge,
            stability,
            "",
            move |o| {
                allowed_labels_metric("annotation", meta(o).annotations.as_ref(), Some(&allow))
            },
        )
    }

    /// Adds the `_labels` family for a label allow list.
    pub fn labels(
        &mut self,
        name: &str,
        stability: Stability,
        allow: &[String],
        meta: fn(&K) -> &ObjectMeta,
    ) -> &mut Self {
        let allow: Arc<[String]> = allow.into();
        self.add(
            name,
            "Kubernetes labels converted to Prometheus labels.",
            Gauge,
            stability,
            "",
            move |o| allowed_labels_metric("label", meta(o).labels.as_ref(), Some(&allow)),
        )
    }

    pub fn build(self) -> Vec<FamilyGenerator<K>> {
        self.out
    }
}

/// `""` for a missing string, as Go's zero value.
pub fn s(v: &Option<String>) -> &str {
    v.as_deref().unwrap_or("")
}

/// Namespace of an object.
pub fn ns(m: &ObjectMeta) -> &str {
    s(&m.namespace)
}

/// Name of an object.
pub fn name(m: &ObjectMeta) -> &str {
    s(&m.name)
}

/// Seconds since the epoch, rounded down (Go `time.Time.Unix()`).
pub fn unix(t: &k8s_openapi::jiff::Timestamp) -> i64 {
    let secs = t.as_second();
    if t.subsec_nanosecond() < 0 {
        secs - 1
    } else {
        secs
    }
}

/// Go's `metav1.Time.IsZero()` for an optional time.
pub fn time_is_zero(t: Option<&Time>) -> bool {
    match t {
        None => true,
        Some(t) => t.0.as_second() == GO_ZERO_TIME_UNIX && t.0.subsec_nanosecond() == 0,
    }
}

/// Unix seconds of Go's zero `time.Time` (0001-01-01T00:00:00Z).
const GO_ZERO_TIME_UNIX: i64 = -62_135_596_800;

/// `float64(t.Unix())` of a set time.
#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts int64 seconds to float64 the same way"
)]
pub fn time_f64(t: &Time) -> f64 {
    unix(&t.0) as f64
}

/// `float64(t.Unix())` of a set micro time.
#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts int64 seconds to float64 the same way"
)]
pub fn micro_time_f64(t: &MicroTime) -> f64 {
    unix(&t.0) as f64
}

/// The `_created` sample: none when the creation timestamp is zero.
pub fn created_metric(m: &ObjectMeta) -> Vec<Metric> {
    match &m.creation_timestamp {
        Some(t) if !time_is_zero(Some(t)) => vec![Metric::value(time_f64(t))],
        _ => Vec::new(),
    }
}

/// The `_deletion_timestamp` sample: none unless the object is being deleted.
pub fn deletion_metric(m: &ObjectMeta) -> Vec<Metric> {
    match &m.deletion_timestamp {
        Some(t) if !time_is_zero(Some(t)) => vec![Metric::value(time_f64(t))],
        _ => Vec::new(),
    }
}

/// The `_metadata_resource_version` sample.
pub fn rv_metric(m: &ObjectMeta) -> Vec<Metric> {
    resource_version_metric(m.resource_version.as_deref())
}

/// Optional i32 as f64, 0 when unset (Go dereferences a default).
pub fn i32_or(v: Option<i32>, default: i32) -> f64 {
    f64::from(v.unwrap_or(default))
}

/// A sample with one static label.
pub fn m1(key: &'static str, value: impl Into<String>, v: f64) -> Metric {
    Metric::with(&[key], vec![value.into()], v)
}

/// A sample with static labels.
pub fn mk(keys: &[&'static str], values: Vec<String>, v: f64) -> Metric {
    Metric::with(keys, values, v)
}

/// The `{"true","false","unknown"}` condition samples with the condition's
/// own labels in front of `status`.
pub fn conditions_with(keys: &[&'static str], values: &[&str], status: &str) -> Vec<Metric> {
    condition_metrics(status)
        .into_iter()
        .map(|c| {
            let mut k: Vec<ksm_core::metric::LabelKey> = keys
                .iter()
                .map(|k| std::borrow::Cow::Borrowed(*k))
                .collect();
            k.push(std::borrow::Cow::Borrowed("status"));
            let mut v: Vec<String> = values.iter().map(|s| (*s).to_owned()).collect();
            v.extend(c.label_values);
            Metric::new(k, v, c.value)
        })
        .collect()
}

/// Go's `metav1.MicroTime.IsZero()` for an optional micro time.
pub fn micro_time_is_zero(t: Option<&MicroTime>) -> bool {
    match t {
        None => true,
        Some(t) => t.0.as_second() == GO_ZERO_TIME_UNIX && t.0.subsec_nanosecond() == 0,
    }
}

/// `float64(t.Unix())` of a value-typed Go time that may be unset: an unset
/// time is Go's zero time (-62135596800).
#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts int64 seconds to float64 the same way"
)]
pub fn time_or_zero_f64(t: Option<&Time>) -> f64 {
    match t {
        Some(t) => time_f64(t),
        None => GO_ZERO_TIME_UNIX as f64,
    }
}

/// The `owner_kind`, `owner_name`, `owner_is_controller` samples of an
/// object's owner references (a single empty sample when there are none).
pub fn owner_metrics(m: &ObjectMeta) -> Vec<Metric> {
    const KEYS: &[&str] = &["owner_kind", "owner_name", "owner_is_controller"];
    match m.owner_references.as_deref() {
        None | Some([]) => vec![mk(
            KEYS,
            vec![String::new(), String::new(), String::new()],
            1.0,
        )],
        Some(owners) => owners
            .iter()
            .map(|o| {
                let ctrl = match o.controller {
                    Some(c) => c.to_string(),
                    None => "false".to_owned(),
                };
                mk(KEYS, vec![o.kind.clone(), o.name.clone(), ctrl], 1.0)
            })
            .collect(),
    }
}

/// Go `strconv.FormatBool` of an optional bool, `""` when unset.
pub fn opt_bool_string(b: Option<bool>) -> String {
    b.map(|b| b.to_string()).unwrap_or_default()
}
