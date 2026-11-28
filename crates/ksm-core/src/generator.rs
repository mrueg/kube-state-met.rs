//! Port of `pkg/metric_generator`: a family generator turns an object into the
//! samples of one metric family; a filter decides which families are exposed.

use std::sync::Arc;

use crate::metric::{Family, MetricType};

/// Stability level of a family (`k8s.io/component-base/metrics.StabilityLevel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stability {
    Internal,
    Alpha,
    Beta,
    Stable,
}

/// Builds the samples of one family from an object of type `T`.
pub type GenerateFn<T> = Arc<dyn Fn(&T) -> Family<'_> + Send + Sync>;

/// One metric family of a resource (`generator.FamilyGenerator`).
pub struct FamilyGenerator<T> {
    pub name: String,
    pub help: String,
    pub metric_type: MetricType,
    pub stability: Stability,
    pub deprecated_version: String,
    pub opt_in: bool,
    pub generate: GenerateFn<T>,
}

impl<T> Clone for FamilyGenerator<T> {
    fn clone(&self) -> Self {
        FamilyGenerator {
            name: self.name.clone(),
            help: self.help.clone(),
            metric_type: self.metric_type,
            stability: self.stability,
            deprecated_version: self.deprecated_version.clone(),
            opt_in: self.opt_in,
            generate: Arc::clone(&self.generate),
        }
    }
}

impl<T> FamilyGenerator<T> {
    /// `generator.NewFamilyGeneratorWithStability`.
    pub fn new(
        name: impl Into<String>,
        help: &str,
        metric_type: MetricType,
        stability: Stability,
        deprecated_version: &str,
        generate: impl Fn(&T) -> Family<'_> + Send + Sync + 'static,
    ) -> Self {
        let help = if deprecated_version.is_empty() {
            help.to_owned()
        } else {
            format!("(Deprecated since {deprecated_version}) {help}")
        };
        FamilyGenerator {
            name: name.into(),
            help,
            metric_type,
            stability,
            deprecated_version: deprecated_version.to_owned(),
            opt_in: false,
            generate: Arc::new(generate),
        }
    }

    /// `generator.NewOptInFamilyGenerator`.
    pub fn opt_in(
        name: impl Into<String>,
        help: &str,
        metric_type: MetricType,
        stability: Stability,
        deprecated_version: &str,
        generate: impl Fn(&T) -> Family<'_> + Send + Sync + 'static,
    ) -> Self {
        let mut g = Self::new(
            name,
            help,
            metric_type,
            stability,
            deprecated_version,
            generate,
        );
        g.opt_in = true;
        g
    }

    /// The `# HELP` / `# TYPE` header of this family.
    pub fn header(&self) -> String {
        let mut h = String::with_capacity(self.name.len() * 2 + self.help.len() + 32);
        h.push_str("# HELP ");
        h.push_str(&self.name);
        h.push(' ');
        if self.stability == Stability::Stable {
            h.push_str("[STABLE] ");
        }
        escape_help(&mut h, &self.help);
        h.push('\n');
        h.push_str("# TYPE ");
        h.push_str(&self.name);
        h.push(' ');
        h.push_str(self.metric_type.as_str());
        h.push('\n');
        h
    }
}

fn escape_help(out: &mut String, help: &str) {
    for c in help.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
}

/// `generator.ExtractMetricFamilyHeaders`.
pub fn extract_headers<T>(families: &[FamilyGenerator<T>]) -> Vec<String> {
    families.iter().map(FamilyGenerator::header).collect()
}

/// Renders every family of `obj` into one buffer; `ends[i]` is where family
/// `i` ends (`generator.ComposeMetricGenFuncs` + `metricsstore.renderFamilies`).
pub fn render<T>(families: &[FamilyGenerator<T>], obj: &T) -> crate::store::Rendered {
    let generated: Vec<Family> = families.iter().map(|g| (g.generate)(obj)).collect();
    let hint: usize = generated
        .iter()
        .zip(families)
        .map(|(f, g)| f.size_hint(&g.name))
        .sum();
    let mut buf = Vec::with_capacity(hint);
    let mut ends = Vec::with_capacity(families.len());
    for (f, g) in generated.iter().zip(families) {
        f.append_bytes(&g.name, &mut buf);
        ends.push(u32::try_from(buf.len()).unwrap_or(u32::MAX));
    }
    crate::store::Rendered::new(buf, ends)
}

/// Decides whether a family is exposed (`generator.FamilyGeneratorFilter`).
pub trait FamilyFilter: Send + Sync {
    fn test(&self, name: &str, opt_in: bool) -> bool;
}

/// Passes a family only if every filter passes it.
#[derive(Default)]
pub struct CompositeFilter {
    pub filters: Vec<Box<dyn FamilyFilter>>,
}

impl FamilyFilter for CompositeFilter {
    fn test(&self, name: &str, opt_in: bool) -> bool {
        self.filters.iter().all(|f| f.test(name, opt_in))
    }
}

/// `generator.FilterFamilyGenerators`.
pub fn filter_families<T>(
    filter: &dyn FamilyFilter,
    families: Vec<FamilyGenerator<T>>,
) -> Vec<FamilyGenerator<T>> {
    families
        .into_iter()
        .filter(|f| filter.test(&f.name, f.opt_in))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metric::Metric;

    #[test]
    fn headers() {
        let g: FamilyGenerator<()> = FamilyGenerator::new(
            "kube_x_info",
            "Info\\about\nx.",
            MetricType::Gauge,
            Stability::Stable,
            "",
            |_| Family::default(),
        );
        assert_eq!(
            g.header(),
            "# HELP kube_x_info [STABLE] Info\\\\about\\nx.\n# TYPE kube_x_info gauge\n"
        );
        let d: FamilyGenerator<()> = FamilyGenerator::new(
            "kube_y",
            "Y.",
            MetricType::StateSet,
            Stability::Alpha,
            "v2.0.0",
            |_| Family::default(),
        );
        assert_eq!(
            d.header(),
            "# HELP kube_y (Deprecated since v2.0.0) Y.\n# TYPE kube_y stateset\n"
        );
    }

    #[test]
    fn render_offsets() {
        let fams: Vec<FamilyGenerator<u32>> = vec![
            FamilyGenerator::new(
                "a",
                "",
                MetricType::Gauge,
                Stability::Alpha,
                "",
                |n: &u32| Family::new(vec![Metric::value(f64::from(*n))]),
            ),
            FamilyGenerator::new("b", "", MetricType::Gauge, Stability::Alpha, "", |_| {
                Family::default()
            }),
        ];
        let r = render(&fams, &7);
        assert_eq!(r.family(0), b"a 7\n");
        assert_eq!(r.family(1), b"");
    }
}
