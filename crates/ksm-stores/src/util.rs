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
            let metrics = f(obj);
            if keys.is_empty() || metrics.is_empty() {
                return Family::new(metrics);
            }
            Family::with_default_labels(metrics, keys, &values(obj))
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
        .map(|mut c| {
            let mut k: Vec<ksm_core::metric::LabelKey> = Vec::with_capacity(keys.len() + 1);
            k.extend(keys.iter().map(|k| std::borrow::Cow::Borrowed(*k)));
            k.push(std::borrow::Cow::Borrowed("status"));
            let mut v: Vec<String> = Vec::with_capacity(values.len() + 1);
            v.extend(values.iter().map(|s| (*s).to_owned()));
            v.append(&mut c.label_values);
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

/// Go `strconv.FormatBool` of an optional bool, `""` when unset.
pub fn opt_bool_string(b: Option<bool>) -> String {
    b.map(|b| b.to_string()).unwrap_or_default()
}

/// `v1.ResourceHugePagesPrefix`.
const RESOURCE_HUGE_PAGES_PREFIX: &str = "hugepages-";
/// `v1.ResourceAttachableVolumesPrefix`.
const RESOURCE_ATTACHABLE_VOLUMES_PREFIX: &str = "attachable-volumes-";
/// `v1.DefaultResourceRequestsPrefix`.
const DEFAULT_RESOURCE_REQUESTS_PREFIX: &str = "requests.";
/// `v1.ResourceDefaultNamespacePrefix`.
const RESOURCE_DEFAULT_NAMESPACE_PREFIX: &str = "kubernetes.io/";

/// `isHugePageResourceName`.
pub fn is_huge_page_resource_name(name: &str) -> bool {
    name.starts_with(RESOURCE_HUGE_PAGES_PREFIX)
}

/// `isAttachableVolumeResourceName`.
pub fn is_attachable_volume_resource_name(name: &str) -> bool {
    name.starts_with(RESOURCE_ATTACHABLE_VOLUMES_PREFIX)
}

/// `isNativeResource`.
fn is_native_resource(name: &str) -> bool {
    !name.contains('/') || name.contains(RESOURCE_DEFAULT_NAMESPACE_PREFIX)
}

/// `isExtendedResourceName`.
pub fn is_extended_resource_name(name: &str) -> bool {
    if is_native_resource(name) || name.starts_with(DEFAULT_RESOURCE_REQUESTS_PREFIX) {
        return false;
    }
    is_qualified_name(&format!("{DEFAULT_RESOURCE_REQUESTS_PREFIX}{name}"))
}

/// One DNS-1123 label: `[a-z0-9]([-a-z0-9]*[a-z0-9])?`.
fn is_dns1123_label_fmt(s: &str) -> bool {
    let b = s.as_bytes();
    let edge = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    match (b.first(), b.last()) {
        (Some(&f), Some(&l)) => edge(f) && edge(l) && b.iter().all(|&c| edge(c) || c == b'-'),
        _ => false,
    }
}

/// k8s `validation.IsDNS1123Subdomain` reports no error.
pub fn is_dns1123_subdomain(s: &str) -> bool {
    s.len() <= 253 && s.split('.').all(is_dns1123_label_fmt)
}

/// The name part of a label key: `([A-Za-z0-9][-A-Za-z0-9_.]*)?[A-Za-z0-9]`,
/// at most 63 bytes.
fn is_label_key_name(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() > 63 {
        return false;
    }
    match (b.first(), b.last()) {
        (Some(&f), Some(&l)) => {
            f.is_ascii_alphanumeric()
                && l.is_ascii_alphanumeric()
                && b.iter()
                    .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        }
        _ => false,
    }
}

/// k8s `validation.IsQualifiedName` reports no error: an optional DNS-1123
/// subdomain prefix and `/`, then a label-key name.
pub fn is_qualified_name(value: &str) -> bool {
    let parts: Vec<&str> = value.split('/').collect();
    match parts.as_slice() {
        [name] => is_label_key_name(name),
        [prefix, name] => {
            !prefix.is_empty() && is_dns1123_subdomain(prefix) && is_label_key_name(name)
        }
        _ => false,
    }
}

/// The IP family (`"4"` or `"6"`) of an address as Go's `netip.ParseAddr`
/// followed by `Unmap()` sees it; `None` where Go fails to parse it.
pub fn go_ip_family(s: &str) -> Option<&'static str> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    // Go picks the parser by the first '.', ':' or '%' in the string.
    match s.bytes().find(|c| matches!(c, b'.' | b':' | b'%'))? {
        b'.' => s.parse::<Ipv4Addr>().ok().map(|_| "4"),
        b':' => {
            let addr = match s.split_once('%') {
                Some((_, "")) => return None,
                Some((a, _)) => a,
                None => s,
            };
            let a = addr.parse::<Ipv6Addr>().ok()?;
            Some(if a.to_ipv4_mapped().is_some() {
                "4"
            } else {
                "6"
            })
        }
        _ => None,
    }
}

/// The `_owner` samples: `owner_kind`, `owner_name` and, with
/// `with_controller`, `owner_is_controller` ("false" when unset). Without
/// owners, one sample with empty values.
pub fn owner_metrics(m: &ObjectMeta, with_controller: bool) -> Vec<Metric> {
    let keys: &[&'static str] = if with_controller {
        &["owner_kind", "owner_name", "owner_is_controller"]
    } else {
        &["owner_kind", "owner_name"]
    };
    let owners = m.owner_references.as_deref().unwrap_or_default();
    if owners.is_empty() {
        return vec![mk(keys, vec![String::new(); keys.len()], 1.0)];
    }
    owners
        .iter()
        .map(|o| {
            let mut values = vec![o.kind.clone(), o.name.clone()];
            if with_controller {
                values.push(o.controller.unwrap_or(false).to_string());
            }
            mk(keys, values, 1.0)
        })
        .collect()
}

/// `intstr.GetScaledValueFromIntOrPercent`: an int, or a percentage of
/// `total` rounded up or down; `None` where Go returns an error.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "Go converts between int and float64 the same way"
)]
pub fn scaled_value_from_int_or_percent(
    v: Option<&k8s_openapi::apimachinery::pkg::util::intstr::IntOrString>,
    total: i64,
    round_up: bool,
) -> Option<i64> {
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    match v? {
        IntOrString::Int(i) => Some(i64::from(*i)),
        IntOrString::String(s) => {
            let value: i64 = s.strip_suffix('%')?.parse().ok()?;
            let scaled = value as f64 * total as f64 / 100.0;
            Some(if round_up {
                scaled.ceil()
            } else {
                scaled.floor()
            } as i64)
        }
    }
}

