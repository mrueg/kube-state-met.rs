//! Port of `pkg/metric`: one sample, a family of samples, and their text
//! exposition, byte for byte as kube-state-metrics writes it.

use std::borrow::Cow;

/// Metric type of a family (`pkg/metric.Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetricType {
    Gauge,
    Info,
    StateSet,
    Counter,
}

impl MetricType {
    pub fn as_str(self) -> &'static str {
        match self {
            MetricType::Gauge => "gauge",
            MetricType::Info => "info",
            MetricType::StateSet => "stateset",
            MetricType::Counter => "counter",
        }
    }

    /// Parses a type name as it appears in custom resource state configs.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "gauge" => MetricType::Gauge,
            "info" => MetricType::Info,
            "stateset" => MetricType::StateSet,
            "counter" => MetricType::Counter,
            _ => return None,
        })
    }
}

/// A label key: almost always a static name, sometimes built at run time
/// (`label_app`, `annotation_team`, CRS labels).
pub type LabelKey = Cow<'static, str>;

/// One sample: label keys, label values and a value (`pkg/metric.Metric`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metric {
    pub label_keys: Vec<LabelKey>,
    pub label_values: Vec<String>,
    pub value: f64,
}

impl Metric {
    pub fn new(label_keys: Vec<LabelKey>, label_values: Vec<String>, value: f64) -> Self {
        Metric {
            label_keys,
            label_values,
            value,
        }
    }

    /// A sample without labels.
    pub fn value(value: f64) -> Self {
        Metric {
            value,
            ..Default::default()
        }
    }

