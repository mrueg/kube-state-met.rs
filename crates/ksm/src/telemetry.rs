//! The self-metrics registry of the telemetry port (Go: a client_golang
//! registry). Hand-written so that the text output matches promhttp's: families
//! sorted by name, label pairs sorted by name (`le` last in histograms), Go's
//! float formatting (docs/port/research/telemetry.md).

use std::collections::BTreeMap;
use std::sync::Arc;

use ksm_core::metric::{escape_string, write_float};
use parking_lot::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counter,
    Gauge,
    Histogram,
}

/// Label values in label-name order.
type Series = Vec<String>;

struct HistogramData {
    buckets: Vec<u64>,
    sum: f64,
    count: u64,
}

enum Values {
    Scalar(BTreeMap<Series, f64>),
    Histogram {
        bounds: Vec<f64>,
        data: BTreeMap<Series, HistogramData>,
    },
}

struct FamilyData {
    help: String,
    kind: Kind,
    /// Label names in declaration order, const labels included.
    labels: Vec<String>,
    /// Const label values, prepended to every series.
    const_values: Vec<String>,
    values: Values,
}

/// A set of metric families.
#[derive(Default, Clone)]
pub struct Registry {
    families: Arc<Mutex<BTreeMap<String, FamilyData>>>,
}

/// A labelled counter or gauge family.
#[derive(Clone)]
pub struct Vector {
    reg: Registry,
    name: String,
}

/// A labelled histogram family.
#[derive(Clone)]
pub struct HistogramVec {
    reg: Registry,
    name: String,
}

/// `prometheus.DefBuckets`.
pub const DEF_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    fn register(
        &self,
        name: &str,
        help: &str,
        kind: Kind,
        labels: &[&str],
        const_labels: &[(&str, &str)],
        values: Values,
    ) {
        let mut l: Vec<String> = const_labels.iter().map(|(k, _)| (*k).to_owned()).collect();
        l.extend(labels.iter().map(|s| (*s).to_owned()));
        self.families.lock().insert(
            name.to_owned(),
            FamilyData {
                help: help.to_owned(),
                kind,
                labels: l,
                const_values: const_labels.iter().map(|(_, v)| (*v).to_owned()).collect(),
                values,
            },
        );
    }

    pub fn counter_vec(&self, name: &str, help: &str, labels: &[&str]) -> Vector {
        self.register(
            name,
            help,
            Kind::Counter,
            labels,
            &[],
            Values::Scalar(BTreeMap::new()),
        );
        Vector {
            reg: self.clone(),
            name: name.to_owned(),
        }
    }

    pub fn gauge_vec(&self, name: &str, help: &str, labels: &[&str]) -> Vector {
        self.register(
            name,
            help,
            Kind::Gauge,
            labels,
            &[],
            Values::Scalar(BTreeMap::new()),
        );
        Vector {
            reg: self.clone(),
            name: name.to_owned(),
        }
    }

    /// A gauge without labels; exposed with value 0 from the start, as Go's.
    pub fn gauge(&self, name: &str, help: &str) -> Vector {
        let v = self.gauge_vec(name, help, &[]);
        v.set(&[], 0.0);
        v
    }

    /// A counter without labels; exposed with value 0 from the start, as Go's.
    pub fn counter(&self, name: &str, help: &str) -> Vector {
        let v = self.counter_vec(name, help, &[]);
        v.add(&[], 0.0);
        v
    }

    pub fn histogram_vec(
        &self,
        name: &str,
        help: &str,
        labels: &[&str],
        const_labels: &[(&str, &str)],
        bounds: &[f64],
    ) -> HistogramVec {
        self.register(
            name,
            help,
            Kind::Histogram,
            labels,
            const_labels,
            Values::Histogram {
                bounds: bounds.to_vec(),
                data: BTreeMap::new(),
            },
        );
        HistogramVec {
            reg: self.clone(),
            name: name.to_owned(),
        }
    }

    /// A constant gauge series (build info).
    pub fn const_gauge(&self, name: &str, help: &str, labels: &[(&str, &str)], value: f64) {
        let names: Vec<&str> = labels.iter().map(|(k, _)| *k).collect();
        let v = self.gauge_vec(name, help, &names);
        let values: Vec<&str> = labels.iter().map(|(_, v)| *v).collect();
        v.set(&values, value);
    }

    /// Number of series in every family (`util.GatherAndCount`).
    pub fn count(&self) -> usize {
        self.families
            .lock()
            .values()
            .map(|f| match &f.values {
                Values::Scalar(m) => m.len(),
                Values::Histogram { data, .. } => data.len(),
            })
            .sum()
    }

    /// The text exposition format (0.0.4) of every family with series, or
    /// only of those named in `only` (promhttp's `name[]` query filter).
    pub fn render(
        &self,
        extra: &[crate::process::ExtraFamily],
        only: Option<&std::collections::HashSet<String>>,
    ) -> Vec<u8> {
        let fams = self.families.lock();
        let mut out = Vec::with_capacity(4096);
        // Extra families (process metrics) are merged into the sorted order.
        let mut names: Vec<&str> = fams.keys().map(String::as_str).collect();
        names.extend(extra.iter().map(|(n, ..)| n.as_str()));
        names.retain(|n| only.is_none_or(|o| o.contains(*n)));
        names.sort_unstable();
        for name in names {
            if let Some(f) = fams.get(name) {
                render_family(&mut out, name, f);
            } else if let Some((_, help, ty, series)) = extra.iter().find(|(n, ..)| n == name) {
                if series.is_empty() {
                    continue;
                }
                header(&mut out, name, help, ty);
                for (labels, v) in series {
                    sample(
                        &mut out,
                        name,
                        labels.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                        *v,
                    );
                }
            }
        }
        out
    }
}

