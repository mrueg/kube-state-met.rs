//! Port of `config.go` and `config_metrics_types.go`: the custom resource
//! state configuration (`kind: CustomResourceStateMetrics`).
//!
//! Maps use `BTreeMap`; Go iterates its maps in random order, so wherever the
//! order could matter (two label paths whose names sanitize to the same label,
//! the first of several bad paths in an error) this port takes sorted order.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::{flect, gostr, yamlgo};

/// `customResourceState`: prefixes the GVK labels and the default metric name prefix.
pub(crate) const CUSTOM_RESOURCE_STATE: &str = "customresource";

/// The top level configuration object (`Metrics`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Metrics {
    #[serde(default, deserialize_with = "yamlgo::zero_or")]
    pub spec: MetricsSpec,
}

/// `MetricsSpec`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MetricsSpec {
    /// The custom resources to monitor. A GVK may appear more than once.
    #[serde(default, deserialize_with = "yamlgo::seq")]
    pub resources: Vec<Resource>,
}

/// Labels added to all metrics (`Labels`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    /// `commonLabels`.
    pub common_labels: BTreeMap<String, String>,
    /// `labelsFromPath`.
    pub labels_from_path: BTreeMap<String, Vec<String>>,
}

impl Labels {
    /// `Labels.Merge`: `other` overwrites keys of `self`.
    pub fn merge(&self, other: &Labels) -> Labels {
        let mut common = self.common_labels.clone();
        let mut paths = self.labels_from_path.clone();
        common.extend(other.common_labels.clone());
        paths.extend(other.labels_from_path.clone());
        Labels {
            common_labels: common,
            labels_from_path: paths,
        }
    }
}

/// The group, version and kind of a resource (`GroupVersionKind`).
/// Version and kind may be `*` (resolved by CRD discovery).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
pub struct GroupVersionKind {
    #[serde(default, deserialize_with = "yamlgo::string")]
    pub group: String,
    #[serde(default, deserialize_with = "yamlgo::string")]
    pub version: String,
    #[serde(default, deserialize_with = "yamlgo::string")]
    pub kind: String,
}

impl fmt::Display for GroupVersionKind {
    /// `group_version_kind`, as Go's `GroupVersionKind.String`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}_{}", self.group, self.version, self.kind)
    }
}

/// One custom resource to generate metrics for (`Resource`).
#[derive(Clone, Debug, Default)]
pub struct Resource {
    /// Inline `commonLabels` / `labelsFromPath`, added to all metrics.
    pub labels: Labels,
    /// `metricNamePrefix`; `None` means the default `kube_customresource`,
    /// `Some("")` no prefix.
    pub metric_name_prefix: Option<String>,
    /// `groupVersionKind`.
    pub group_version_kind: GroupVersionKind,
    /// `resourcePlural`; defaults to the flect plural of the kind.
    pub resource_plural: String,
    /// `metrics`.
    pub metrics: Vec<Generator>,
    /// `errorLogV`.
    pub error_log_v: i32,
}

/// The name the task asked for; the same type as [`Resource`].
pub type ResourceConfig = Resource;

#[derive(Deserialize)]
struct RawResource {
    #[serde(
        rename = "commonLabels",
        default,
        deserialize_with = "yamlgo::string_map"
    )]
    common_labels: BTreeMap<String, String>,
    #[serde(
        rename = "labelsFromPath",
        default,
        deserialize_with = "yamlgo::paths_map"
    )]
    labels_from_path: BTreeMap<String, Vec<String>>,
    #[serde(
        rename = "metricNamePrefix",
        default,
        deserialize_with = "yamlgo::opt_string"
    )]
    metric_name_prefix: Option<String>,
    #[serde(
        rename = "groupVersionKind",
        default,
        deserialize_with = "yamlgo::zero_or"
    )]
    group_version_kind: GroupVersionKind,
    #[serde(
        rename = "resourcePlural",
        default,
        deserialize_with = "yamlgo::string"
    )]
    resource_plural: String,
    #[serde(default, deserialize_with = "yamlgo::seq")]
    metrics: Vec<Generator>,
    #[serde(rename = "errorLogV", default, deserialize_with = "yamlgo::go_i32")]
    error_log_v: i32,
}