/// An optional count as a sample, none when unset (Go checks `!= nil`).
pub fn opt_i32_metric(v: Option<i32>) -> Vec<Metric> {
    v.map(|v| vec![Metric::value(f64::from(v))])
        .unwrap_or_default()
}

/// A time as a sample, none when unset (Go checks `!= nil`, not `IsZero`).
pub fn opt_time_metric(t: Option<&Time>) -> Vec<Metric> {
    t.map(|t| vec![Metric::value(time_f64(t))])
        .unwrap_or_default()
}

/// `float64` of an `int64` (Go's conversion).
#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts int64 to float64 the same way"
)]
pub fn i64_f64(v: i64) -> f64 {
    v as f64
}

/// `float64` of a length or count (Go's `float64(len(x))`).
#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts int to float64 the same way"
)]
pub fn usize_f64(n: usize) -> f64 {
    n as f64
}

/// The `*_metadata_generation` sample: `metadata.generation`, 0 when unset.
pub fn generation_metric(m: &ObjectMeta) -> Vec<Metric> {
    vec![Metric::value(i64_f64(m.generation.unwrap_or(0)))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_resource_names() {
        assert!(is_extended_resource_name("nvidia.com/gpu"));
        assert!(is_extended_resource_name("example.com/a_b.c"));
        assert!(!is_extended_resource_name("cpu"));
        assert!(!is_extended_resource_name("kubernetes.io/foo"));
        assert!(!is_extended_resource_name("requests.foo/bar"));
        assert!(!is_extended_resource_name("Example.com/gpu"));
        assert!(!is_extended_resource_name("a/b/c"));
        assert!(!is_extended_resource_name("example.com/-gpu"));
        assert!(!is_extended_resource_name("example..com/gpu"));
        assert!(!is_extended_resource_name("example.com/"));
    }

    #[test]
    fn resource_name_prefixes() {
        assert!(is_huge_page_resource_name("hugepages-2Mi"));
        assert!(is_attachable_volume_resource_name(
            "attachable-volumes-aws-ebs"
        ));
        assert!(!is_huge_page_resource_name("memory"));
    }
}