    /// A sample with static label keys.
    pub fn with(keys: &[&'static str], values: Vec<String>, value: f64) -> Self {
        Metric {
            label_keys: keys.iter().map(|k| Cow::Borrowed(*k)).collect(),
            label_values: values,
            value,
        }
    }

    /// Writes `{labels} value\n` (the family name is written by the caller).
    ///
    /// Panics when keys and values differ in length, as the Go original does.
    pub fn write(&self, buf: &mut Vec<u8>) {
        self.write_prefixed(buf, &[]);
    }

    /// [`write`](Self::write) with `prefix` (rendered `k="v"` pairs, see
    /// [`Family::with_default_labels`]) before this sample's own labels.
    fn write_prefixed(&self, buf: &mut Vec<u8>, prefix: &[u8]) {
        assert!(
            self.label_keys.len() == self.label_values.len(),
            "expected labelKeys {:?} to be of same length as labelValues {:?}",
            self.label_keys,
            self.label_values
        );
        if !prefix.is_empty() || !self.label_keys.is_empty() {
            buf.push(b'{');
            buf.extend_from_slice(prefix);
            let mut separator = !prefix.is_empty();
            for (k, v) in self.label_keys.iter().zip(&self.label_values) {
                if separator {
                    buf.push(b',');
                }
                write_label(buf, k, v);
                separator = true;
            }
            buf.push(b'}');
        }
        buf.push(b' ');
        write_float(buf, self.value);
        buf.push(b'\n');
    }
}

/// `k="v"` with the value escaped.
fn write_label(buf: &mut Vec<u8>, k: &str, v: &str) {
    buf.extend_from_slice(k.as_bytes());
    buf.extend_from_slice(b"=\"");
    escape_string(buf, v);
    buf.push(b'"');
}

/// Escapes `\`, newline and `"` in a label value.
pub fn escape_string(buf: &mut Vec<u8>, v: &str) {
    if !v.bytes().any(|c| c == b'\\' || c == b'\n' || c == b'"') {
        buf.extend_from_slice(v.as_bytes());
        return;
    }
    for c in v.bytes() {
        match c {
            b'\\' => buf.extend_from_slice(b"\\\\"),
            b'\n' => buf.extend_from_slice(b"\\n"),
            b'"' => buf.extend_from_slice(b"\\\""),
            c => buf.push(c),
        }
    }
}

/// Writes a float the way kube-state-metrics does: `1`, `0`, `-1`, `NaN`,
/// `+Inf`, `-Inf`, else Go's `strconv.FormatFloat(f, 'g', -1, 64)`.
pub fn write_float(buf: &mut Vec<u8>, f: f64) {
    if f == 1.0 {
        buf.push(b'1');
    } else if f == 0.0 {
        buf.push(b'0');
    } else if f == -1.0 {
        buf.extend_from_slice(b"-1");
    } else if f.is_nan() {
        buf.extend_from_slice(b"NaN");
    } else if f == f64::INFINITY {
        buf.extend_from_slice(b"+Inf");
    } else if f == f64::NEG_INFINITY {
        buf.extend_from_slice(b"-Inf");
    } else {
        format_go_g(buf, f);
    }
}

/// Go's `strconv.FormatFloat(f, 'g', -1, 64)` for finite `f`.
///
/// Shortest round-trip digits come from Rust's `{:e}` formatting (same digits
/// as Go's shortest algorithm); the layout follows Go: `%e` when the decimal
/// exponent is `< -4` or `>= 6` (the precision Go uses for shortest `%g`),
/// `%f` otherwise; exponents have a sign and at least two digits.
pub fn format_go_g(buf: &mut Vec<u8>, f: f64) {
    use std::io::Write as _;
    if f.is_nan() {
        buf.extend_from_slice(b"NaN");
        return;
    }
    if f.is_infinite() {
        buf.extend_from_slice(if f > 0.0 { b"+Inf" } else { b"-Inf" });
        return;
    }
    if f.is_sign_negative() {
        buf.push(b'-');
    }
    let a = f.abs();
    if a == 0.0 {
        buf.push(b'0');
        return;
    }
    let mut sci = [0u8; 40];
    let mut cursor = std::io::Cursor::new(&mut sci[..]);
    // INFALLIBLE: a formatted f64 is at most 24 bytes, the buffer holds 40.
    let _ = write!(cursor, "{a:e}");
    let len = cursor.position() as usize;
    let s = &sci[..len];
    let e_pos = s.iter().position(|&c| c == b'e').unwrap_or(len);
    let mantissa = &s[..e_pos];
    let exp: i32 = std::str::from_utf8(&s[e_pos + 1..])
        .ok()
        .and_then(|e| e.parse().ok())
        .unwrap_or(0);
    let mut digits = [0u8; 24];
    let mut nd = 0;
    for &c in mantissa {
        if c != b'.' {
            digits[nd] = c;
            nd += 1;
        }
    }
    let tie = go_tie_digits(a, nd, exp);
    let digits: &[u8] = match &tie {
        Some(t) => t.as_bytes(),
        None => &digits[..nd],
    };
    let nd = digits.len();
    let dp = exp + 1; // position of the decimal point relative to the digits
    if !(-4..6).contains(&exp) {
        buf.push(digits[0]);
        if nd > 1 {
            buf.push(b'.');
            buf.extend_from_slice(&digits[1..]);
        }
        buf.push(b'e');
        let (sign, e) = if exp < 0 { (b'-', -exp) } else { (b'+', exp) };
        buf.push(sign);
        if e < 10 {
            buf.push(b'0');
        }
        let _ = write!(buf, "{e}");
    } else if dp <= 0 {
        buf.extend_from_slice(b"0.");
        buf.extend(std::iter::repeat_n(b'0', (-dp) as usize));
        buf.extend_from_slice(digits);
    } else {
        let dp = dp as usize;
        if dp >= nd {
            buf.extend_from_slice(digits);
            buf.extend(std::iter::repeat_n(b'0', dp - nd));
        } else {
            buf.extend_from_slice(&digits[..dp]);
            buf.push(b'.');
            buf.extend_from_slice(&digits[dp..]);
        }
    }
}

/// Go rounds an exact tie between two shortest candidates half-to-even;
/// Rust's `{:e}` rounds it up. A tie needs the exact decimal value of `f` to
/// have exactly `nd + 1` significant digits, the last being 5. Returns Go's
/// digits in that case (docs/port/research/go-float-format.md).
fn go_tie_digits(f: f64, nd: usize, exp: i32) -> Option<String> {
    if nd < 16 {
        return None;
    }
    let bits = f.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let mut m = bits & ((1u64 << 52) - 1);
    let mut e;
    if biased == 0 {
        e = -1074;
    } else {
        m |= 1u64 << 52;
        e = biased - 1075;
    }
    let tz = m.trailing_zeros() as i32;
    m >>= tz;
    e += tz;
    let n: u128 = if e < 0 {
        let mut n = u128::from(m);
        for _ in 0..(-e) {
            n = n.checked_mul(5)?;
        }
        n
    } else {
        let p5 = 5u128.checked_pow(e.unsigned_abs() + 1)?;
        if u128::from(m) % p5 != 0 {
            return None;
        }
        u128::from(m) / 5u128.pow(e.unsigned_abs())
    };
    let s = n.to_string();
    if s.len() != nd + 1 || !s.ends_with('5') {
        return None;
    }
    let mut q = n / 10;
    if q % 2 == 1 {
        q += 1;
    }
    let qs = q.to_string();
    // Go only breaks the tie if both candidates round-trip to `f`.
    let back: f64 = format!("{}.{}e{}", &qs[..1], &qs[1..], exp).parse().ok()?;
    (back == f).then_some(qs)
}

/// Go-formatted float as a `String`.
pub fn go_float_string(f: f64) -> String {
    let mut b = Vec::with_capacity(24);
    format_go_g(&mut b, f);
    // INFALLIBLE: only ASCII digits, signs, '.', 'e', "NaN" and "Inf" are written.
    String::from_utf8(b).unwrap_or_default()
}

/// A family's samples (`pkg/metric.Family`); name and type come from its generator.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Family {
    pub metrics: Vec<Metric>,
    /// Labels every sample starts with, rendered once (`k="v",k2="v2"`).
    prefix: Vec<u8>,
}