impl<'de> Deserialize<'de> for Resource {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawResource::deserialize(d)?;
        Ok(Resource {
            labels: Labels {
                common_labels: r.common_labels,
                labels_from_path: r.labels_from_path,
            },
            metric_name_prefix: r.metric_name_prefix,
            group_version_kind: r.group_version_kind,
            resource_plural: r.resource_plural,
            metrics: r.metrics,
            error_log_v: r.error_log_v,
        })
    }
}

impl Resource {
    /// `GetMetricNamePrefix`.
    pub fn get_metric_name_prefix(&self) -> String {
        match &self.metric_name_prefix {
            None => format!("kube_{CUSTOM_RESOURCE_STATE}"),
            Some(p) => p.clone(),
        }
    }

    /// `GetResourceName`: `resourcePlural` if set, else the lowercased flect
    /// plural of the kind (the kubebuilder default).
    pub fn get_resource_name(&self) -> String {
        if !self.resource_plural.is_empty() {
            tracing::info!(
                resource = %self.group_version_kind,
                plural = %self.resource_plural,
                "Using custom resource plural"
            );
            return self.resource_plural.clone();
        }
        gostr::to_lower(&flect::pluralize(&self.group_version_kind.kind))
    }

    /// The configured GVK.
    pub fn gvk(&self) -> &GroupVersionKind {
        &self.group_version_kind
    }

    /// This resource with the GVK and plural CRD discovery resolved it to
    /// (`FromConfig` sets both on a copy for each resolved GVK).
    pub fn resolved(&self, gvk: GroupVersionKind, plural: String) -> Resource {
        let mut r = self.clone();
        r.group_version_kind = gvk;
        r.resource_plural = plural;
        r
    }
}

/// One metric family (`Generator`).
#[derive(Clone, Debug, Default)]
pub struct Generator {
    /// `each`: what to read from the resource.
    pub each: Metric,
    /// Inline `commonLabels` / `labelsFromPath`; override the resource's.
    pub labels: Labels,
    /// `name`, prefixed by the resource's metric name prefix.
    pub name: String,
    /// `help`.
    pub help: String,
    /// `errorLogV`; zero means the resource's.
    pub error_log_v: i32,
}

#[derive(Deserialize)]
struct RawGenerator {
    #[serde(default, deserialize_with = "yamlgo::zero_or")]
    each: Metric,
    #[serde(
        rename = "commonLabels",
        default,
        deserialize_with = "yamlgo::string_map"
    )]
    common_labels: BTreeMap<String, String>,
    #[serde(
        rename = "labelsFromPath",
        default,
        deserialize_with = "yamlgo::paths_map"
    )]
    labels_from_path: BTreeMap<String, Vec<String>>,
    #[serde(default, deserialize_with = "yamlgo::string")]
    name: String,
    #[serde(default, deserialize_with = "yamlgo::string")]
    help: String,
    #[serde(rename = "errorLogV", default, deserialize_with = "yamlgo::go_i32")]
    error_log_v: i32,
}

impl<'de> Deserialize<'de> for Generator {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let g = RawGenerator::deserialize(d)?;
        Ok(Generator {
            each: g.each,
            labels: Labels {
                common_labels: g.common_labels,
                labels_from_path: g.labels_from_path,
            },
            name: g.name,
            help: g.help,
            error_log_v: g.error_log_v,
        })
    }
}

/// `Metric`: the union of the metric types, selected by `type`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Metric {
    #[serde(default, deserialize_with = "yamlgo::opt")]
    pub gauge: Option<MetricGauge>,
    #[serde(rename = "stateSet", default, deserialize_with = "yamlgo::opt")]
    pub state_set: Option<MetricStateSet>,
    #[serde(default, deserialize_with = "yamlgo::opt")]
    pub info: Option<MetricInfo>,
    /// `type`: `gauge`, `stateset` or `info` (lowercased by [`from_config`]).
    #[serde(rename = "type", default, deserialize_with = "yamlgo::string")]
    pub metric_type: String,
}