fn header(out: &mut Vec<u8>, name: &str, help: &str, ty: &str) {
    out.extend_from_slice(b"# HELP ");
    out.extend_from_slice(name.as_bytes());
    out.push(b' ');
    for c in help.bytes() {
        match c {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            c => out.push(c),
        }
    }
    out.extend_from_slice(b"\n# TYPE ");
    out.extend_from_slice(name.as_bytes());
    out.push(b' ');
    out.extend_from_slice(ty.as_bytes());
    out.push(b'\n');
}

fn sample<'a>(
    out: &mut Vec<u8>,
    name: &str,
    labels: impl Iterator<Item = (&'a str, &'a str)>,
    v: f64,
) {
    out.extend_from_slice(name.as_bytes());
    let mut first = true;
    for (k, val) in labels {
        out.push(if first { b'{' } else { b',' });
        first = false;
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(b"=\"");
        escape_string(out, val);
        out.push(b'"');
    }
    if !first {
        out.push(b'}');
    }
    out.push(b' ');
    write_float(out, v);
    out.push(b'\n');
}

/// Labels of one series sorted by name.
fn sorted_labels<'a>(f: &'a FamilyData, series: &'a [String]) -> Vec<(&'a str, &'a str)> {
    let mut l: Vec<(&str, &str)> = f
        .labels
        .iter()
        .map(String::as_str)
        .zip(
            f.const_values
                .iter()
                .chain(series.iter())
                .map(String::as_str),
        )
        .collect();
    l.sort_by(|a, b| a.0.cmp(b.0));
    l
}

fn render_family(out: &mut Vec<u8>, name: &str, f: &FamilyData) {
    match &f.values {
        Values::Scalar(m) => {
            if m.is_empty() {
                return;
            }
            header(
                out,
                name,
                &f.help,
                if f.kind == Kind::Counter {
                    "counter"
                } else {
                    "gauge"
                },
            );
            let mut rows: Vec<(Vec<(&str, &str)>, f64)> =
                m.iter().map(|(s, v)| (sorted_labels(f, s), *v)).collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            for (labels, v) in rows {
                sample(out, name, labels.into_iter(), v);
            }
        }
        Values::Histogram { bounds, data } => {
            if data.is_empty() {
                return;
            }
            header(out, name, &f.help, "histogram");
            let mut rows: Vec<(Vec<(&str, &str)>, &HistogramData)> =
                data.iter().map(|(s, d)| (sorted_labels(f, s), d)).collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            let bucket = format!("{name}_bucket");
            for (labels, d) in rows {
                let mut cumulative = 0;
                for (i, b) in bounds.iter().enumerate() {
                    cumulative += d.buckets[i];
                    let le = ksm_core::metric::go_float_string(*b);
                    sample(
                        out,
                        &bucket,
                        labels.iter().copied().chain([("le", le.as_str())]),
                        cumulative as f64,
                    );
                }
                sample(
                    out,
                    &bucket,
                    labels.iter().copied().chain([("le", "+Inf")]),
                    d.count as f64,
                );
                sample(out, &format!("{name}_sum"), labels.iter().copied(), d.sum);
                sample(
                    out,
                    &format!("{name}_count"),
                    labels.iter().copied(),
                    d.count as f64,
                );
            }
        }
    }
}

impl Vector {
    fn update(&self, values: &[&str], f: impl FnOnce(&mut f64)) {
        let mut fams = self.reg.families.lock();
        if let Some(FamilyData {
            values: Values::Scalar(m),
            ..
        }) = fams.get_mut(&self.name)
        {
            let key: Series = values.iter().map(|s| (*s).to_owned()).collect();
            f(m.entry(key).or_insert(0.0));
        }
    }