/// Assumed rendered length of a value plus the space before it and the newline.
const VALUE_SIZE_HINT: usize = 16;

impl Family {
    pub fn new(metrics: Vec<Metric>) -> Self {
        Family {
            metrics,
            prefix: Vec::new(),
        }
    }

    /// Samples that all start with the resource's default labels (Go's
    /// `wrapXFunc`: `mergeKeyValues` with `keys`/`values` first). The labels
    /// are escaped and rendered once, not per sample.
    ///
    /// Panics when keys and values differ in length, as the Go original does.
    pub fn with_default_labels(metrics: Vec<Metric>, keys: &[&str], values: &[&str]) -> Self {
        assert!(
            keys.len() == values.len(),
            "expected labelKeys {keys:?} to be of same length as labelValues {values:?}"
        );
        let mut prefix = Vec::new();
        for (i, (k, v)) in keys.iter().zip(values).enumerate() {
            if i > 0 {
                prefix.push(b',');
            }
            write_label(&mut prefix, k, v);
        }
        Family { metrics, prefix }
    }

    /// Appends every sample, each prefixed with `name`. The caller sizes
    /// `buf` (`size_hint`), once for all of an object's families.
    pub fn append_bytes(&self, name: &str, buf: &mut Vec<u8>) {
        for m in &self.metrics {
            buf.extend_from_slice(name.as_bytes());
            m.write_prefixed(buf, &self.prefix);
        }
    }

    /// Estimated rendered size, to size buffers up front.
    pub fn size_hint(&self, name: &str) -> usize {
        self.metrics
            .iter()
            .map(|m| {
                name.len()
                    + 3
                    + self.prefix.len()
                    + VALUE_SIZE_HINT
                    + m.label_keys
                        .iter()
                        .zip(&m.label_values)
                        .map(|(k, v)| k.len() + 4 + v.len())
                        .sum::<usize>()
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(f: f64) -> String {
        go_float_string(f)
    }

    #[test]
    fn go_g_format() {
        assert_eq!(g(1.501569018e9), "1.501569018e+09");
        assert_eq!(g(1e6), "1e+06");
        assert_eq!(g(999999.0), "999999");
        assert_eq!(g(123456.5), "123456.5");
        assert_eq!(g(0.0001), "0.0001");
        assert_eq!(g(0.00001), "1e-05");
        assert_eq!(g(0.5), "0.5");
        assert_eq!(g(-2.5), "-2.5");
        assert_eq!(g(1e21), "1e+21");
        assert_eq!(g(1.7976931348623157e308), "1.7976931348623157e+308");
        assert_eq!(g(5e-324), "5e-324");
        assert_eq!(g(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(g(100.0), "100");
        assert_eq!(g(10596.0), "10596");
    }

    #[test]
    fn write_float_specials() {
        let w = |f: f64| {
            let mut b = Vec::new();
            write_float(&mut b, f);
            String::from_utf8(b).unwrap()
        };
        assert_eq!(w(1.0), "1");
        assert_eq!(w(-0.0), "0");
        assert_eq!(w(-1.0), "-1");
        assert_eq!(w(f64::NAN), "NaN");
        assert_eq!(w(f64::INFINITY), "+Inf");
        assert_eq!(w(f64::NEG_INFINITY), "-Inf");
    }

    #[test]
    fn default_labels_come_first() {
        let f = Family::with_default_labels(
            vec![
                Metric::with(&["phase"], vec!["Running".into()], 1.0),
                Metric::value(0.0),
            ],
            &["namespace", "pod"],
            &["ns", "a\"b"],
        );
        let mut b = Vec::new();
        f.append_bytes("kube_x", &mut b);
        assert_eq!(
            String::from_utf8(b).unwrap(),
            "kube_x{namespace=\"ns\",pod=\"a\\\"b\",phase=\"Running\"} 1\n\
             kube_x{namespace=\"ns\",pod=\"a\\\"b\"} 0\n"
        );
    }

    #[test]
    fn family_rendering_and_escaping() {
        let f = Family::new(vec![
            Metric::with(
                &["namespace", "pod"],
                vec!["ns".into(), "a\"b\\c\nd".into()],
                1.0,
            ),
            Metric::value(3.5),
        ]);
        let mut b = Vec::new();
        f.append_bytes("kube_x", &mut b);
        assert_eq!(
            String::from_utf8(b).unwrap(),
            "kube_x{namespace=\"ns\",pod=\"a\\\"b\\\\c\\nd\"} 1\nkube_x 3.5\n"
        );
    }

    #[test]
    #[should_panic(expected = "same length")]
    fn mismatched_labels_panic() {
        let m = Metric::new(vec![Cow::Borrowed("a")], vec![], 1.0);
        m.write(&mut Vec::new());
    }
}
