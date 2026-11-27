//! Port of the label helpers in `internal/store/utils.go`: Kubernetes labels
//! and annotations to Prometheus labels, allow lists with wildcards, condition
//! samples and resource-version samples.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use crate::metric::{LabelKey, Metric};

/// Wildcard in label and annotation allow lists.
pub const LABEL_WILDCARD: &str = "*";
/// Most wildcards one allow-list entry may hold.
pub const MAX_PARTIAL_WILDCARDS_PER_LABEL: usize = 1;

/// Keys wildcards never match; only listed by their exact name.
const WILDCARD_EXCLUDED_KEYS: &[&str] = &["kubectl.kubernetes.io/last-applied-configuration"];

/// Condition statuses, in the order kube-state-metrics emits them.
pub const CONDITION_STATUSES: [&str; 3] = ["True", "False", "Unknown"];

/// `boolFloat64`.
pub fn bool_f64(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

/// `addConditionMetrics`: one sample per status (`true`, `false`, `unknown`),
/// 1 for the one `cs` equals. The status label is the only label added.
pub fn condition_metrics(cs: &str) -> Vec<Metric> {
    CONDITION_STATUSES
        .iter()
        .map(|s| Metric::new(Vec::new(), vec![s.to_lowercase()], bool_f64(cs == *s)))
        .collect()
}

/// Like [`condition_metrics`], with the label key set.
pub fn condition_metrics_keyed(key: &'static str, cs: &str) -> Vec<Metric> {
    CONDITION_STATUSES
        .iter()
        .map(|s| Metric::with(&[key], vec![s.to_lowercase()], bool_f64(cs == *s)))
        .collect()
}

/// `resourceVersionMetric`: one sample with the version as value, none when
/// it is not a float by Go's `strconv.ParseFloat`.
pub fn resource_version_metric(rv: Option<&str>) -> Vec<Metric> {
    match go_parse_float(rv.unwrap_or("")) {
        Some(v) => vec![Metric::value(v)],
        None => Vec::new(),
    }
}

/// Go's `strconv.ParseFloat(s, 64)`, `None` where Go returns an error.
pub fn go_parse_float(s: &str) -> Option<f64> {
    crate::gostd::parse_float(s).ok()
}

fn is_valid_label_char(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

/// Replaces every character outside `[a-zA-Z0-9_]` with `_`, one `_` per
/// character (not per byte).
pub fn sanitize_label_name(s: &str) -> Cow<'_, str> {
    if s.bytes().all(is_valid_label_char) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() && is_valid_label_char(c as u8) {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    Cow::Owned(out)
}

fn is_snake_boundary_start(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

/// `([a-z0-9])([A-Z])` -> `${1}_${2}` (non-overlapping), then lower-case.
pub fn to_snake_case(s: &str) -> Cow<'_, str> {
    let b = s.as_bytes();
    if !b.iter().any(u8::is_ascii_uppercase) {
        return Cow::Borrowed(s);
    }
    let mut out = Vec::with_capacity(b.len() + 4);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if i + 1 < b.len() && is_snake_boundary_start(c) && b[i + 1].is_ascii_uppercase() {
            out.push(c);
            out.push(b'_');
            out.push(b[i + 1].to_ascii_lowercase());
            i += 2;
            continue;
        }
        out.push(c.to_ascii_lowercase());
        i += 1;
    }
    // INFALLIBLE: only ASCII bytes were changed, so the bytes stay valid UTF-8.
    Cow::Owned(String::from_utf8(out).unwrap_or_default())
}

/// `labelName`: `prefix_` + the sanitized, snake-cased key.
pub fn label_name(prefix: &str, key: &str) -> String {
    let sanitized = sanitize_label_name(key);
    let snake = to_snake_case(&sanitized);
    let mut s = String::with_capacity(prefix.len() + 1 + snake.len());
    s.push_str(prefix);
    s.push('_');
    s.push_str(&snake);
    s
}

fn next_free_label_name(base: &str, count: &mut usize, used: &HashMap<String, usize>) -> String {
    loop {
        *count += 1;
        let candidate = format!("{base}_conflict{count}");
        if !used.contains_key(&candidate) {
            return candidate;
        }
    }
}

/// `mapToPrometheusLabels`: keys in sorted order, colliding names renamed to
/// `_conflict1`, `_conflict2`, ...
pub fn map_to_prometheus_labels<'a, I>(labels: I, prefix: &str) -> (Vec<LabelKey>, Vec<String>)
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    let mut sorted: Vec<(&String, &String)> = labels.into_iter().collect();
    sorted.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let mut keys: Vec<String> = Vec::with_capacity(sorted.len());
    let mut values: Vec<String> = Vec::with_capacity(sorted.len());
    let mut used: HashMap<String, usize> = HashMap::with_capacity(sorted.len());
    let mut conflicts: HashMap<String, usize> = HashMap::new();
    for (k, v) in sorted {
        let base = label_name(prefix, k);
        let mut name = base.clone();
        if let Some(count) = conflicts.get(&base).copied() {
            let mut count = count;
            name = next_free_label_name(&base, &mut count, &used);
            conflicts.insert(base, count);
        } else if let Some(&idx) = used.get(&name) {
            let mut count = 0;
            let renamed = next_free_label_name(&base, &mut count, &used);
            used.remove(&name);
            keys[idx].clone_from(&renamed);
            used.insert(renamed, idx);
            name = next_free_label_name(&base, &mut count, &used);
            conflicts.insert(base, count);
        }
        used.insert(name.clone(), keys.len());
        keys.push(name);
        values.push(v.clone());
    }
    (keys.into_iter().map(Cow::Owned).collect(), values)
}