    pub fn inc(&self, values: &[&str]) {
        self.add(values, 1.0);
    }

    pub fn add(&self, values: &[&str], v: f64) {
        self.update(values, |x| *x += v);
    }

    pub fn set(&self, values: &[&str], v: f64) {
        self.update(values, |x| *x = v);
    }

    /// Removes every series (`Reset`).
    pub fn reset(&self) {
        let mut fams = self.reg.families.lock();
        if let Some(FamilyData {
            values: Values::Scalar(m),
            ..
        }) = fams.get_mut(&self.name)
        {
            m.clear();
        }
    }
}

impl HistogramVec {
    pub fn observe(&self, values: &[&str], v: f64) {
        let mut fams = self.reg.families.lock();
        if let Some(FamilyData {
            values: Values::Histogram { bounds, data },
            ..
        }) = fams.get_mut(&self.name)
        {
            let key: Series = values.iter().map(|s| (*s).to_owned()).collect();
            let n = bounds.len();
            let d = data.entry(key).or_insert_with(|| HistogramData {
                buckets: vec![0; n],
                sum: 0.0,
                count: 0,
            });
            if let Some(i) = bounds.iter().position(|b| v <= *b) {
                d.buckets[i] += 1;
            }
            d.sum += v;
            d.count += 1;
        }
    }
}

/// The list/watch self-metrics (Go: `pkg/watch.ListWatchMetrics`).
#[derive(Clone)]
pub struct ListWatchMetrics {
    pub watch_total: Vector,
    pub list_total: Vector,
    pub list_objects: Vector,
    pub list_objects_limit: Vector,
    pub objects_dropped: Vector,
}

impl ListWatchMetrics {
    pub fn new(r: &Registry) -> Self {
        ListWatchMetrics {
            watch_total: r.counter_vec(
                "kube_state_metrics_watch_total",
                "Number of total resource watch calls in kube-state-metrics",
                &["result", "resource"],
            ),
            list_total: r.counter_vec(
                "kube_state_metrics_list_total",
                "Number of total resource list calls in kube-state-metrics",
                &["result", "resource"],
            ),
            list_objects: r.gauge_vec("kube_state_metrics_list_objects", "Number of resources listed in kube-state-metrics", &["resource"]),
            list_objects_limit: r.gauge_vec(
                "kube_state_metrics_list_objects_limit",
                "Number of resource list limit in kube-state-metrics",
                &["resource"],
            ),
            objects_dropped: r.counter_vec(
                "kube_state_metrics_object_limit_dropped_total",
                "Number of add/update events for objects not stored because the object limit was reached",
                &["resource"],
            ),
        }
    }
}

/// The sharding self-metrics (Go: `pkg/sharding.Metrics`).
#[derive(Clone)]
pub struct ShardingMetrics {
    pub ordinal: Vector,
    pub total: Vector,
}

impl ShardingMetrics {
    pub fn new(r: &Registry) -> Self {
        ShardingMetrics {
            ordinal: r.gauge_vec(
                "kube_state_metrics_shard_ordinal",
                "Current sharding ordinal/index of this instance",
                &["shard_ordinal"],
            ),
            total: r.gauge(
                "kube_state_metrics_total_shards",
                "Number of total shards this instance is aware of",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_sorted() {
        let r = Registry::new();
        let c = r.counter_vec("b_total", "B.", &["z", "a"]);
        c.inc(&["1", "2"]);
        let g = r.gauge("a_gauge", "A.");
        g.set(&[], 54_304_768.0);
        let h = r.histogram_vec(
            "h",
            "H.",
            &["method"],
            &[("handler", "metrics")],
            &[0.1, 1.0],
        );
        h.observe(&["get"], 0.05);
        let out = String::from_utf8(r.render(&[], None)).unwrap();
        assert_eq!(
            out,
            "# HELP a_gauge A.\n# TYPE a_gauge gauge\na_gauge 5.4304768e+07\n\
             # HELP b_total B.\n# TYPE b_total counter\nb_total{a=\"2\",z=\"1\"} 1\n\
             # HELP h H.\n# TYPE h histogram\n\
             h_bucket{handler=\"metrics\",method=\"get\",le=\"0.1\"} 1\n\
             h_bucket{handler=\"metrics\",method=\"get\",le=\"1\"} 1\n\
             h_bucket{handler=\"metrics\",method=\"get\",le=\"+Inf\"} 1\n\
             h_sum{handler=\"metrics\",method=\"get\"} 0.05\n\
             h_count{handler=\"metrics\",method=\"get\"} 1\n"
        );
    }
}
