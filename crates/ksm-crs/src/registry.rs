//! Port of `registry_factory.go` and `custom_resource_metrics.go`: compiles a
//! [`Resource`] into family generators over unstructured objects.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;

use ksm_core::labels::sanitize_label_name;
use ksm_core::{Family, FamilyGenerator, Metric, MetricType, Stability};
use serde_json::Value;

use crate::config::{Generator, MetricMeta, Resource};
use crate::path::{ValuePath, compile_path};
use crate::value::{go_fmt_v, go_type_name, non_nil, to_float64};

/// `customresource_group`.
pub const GVK_GROUP_LABEL: &str = "customresource_group";
/// `customresource_version`.
pub const GVK_VERSION_LABEL: &str = "customresource_version";
/// `customresource_kind`.
pub const GVK_KIND_LABEL: &str = "customresource_kind";

type LabelMap = BTreeMap<String, String>;
type PathMap = BTreeMap<String, ValuePath>;

/// `eachValue`.
#[derive(Clone, Debug)]
struct EachValue {
    labels: LabelMap,
    value: f64,
}

impl EachValue {
    /// `DefaultLabels`: fills in labels that are not set.
    fn default_labels(&mut self, defaults: &LabelMap) {
        for (k, v) in defaults {
            self.labels.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    /// `ToMetric`: label keys sorted.
    fn into_metric(self) -> Metric {
        let mut keys = Vec::with_capacity(self.labels.len());
        let mut values = Vec::with_capacity(self.labels.len());
        for (k, v) in self.labels {
            keys.push(Cow::Owned(k));
            values.push(v);
        }
        Metric::new(keys, values, self.value)
    }
}

/// `compiledCommon`.
#[derive(Clone, Debug)]
struct Common {
    label_from_path: PathMap,
    /// Only fill in labels an each value has not set.
    common_labels: LabelMap,
    path: ValuePath,
}

#[derive(Clone, Debug)]
struct Gauge {
    common: Common,
    label_from_key: String,
    value_from: ValuePath,
    nil_is_zero: bool,
}

#[derive(Clone, Debug)]
struct Info {
    common: Common,
    label_from_key: String,
}

#[derive(Clone, Debug)]
struct StateSet {
    common: Common,
    label_name: String,
    value_from: ValuePath,
    list: Vec<String>,
}

/// `compiledMetric`.
#[derive(Clone, Debug)]
enum Compiled {
    Gauge(Gauge),
    Info(Info),
    StateSet(StateSet),
}

/// `compiledFamily`.
#[derive(Clone, Debug)]
struct CompiledFamily {
    each: Compiled,
    labels: LabelMap,
    label_from_path: PathMap,
    name: String,
    help: String,
    error_log_v: i32,
}

fn compile_paths(paths: &BTreeMap<String, Vec<String>>) -> Result<PathMap, String> {
    let mut out = PathMap::new();
    for (k, v) in paths {
        out.insert(k.clone(), compile_path(v).map_err(|e| format!("{k}: {e}"))?);
    }
    Ok(out)
}

fn compile_common(c: &MetricMeta, common_labels: &LabelMap) -> Result<Common, String> {
    let path = compile_path(&c.path).map_err(|e| format!("path: {e}"))?;
    let label_from_path =
        compile_paths(&c.labels_from_path).map_err(|e| format!("labelsFromPath: {e}"))?;
    Ok(Common {
        label_from_path,
        common_labels: common_labels.clone(),
        path,
    })
}

/// `newCompiledMetric`.
fn new_compiled_metric(
    m: &crate::config::Metric,
    common_labels: &LabelMap,
) -> Result<Compiled, String> {
    match m.metric_type.as_str() {
        "gauge" => {
            let g = m
                .gauge
                .as_ref()
                .ok_or_else(|| "expected each.gauge to not be nil".to_owned())?;
            let common =
                compile_common(&g.meta, common_labels).map_err(|e| format!("each.gauge: {e}"))?;
            let value_from =
                compile_path(&g.value_from).map_err(|e| format!("each.gauge.valueFrom: {e}"))?;
            Ok(Compiled::Gauge(Gauge {
                common,
                label_from_key: g.label_from_key.clone(),
                value_from,
                nil_is_zero: g.nil_is_zero,
            }))
        }
        "info" => {
            let i = m
                .info
                .as_ref()
                .ok_or_else(|| "expected each.info to not be nil".to_owned())?;
            let common =
                compile_common(&i.meta, common_labels).map_err(|e| format!("each.info: {e}"))?;
            Ok(Compiled::Info(Info {
                common,
                label_from_key: i.label_from_key.clone(),
            }))
        }
        "stateset" => {
            let s = m
                .state_set
                .as_ref()
                .ok_or_else(|| "expected each.stateSet to not be nil".to_owned())?;
            let common = compile_common(&s.meta, common_labels)
                .map_err(|e| format!("each.stateSet: {e}"))?;
            let value_from =
                compile_path(&s.value_from).map_err(|e| format!("each.stateSet.valueFrom: {e}"))?;
            Ok(Compiled::StateSet(StateSet {
                common,
                label_name: s.label_name.clone(),
                value_from,
                list: s.list.clone(),
            }))
        }
        other => Err(format!("unknown metric type {other}")),
    }
}

/// `fullName`.
fn full_name(resource: &Resource, f: &Generator) -> String {
    let prefix = resource.get_metric_name_prefix();
    if prefix.is_empty() {
        f.name.clone()
    } else {
        format!("{prefix}_{}", f.name)
    }
}

/// `compileFamily`; `resource` already carries the GVK labels.
fn compile_family(f: &Generator, resource: &Resource) -> Result<CompiledFamily, String> {
    let labels = resource.labels.merge(&f.labels);
    if f.each.metric_type == "info" && !f.name.ends_with("_info") {
        tracing::info!(
            gvk = %resource.group_version_kind,
            name = %f.name,
            "Info metric does not have _info suffix"
        );
    }
    let each = new_compiled_metric(&f.each, &labels.common_labels)
        .map_err(|e| format!("compiling metric: {e}"))?;
    let label_from_path =
        compile_paths(&labels.labels_from_path).map_err(|e| format!("labelsFromPath: {e}"))?;
    let error_log_v = if f.error_log_v == 0 {
        resource.error_log_v
    } else {
        f.error_log_v
    };
    Ok(CompiledFamily {
        each,
        labels: labels.common_labels,
        label_from_path,
        name: full_name(resource, f),
        help: f.help.clone(),
        error_log_v,
    })
}

/// `compile`.
fn compile(resource: &Resource) -> Result<Vec<CompiledFamily>, String> {
    let mut resource = resource.clone();
    let gvk = resource.group_version_kind.clone();
    let common = &mut resource.labels.common_labels;
    common.insert(GVK_GROUP_LABEL.to_owned(), gvk.group);
    common.insert(GVK_VERSION_LABEL.to_owned(), gvk.version);
    common.insert(GVK_KIND_LABEL.to_owned(), gvk.kind);
    let mut families = Vec::with_capacity(resource.metrics.len());
    for f in &resource.metrics {
        families.push(compile_family(f, &resource).map_err(|e| format!("{}: {e}", f.name))?);
    }
    Ok(families)
}

/// `isCopiableLabelName`: a valid legacy label name, not reserved (`__`).
fn is_copiable_label_name(name: &str) -> bool {
    let b = name.as_bytes();
    let Some(&first) = b.first() else {
        return false;
    };
    (first == b'_' || first.is_ascii_alphabetic())
        && b.iter().all(|c| *c == b'_' || c.is_ascii_alphanumeric())
        && !name.starts_with("__")
}

fn is_star(k: &str) -> bool {
    k.starts_with('*') || k.ends_with('*')
}

/// `addPathLabels`.
fn add_path_labels(
    obj: Option<&Value>,
    labels: &PathMap,
    protected: Option<&LabelMap>,
    result: &mut LabelMap,
) {
    // "*" keys copy a whole object; they never override a label that is set
    // or protected, and later stars and keys win (walk both backwards and
    // keep the first write).
    for (star, path) in labels.iter().rev().filter(|(k, _)| is_star(k)) {
        let Some(Value::Object(kv)) = path.get(obj) else {
            continue;
        };
        let prefix = star.strip_suffix('*').unwrap_or("");
        let mut keys: Vec<&String> = kv.keys().collect();
        keys.sort();
        for k in keys.into_iter().rev() {
            let name = sanitize_label_name(&format!("{prefix}{k}")).into_owned();
            let set = result.contains_key(&name);
            let is_protected = protected.is_some_and(|p| p.contains_key(&name));
            if set || is_protected || !is_copiable_label_name(&name) {
                continue;
            }
            let v = kv.get(k.as_str()).map(go_fmt_v).unwrap_or_default();
            result.insert(name, v);
        }
    }
    for (k, path) in labels.iter().filter(|(k, _)| !is_star(k)) {
        let Some(value) = path.get(obj) else {
            continue;
        };
        result.insert(sanitize_label_name(k).into_owned(), go_fmt_v(value));
    }
}

/// `compareLabels`.
fn compare_labels(a: &LabelMap, b: &LabelMap) -> Ordering {
    for ((ak, av), (bk, bv)) in a.iter().zip(b.iter()) {
        let c = ak.cmp(bk).then_with(|| av.cmp(bv));
        if c != Ordering::Equal {
            return c;
        }
    }
    a.len().cmp(&b.len())
}

const LABEL_FROM_KEY_CONFLICT: &str =
    "generated labels conflict with labelsFromPath, consider renaming it";

impl Gauge {
    /// `compiledGauge.value`.
    fn value(&self, it: Option<&Value>) -> Result<Option<EachValue>, String> {
        let got = self.value_from.get(it);
        if got.is_none() && !self.nil_is_zero {
            return Ok(None);
        }
        let value =
            to_float64(got, self.nil_is_zero).map_err(|e| format!("{}: {e}", self.value_from))?;
        Ok(Some(EachValue {
            labels: LabelMap::new(),
            value,
        }))
    }

    fn values(&self, v: Option<&Value>) -> (Vec<EachValue>, Vec<String>) {
        let mut result = Vec::new();
        let mut errs = Vec::new();
        let path = &self.common.path;
        let mut on_error = |e: String| errs.push(format!("{path}: {e}"));
        match v {
            Some(Value::Object(iter)) => {
                let s_value_from = self.value_from.to_string();
                // A unit-length valueFrom ("[...]", not "[]", no comma).
                let extracted = (!s_value_from.contains(',') && s_value_from.len() > 2)
                    .then(|| &s_value_from[1..s_value_from.len() - 1]);
                for (key, it) in iter {
                    let it = non_nil(Some(it));
                    let mut ev = None;
                    if let Some(extracted) = extracted
                        && key == extracted
                        && it.is_some()
                    {
                        match to_float64(it, self.nil_is_zero) {
                            Ok(value) => {
                                ev = Some(EachValue {
                                    labels: LabelMap::new(),
                                    value,
                                });
                            }
                            Err(e) => {
                                on_error(format!("[{key}]: {e}"));
                                continue;
                            }
                        }
                    }
                    let mut ev = match ev {
                        Some(ev) => ev,
                        // Fall back to the regular path resolution; an
                        // unresolved value is skipped, errors included.
                        None => match self.value(it) {
                            Ok(Some(ev)) => ev,
                            _ => continue,
                        },
                    };
                    if ev.labels.contains_key(&self.label_from_key) {
                        on_error(format!(
                            "labelFromKey ({}) {LABEL_FROM_KEY_CONFLICT}",
                            self.label_from_key
                        ));
                        continue;
                    }
                    if !key.is_empty() && !self.label_from_key.is_empty() {
                        ev.labels.insert(self.label_from_key.clone(), key.clone());
                    }
                    add_path_labels(
                        it,
                        &self.common.label_from_path,
                        Some(&self.common.common_labels),
                        &mut ev.labels,
                    );
                    // Also evaluate relative to the parent.
                    add_path_labels(
                        v,
                        &self.common.label_from_path,
                        Some(&self.common.common_labels),
                        &mut ev.labels,
                    );
                    result.push(ev);
                }
            }
            Some(Value::Array(iter)) => {
                for (i, it) in iter.iter().enumerate() {
                    let it = non_nil(Some(it));
                    match self.value(it) {
                        Err(e) => on_error(format!("[{i}]: {e}")),
                        Ok(None) => {}
                        Ok(Some(mut ev)) => {
                            add_path_labels(
                                it,
                                &self.common.label_from_path,
                                Some(&self.common.common_labels),
                                &mut ev.labels,
                            );
                            result.push(ev);
                        }
                    }
                }
            }
            _ => match self.value(v) {
                Err(e) => on_error(e),
                Ok(None) => {}
                Ok(Some(mut ev)) => {
                    add_path_labels(
                        v,
                        &self.common.label_from_path,
                        Some(&self.common.common_labels),
                        &mut ev.labels,
                    );
                    result.push(ev);
                }
            },
        }
        (result, errs)
    }
}

impl Info {
    /// `compiledInfo.values`.
    fn values_of(&self, v: Option<&Value>) -> Vec<EachValue> {
        if v.is_none() {
            return Vec::new();
        }
        let mut labels = LabelMap::new();
        add_path_labels(
            v,
            &self.common.label_from_path,
            Some(&self.common.common_labels),
            &mut labels,
        );
        if labels.is_empty() {
            return Vec::new();
        }
        vec![EachValue { labels, value: 1.0 }]
    }

    fn values(&self, v: Option<&Value>) -> (Vec<EachValue>, Vec<String>) {
        let mut result = Vec::new();
        let mut errs = Vec::new();
        match v {
            Some(Value::Array(iter)) => {
                for obj in iter {
                    result.extend(self.values_of(non_nil(Some(obj))));
                }
            }
            Some(Value::Object(iter)) => {
                let value = self.values_of(v);
                for ev in &value {
                    if ev.labels.contains_key(&self.label_from_key) {
                        errs.push(format!(
                            "{}: labelFromKey ({}) {LABEL_FROM_KEY_CONFLICT}",
                            self.common.path, self.label_from_key
                        ));
                    }
                }
                for (key, it) in iter {
                    let mut labels = LabelMap::new();
                    if !key.is_empty() && !self.label_from_key.is_empty() {
                        labels.insert(self.label_from_key.clone(), key.clone());
                    }
                    add_path_labels(
                        non_nil(Some(it)),
                        &self.common.label_from_path,
                        Some(&self.common.common_labels),
                        &mut labels,
                    );
                    if !labels.is_empty() {
                        result.push(EachValue { labels, value: 1.0 });
                    }
                }
                result.extend(value);
            }
            _ => result = self.values_of(v),
        }
        (result, errs)
    }
}

impl StateSet {
    /// The path the value is read from, including valueFrom.
    fn full_value_path(&self) -> ValuePath {
        if self.value_from.is_empty() {
            return self.common.path.clone();
        }
        self.common.path.join(&self.value_from)
    }

    /// `compiledStateSet.values`.
    fn values_of(&self, v: Option<&Value>) -> Result<Vec<EachValue>, String> {
        let Some(raw) = self.value_from.get(v) else {
            // Not populated yet: no metrics, no error.
            return Ok(Vec::new());
        };
        let Value::String(value) = raw else {
            return Err(format!(
                "{}: expected value for path to be string, got {}",
                self.full_value_path(),
                go_type_name(Some(raw))
            ));
        };
        let mut result = Vec::with_capacity(self.list.len());
        for entry in &self.list {
            let mut labels = LabelMap::new();
            labels.insert(self.label_name.clone(), entry.clone());
            add_path_labels(
                v,
                &self.common.label_from_path,
                Some(&self.common.common_labels),
                &mut labels,
            );
            result.push(EachValue {
                labels,
                value: if value == entry { 1.0 } else { 0.0 },
            });
        }
        Ok(result)
    }

    fn values(&self, v: Option<&Value>) -> (Vec<EachValue>, Vec<String>) {
        if let Some(Value::Array(vs)) = v {
            let mut result = Vec::new();
            let mut errs = Vec::new();
            for obj in vs {
                match self.values_of(non_nil(Some(obj))) {
                    Ok(ev) => result.extend(ev),
                    Err(e) => errs.push(e),
                }
            }
            return (result, errs);
        }
        match self.values_of(v) {
            Ok(ev) => (ev, Vec::new()),
            Err(e) => (Vec::new(), vec![e]),
        }
    }
}

impl Compiled {
    fn metric_type(&self) -> MetricType {
        match self {
            Compiled::Gauge(_) => MetricType::Gauge,
            Compiled::Info(_) => MetricType::Info,
            Compiled::StateSet(_) => MetricType::StateSet,
        }
    }

    fn path(&self) -> &ValuePath {
        match self {
            Compiled::Gauge(g) => &g.common.path,
            Compiled::Info(i) => &i.common.path,
            Compiled::StateSet(s) => &s.common.path,
        }
    }

    fn values(&self, v: Option<&Value>) -> (Vec<EachValue>, Vec<String>) {
        match self {
            Compiled::Gauge(g) => g.values(v),
            Compiled::Info(i) => i.values(v),
            Compiled::StateSet(s) => s.values(v),
        }
    }
}

/// `scrapeValuesFor`: values in a consistent order.
fn scrape_values_for(e: &Compiled, obj: &Value) -> (Vec<EachValue>, Vec<String>) {
    let v = e.path().get(Some(obj));
    let (mut result, errs) = e.values(v);
    result.sort_by(|a, b| compare_labels(&a.labels, &b.labels));
    (result, errs)
}

impl CompiledFamily {
    /// `BaseLabels`.
    fn base_labels(&self, obj: &Value) -> LabelMap {
        let mut result = self.labels.clone();
        add_path_labels(
            Some(obj),
            &self.label_from_path,
            Some(&self.labels),
            &mut result,
        );
        result
    }

    /// `generate`.
    fn generate(&self, obj: &Value) -> Family {
        let base = self.base_labels(obj);
        let (values, errors) = scrape_values_for(&self.each, obj);
        for err in errors {
            if self.error_log_v == 0 {
                tracing::error!(family = %self.name, "{err}");
            } else {
                tracing::debug!(family = %self.name, v = self.error_log_v, "{err}");
            }
        }
        let metrics = values
            .into_iter()
            .map(|mut v| {
                v.default_labels(&base);
                v.into_metric()
            })
            .collect();
        Family::new(metrics)
    }
}

/// `famGen`.
fn fam_gen(f: CompiledFamily) -> FamilyGenerator<Value> {
    let name = f.name.clone();
    let help = f.help.clone();
    let metric_type = f.each.metric_type();
    let f = Arc::new(f);
    // Go leaves StabilityLevel empty, which renders like any non-stable level.
    FamilyGenerator::new(name, &help, metric_type, Stability::Alpha, "", move |obj| {
        f.generate(obj)
    })
}

impl Resource {
    /// `NewCustomResourceMetrics(...).MetricFamilyGenerators()`: the families
    /// of this resource over an unstructured object (the whole object as
    /// JSON). The error is the one `NewCustomResourceMetrics` returns.
    pub fn families(&self) -> Result<Vec<FamilyGenerator<Value>>, String> {
        let compiled = compile(self)?;
        let names: Vec<&str> = compiled.iter().map(|f| f.name.as_str()).collect();
        tracing::info!(familyNames = ?names, "Custom resource state added metrics");
        Ok(compiled.into_iter().map(fam_gen).collect())
    }

    /// The names of this resource's families, or the compile error.
    pub fn family_names(&self) -> Result<Vec<String>, String> {
        Ok(compile(self)?.into_iter().map(|f| f.name).collect())
    }
}