/// `MetricMeta`: fields every metric type has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricMeta {
    /// `labelsFromPath`, relative to `path`.
    pub labels_from_path: BTreeMap<String, Vec<String>>,
    /// `path`.
    pub path: Vec<String>,
}

/// `MetricGauge`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricGauge {
    pub label_from_key: String,
    pub meta: MetricMeta,
    pub value_from: Vec<String>,
    pub nil_is_zero: bool,
}

/// `MetricInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricInfo {
    pub label_from_key: String,
    pub meta: MetricMeta,
}

/// `MetricStateSet`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricStateSet {
    pub meta: MetricMeta,
    pub list: Vec<String>,
    pub label_name: String,
    pub value_from: Vec<String>,
}

#[derive(Deserialize)]
struct RawGauge {
    #[serde(rename = "labelFromKey", default, deserialize_with = "yamlgo::string")]
    label_from_key: String,
    #[serde(
        rename = "labelsFromPath",
        default,
        deserialize_with = "yamlgo::paths_map"
    )]
    labels_from_path: BTreeMap<String, Vec<String>>,
    #[serde(default, deserialize_with = "yamlgo::string_seq")]
    path: Vec<String>,
    #[serde(rename = "valueFrom", default, deserialize_with = "yamlgo::string_seq")]
    value_from: Vec<String>,
    #[serde(rename = "nilIsZero", default, deserialize_with = "yamlgo::go_bool")]
    nil_is_zero: bool,
}

#[derive(Deserialize)]
struct RawInfo {
    #[serde(rename = "labelFromKey", default, deserialize_with = "yamlgo::string")]
    label_from_key: String,
    #[serde(
        rename = "labelsFromPath",
        default,
        deserialize_with = "yamlgo::paths_map"
    )]
    labels_from_path: BTreeMap<String, Vec<String>>,
    #[serde(default, deserialize_with = "yamlgo::string_seq")]
    path: Vec<String>,
}

#[derive(Deserialize)]
struct RawStateSet {
    #[serde(
        rename = "labelsFromPath",
        default,
        deserialize_with = "yamlgo::paths_map"
    )]
    labels_from_path: BTreeMap<String, Vec<String>>,
    #[serde(default, deserialize_with = "yamlgo::string_seq")]
    path: Vec<String>,
    #[serde(default, deserialize_with = "yamlgo::string_seq")]
    list: Vec<String>,
    #[serde(rename = "labelName", default, deserialize_with = "yamlgo::string")]
    label_name: String,
    #[serde(rename = "valueFrom", default, deserialize_with = "yamlgo::string_seq")]
    value_from: Vec<String>,
}

// Unknown keys are ignored, as yaml.v3 does without KnownFields.
impl<'de> Deserialize<'de> for MetricGauge {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawGauge::deserialize(d)?;
        Ok(MetricGauge {
            label_from_key: r.label_from_key,
            meta: MetricMeta {
                labels_from_path: r.labels_from_path,
                path: r.path,
            },
            value_from: r.value_from,
            nil_is_zero: r.nil_is_zero,
        })
    }
}

impl<'de> Deserialize<'de> for MetricInfo {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawInfo::deserialize(d)?;
        Ok(MetricInfo {
            label_from_key: r.label_from_key,
            meta: MetricMeta {
                labels_from_path: r.labels_from_path,
                path: r.path,
            },
        })
    }
}

impl<'de> Deserialize<'de> for MetricStateSet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RawStateSet::deserialize(d)?;
        Ok(MetricStateSet {
            meta: MetricMeta {
                labels_from_path: r.labels_from_path,
                path: r.path,
            },
            list: r.list,
            label_name: r.label_name,
            value_from: r.value_from,
        })
    }
}

/// Whether a YAML stream holds no document at all (only blank lines and
/// comments): yaml.v3's `Decoder.Decode` then returns `io.EOF`.
fn has_no_document(s: &str) -> bool {
    s.lines().all(|l| {
        let t = l.trim_start_matches([' ', '\t', '\r', '\u{feff}']);
        t.is_empty() || t.starts_with('#')
    })
}