/// Matches `key` against an allow-list entry with exactly one `*`, as Go's
/// `^QuoteMeta(before).*QuoteMeta(after)$` (`.` does not match a newline).
fn wildcard_match(pattern: &str, key: &str) -> bool {
    let Some((before, after)) = pattern.split_once('*') else {
        return pattern == key;
    };
    key.len() >= before.len() + after.len()
        && key.starts_with(before)
        && key.ends_with(after)
        && !key[before.len()..key.len() - after.len()].contains('\n')
}

/// `createPrometheusLabelKeysValues`: the allowed labels/annotations of an
/// object as Prometheus labels named `prefix_<key>`.
pub fn create_prometheus_label_keys_values(
    prefix: &str,
    all: Option<&BTreeMap<String, String>>,
    allow_list: &[String],
) -> (Vec<LabelKey>, Vec<String>) {
    let empty = BTreeMap::new();
    let all = all.unwrap_or(&empty);
    let mut allowed: BTreeMap<&String, &String> = BTreeMap::new();
    for (i, l) in allow_list.iter().enumerate() {
        if l == LABEL_WILDCARD {
            if i == 0 {
                let rest = &allow_list[1..];
                return map_to_prometheus_labels(
                    all.iter().filter(|(k, _)| {
                        !WILDCARD_EXCLUDED_KEYS.contains(&k.as_str()) || rest.contains(k)
                    }),
                    prefix,
                );
            }
            continue;
        }
        if !l.contains(LABEL_WILDCARD) {
            if let Some((k, v)) = all.get_key_value(l) {
                allowed.insert(k, v);
            }
            continue;
        }
        if l.matches(LABEL_WILDCARD).count() > MAX_PARTIAL_WILDCARDS_PER_LABEL {
            tracing_warn_pattern(l);
            continue;
        }
        for (k, v) in all {
            if WILDCARD_EXCLUDED_KEYS.contains(&k.as_str()) {
                continue;
            }
            if wildcard_match(l, k) {
                allowed.insert(k, v);
            }
        }
    }
    map_to_prometheus_labels(allowed, prefix)
}

fn tracing_warn_pattern(pattern: &str) {
    // Logged by the binary's subscriber when one is installed.
    eprintln_once(pattern);
}

fn eprintln_once(pattern: &str) {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Ok(mut s) = seen.lock()
        && s.insert(pattern.to_owned())
    {
        eprintln!(
            "kube-state-metrics: ignoring allowlist pattern {pattern:?}: exceeds maximum of {MAX_PARTIAL_WILDCARDS_PER_LABEL} wildcard(s)"
        );
    }
}

/// The `kube_<resource>_labels`/`_annotations` sample for an allow list:
/// `None` (no samples) when the list is empty.
pub fn allowed_labels_metric(
    prefix: &str,
    all: Option<&BTreeMap<String, String>>,
    allow_list: Option<&[String]>,
) -> Vec<Metric> {
    match allow_list {
        Some(list) if !list.is_empty() => {
            let (k, v) = create_prometheus_label_keys_values(prefix, all, list);
            vec![Metric::new(k, v, 1.0)]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn sanitize_and_snake() {
        assert_eq!(
            sanitize_label_name("app.kubernetes.io/name"),
            "app_kubernetes_io_name"
        );
        assert_eq!(sanitize_label_name("ü-x"), "__x");
        assert_eq!(to_snake_case("aBC"), "a_bc");
        assert_eq!(to_snake_case("FooBar"), "foo_bar");
        assert_eq!(to_snake_case("a1B"), "a1_b");
        assert_eq!(
            label_name("label", "app.k8s.io/OwnerRef"),
            "label_app_k8s_io_owner_ref"
        );
    }

    #[test]
    fn allow_lists() {
        let l = m(&[
            ("app", "x"),
            ("team/a", "1"),
            ("team/b", "2"),
            ("kubectl.kubernetes.io/last-applied-configuration", "{}"),
        ]);
        let (k, _) = create_prometheus_label_keys_values("label", Some(&l), &["*".into()]);
        assert_eq!(k.len(), 3);
        let (k, _) = create_prometheus_label_keys_values(
            "label",
            Some(&l),
            &[
                "*".into(),
                "kubectl.kubernetes.io/last-applied-configuration".into(),
            ],
        );
        assert_eq!(k.len(), 4);
        let (k, v) = create_prometheus_label_keys_values(
            "label",
            Some(&l),
            &["team/*".into(), "app".into()],
        );
        assert_eq!(k, vec!["label_app", "label_team_a", "label_team_b"]);
        assert_eq!(v, vec!["x", "1", "2"]);
        let (k, _) = create_prometheus_label_keys_values("label", Some(&l), &["*team*".into()]);
        assert!(k.is_empty());
        let (k, _) =
            create_prometheus_label_keys_values("label", Some(&l), &["app".into(), "*".into()]);
        assert_eq!(k, vec!["label_app"]);
    }

    #[test]
    fn parse_float() {
        assert_eq!(go_parse_float("10596"), Some(10596.0));
        assert_eq!(go_parse_float("BBBBB"), None);
        assert_eq!(go_parse_float(""), None);
        assert_eq!(go_parse_float("1e400"), None);
        assert_eq!(go_parse_float("+Inf"), Some(f64::INFINITY));
        assert_eq!(go_parse_float("1_000"), Some(1000.0));
    }
}