/// yaml.v3 only accepts `%YAML 1.1` directives (libyaml also takes 1.2);
/// checks the directives before the first document and returns the 0-based
/// line of an incompatible one.
fn incompatible_yaml_directive(s: &str) -> Option<usize> {
    for (i, l) in s.lines().enumerate() {
        let t = l.trim_start_matches([' ', '\t', '\r', '\u{feff}']);
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some(rest) = t.strip_prefix("%YAML") else {
            if t.starts_with('%') {
                continue;
            }
            return None;
        };
        let version = rest.split_whitespace().next().unwrap_or("");
        if version != "1.1" {
            return Some(i);
        }
    }
    None
}

/// Decodes the first YAML document of `yaml_docs`, as one call of
/// yaml.v3's `Decoder.Decode` does, and applies `configOverrides`.
pub fn decode_metrics(yaml_docs: &str) -> Result<Metrics, String> {
    let wrap = |e: &dyn fmt::Display| format!("failed to parse Custom Resource State metrics: {e}");
    if has_no_document(yaml_docs) {
        return Err(wrap(&"EOF"));
    }
    if let Some(line) = incompatible_yaml_directive(yaml_docs) {
        let at = if line == 0 {
            String::new()
        } else {
            format!("line {line}: ")
        };
        return Err(wrap(&format!("yaml: {at}found incompatible YAML document")));
    }
    let first = || serde_yaml_ng::Deserializer::from_str(yaml_docs).next();
    let Some(doc) = first() else {
        return Err(wrap(&"EOF"));
    };
    // yaml.v3 parses the whole document before decoding it, so a syntax
    // error wins over a type error that comes earlier in the text.
    let tree = match Option::<serde_yaml_ng::Value>::deserialize(doc) {
        Ok(tree) => tree,
        Err(e) if yamlgo::is_syntax_error(&e) => return Err(wrap(&yamlgo::go_error(&e))),
        Err(_) => None,
    };
    let m: Option<Metrics> = match tree {
        // Merge keys (`<<`): yaml.v3 applies them; serde_yaml_ng only on a
        // Value, so merge there and decode the re-serialized document.
        Some(mut tree) if has_merge_key(&tree) => {
            tree.apply_merge()
                .map_err(|e| wrap(&yamlgo::go_error(&e)))?;
            let text = serde_yaml_ng::to_string(&tree).map_err(|e| wrap(&e))?;
            Option::deserialize(serde_yaml_ng::Deserializer::from_str(&text))
                .map_err(|e| wrap(&yamlgo::go_error(&e)))?
        }
        _ => {
            let Some(doc) = first() else {
                return Err(wrap(&"EOF"));
            };
            Option::deserialize(doc).map_err(|e| wrap(&yamlgo::go_error(&e)))?
        }
    };
    let mut m = m.unwrap_or_default();
    config_overrides(&mut m);
    Ok(m)
}

/// Whether any mapping in the tree has a `<<` key.
fn has_merge_key(v: &serde_yaml_ng::Value) -> bool {
    match v {
        serde_yaml_ng::Value::Mapping(m) => m
            .iter()
            .any(|(k, v)| k.as_str() == Some("<<") || has_merge_key(k) || has_merge_key(v)),
        serde_yaml_ng::Value::Sequence(s) => s.iter().any(has_merge_key),
        serde_yaml_ng::Value::Tagged(t) => has_merge_key(&t.value),
        _ => false,
    }
}

/// `configOverrides`: lowercases every metric type.
fn config_overrides(m: &mut Metrics) {
    for r in &mut m.spec.resources {
        for g in &mut r.metrics {
            g.each.metric_type = gostr::to_lower(&g.each.metric_type);
        }
    }
}

/// `FromConfig` without discovery: decodes the configuration and returns
/// its resources, still with the configured (possibly `*`) GVKs. The caller
/// resolves each through CRD discovery ([`Resource::resolved`]) and then
/// builds its families ([`Resource::families`]).
pub fn from_config(yaml_docs: &str) -> Result<Vec<Resource>, String> {
    Ok(decode_metrics(yaml_docs)?.spec.resources)
}
