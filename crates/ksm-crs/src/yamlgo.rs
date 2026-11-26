//! Decoding helpers that reproduce `gopkg.in/yaml.v3` (as vendored by
//! `sigs.k8s.io/yaml/goyaml.v3`) on top of serde:
//!
//! - a `string` takes the raw text of any non-null scalar (`1.50`, `0x10`,
//!   `true` stay as written); a null scalar leaves it empty;
//! - `null` leaves any field at its zero value;
//! - null elements of a sequence are dropped; a null map value is the zero
//!   value of the element type;
//! - `bool` also accepts the YAML 1.1 words (`yes`, `on`, `n`, ...);
//! - integers accept floats with an integral part in range (truncated);
//! - a repeated mapping key is an error.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// A Go `string` field: `None` for a null scalar.
pub(crate) struct GoString(pub Option<String>);

struct RawStr;

impl<'de> Visitor<'de> for RawStr {
    type Value = Option<String>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_str(self)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(Some(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(Some(v))
    }
}

impl<'de> Deserialize<'de> for GoString {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_option(RawStr).map(GoString)
    }
}

/// A value that may be null: Go leaves the field at its zero value.
pub(crate) struct Nullable<T>(pub Option<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Nullable<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(d).map(Nullable)
    }
}

/// A Go slice: null elements are dropped.
pub(crate) struct GoSeq<T>(pub Vec<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for GoSeq<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Vec<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a sequence")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
                let mut out = Vec::new();
                while let Some(e) = seq.next_element::<Nullable<T>>()? {
                    if let Some(e) = e.0 {
                        out.push(e);
                    }
                }
                Ok(out)
            }
        }
        d.deserialize_seq(V(PhantomData)).map(GoSeq)
    }
}

/// A Go `[]string`.
pub(crate) fn string_seq<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = Nullable::<GoSeq<GoString>>::deserialize(d)?;
    Ok(v.0
        .map(|s| s.0.into_iter().filter_map(|e| e.0).collect())
        .unwrap_or_default())
}

/// A Go `string`.
pub(crate) fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(GoString::deserialize(d)?.0.unwrap_or_default())
}

/// A Go `*string`.
pub(crate) fn opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(GoString::deserialize(d)?.0)
}

/// A struct-valued field that may be null.
pub(crate) fn zero_or<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<T, D::Error> {
    Ok(Nullable::<T>::deserialize(d)?.0.unwrap_or_default())
}

/// A pointer-to-struct field.
pub(crate) fn opt<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Ok(Nullable::<T>::deserialize(d)?.0)
}

/// A slice of structs.
pub(crate) fn seq<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    Ok(Nullable::<GoSeq<T>>::deserialize(d)?
        .0
        .map(|s| s.0)
        .unwrap_or_default())
}

/// A Go map with string keys; values are decoded by `V` and a null value
/// becomes the zero value.
fn go_map<'de, D, V, T>(d: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de> + Into<T>,
    T: Default,
{
    struct Vis<V, T>(PhantomData<(V, T)>);
    impl<'de, V: Deserialize<'de> + Into<T>, T: Default> Visitor<'de> for Vis<V, T> {
        type Value = Option<BTreeMap<String, T>>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a mapping")
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_map(self)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some(k) = map.next_key::<GoString>()? {
                let v = map.next_value::<Nullable<V>>()?;
                let Some(k) = k.0 else {
                    continue;
                };
                if out.contains_key(&k) {
                    return Err(de::Error::custom(format!(
                        "mapping key {k:?} already defined"
                    )));
                }
                out.insert(k, v.0.map(Into::into).unwrap_or_default());
            }
            Ok(Some(out))
        }
    }
    Ok(d.deserialize_option(Vis::<V, T>(PhantomData))?
        .unwrap_or_default())
}

struct StrList(Vec<String>);

impl<'de> Deserialize<'de> for StrList {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        string_seq(d).map(StrList)
    }
}

impl From<StrList> for Vec<String> {
    fn from(v: StrList) -> Self {
        v.0
    }
}

impl From<GoString> for String {
    fn from(v: GoString) -> Self {
        v.0.unwrap_or_default()
    }
}

/// A Go `map[string]string`.
pub(crate) fn string_map<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    go_map::<D, GoString, String>(d)
}

/// A Go `map[string][]string`.
pub(crate) fn paths_map<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, Vec<String>>, D::Error> {
    go_map::<D, StrList, Vec<String>>(d)
}

/// yaml.v3's `decoder.terror` text: `cannot unmarshal !!tag `value` into T`,
/// values longer than 10 bytes cut to 7 and `...`; no value for `!!seq` and
/// `!!map`.
pub(crate) fn terror(tag: &str, value: &str, go_type: &str) -> String {
    if tag == "!!seq" || tag == "!!map" {
        return format!("cannot unmarshal {tag} into {go_type}");
    }
    if value.len() > 10 {
        let mut cut = 7;
        while !value.is_char_boundary(cut) {
            cut -= 1;
        }
        return format!(
            "cannot unmarshal {tag} `{}...` into {go_type}",
            &value[..cut]
        );
    }
    format!("cannot unmarshal {tag} `{value}` into {go_type}")
}

/// A Go `bool`.
pub(crate) fn go_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct V;
    impl Visitor<'_> for V {
        type Value = bool;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bool")
        }
        fn visit_unit<E: de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_none<E: de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<bool, E> {
            // YAML 1.1 compatibility of yaml.v3 when decoding into a bool.
            match v {
                "y" | "Y" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => Ok(true),
                "n" | "N" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => Ok(false),
                _ => Err(E::custom(terror("!!str", v, "bool"))),
            }
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<bool, E> {
            Err(E::custom(terror("!!int", &v.to_string(), "bool")))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<bool, E> {
            Err(E::custom(terror("!!int", &v.to_string(), "bool")))
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<bool, E> {
            Err(E::custom(terror("!!float", &v.to_string(), "bool")))
        }
    }
    d.deserialize_any(V)
}

const LEVEL: &str = "klog.Level";

/// A Go `int32` (`klog.Level`).
pub(crate) fn go_i32<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    struct V;
    impl Visitor<'_> for V {
        type Value = i32;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an int32")
        }
        fn visit_unit<E: de::Error>(self) -> Result<i32, E> {
            Ok(0)
        }
        fn visit_none<E: de::Error>(self) -> Result<i32, E> {
            Ok(0)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i32, E> {
            i32::try_from(v).map_err(|_| E::custom(terror("!!int", &v.to_string(), LEVEL)))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i32, E> {
            i32::try_from(v).map_err(|_| E::custom(terror("!!int", &v.to_string(), LEVEL)))
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<i32, E> {
            // yaml.v3 converts a float when its int64 conversion fits.
            let t = v.trunc();
            if v.is_finite() && t >= f64::from(i32::MIN) && t <= f64::from(i32::MAX) {
                #[expect(clippy::cast_possible_truncation, reason = "range checked above")]
                return Ok(t as i32);
            }
            Err(E::custom(terror("!!float", &v.to_string(), LEVEL)))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<i32, E> {
            Err(E::custom(terror("!!str", v, LEVEL)))
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<i32, E> {
            Err(E::custom(terror("!!bool", &v.to_string(), LEVEL)))
        }
    }
    d.deserialize_any(V)
}

/// The Go type yaml.v3 names in a type error, from serde's path to the
/// failing value (`spec.resources[0].metrics[1].each.gauge.path`).
fn go_type_at(path: &str) -> &'static str {
    const PKG_SPEC: &str = "customresourcestate.MetricsSpec";
    if path.is_empty() || path == "." {
        return "customresourcestate.Metrics";
    }
    // Values of the two map fields are leaves (their keys may contain dots).
    let paths = path.rfind("labelsFromPath.");
    let common = path.rfind("commonLabels.");
    match (paths, common) {
        (Some(p), c) if c.is_none_or(|c| c < p) => {
            return if path.ends_with(']') {
                "string"
            } else {
                "[]string"
            };
        }
        (_, Some(_)) => return "string",
        _ => {}
    }
    let last = path.rsplit('.').next().unwrap_or(path);
    if let Some(field) = last.strip_suffix(']').and_then(|l| l.rsplit_once('[')) {
        return match field.0 {
            "resources" => "customresourcestate.Resource",
            "metrics" => "customresourcestate.Generator",
            _ => "string",
        };
    }
    match last {
        "spec" => PKG_SPEC,
        "resources" => "[]customresourcestate.Resource",
        "metrics" => "[]customresourcestate.Generator",
        "groupVersionKind" => "customresourcestate.GroupVersionKind",
        "each" => "customresourcestate.Metric",
        "gauge" => "customresourcestate.MetricGauge",
        "info" => "customresourcestate.MetricInfo",
        "stateSet" => "customresourcestate.MetricStateSet",
        "path" | "valueFrom" | "list" => "[]string",
        "labelsFromPath" => "map[string][]string",
        "commonLabels" => "map[string]string",
        "errorLogV" => LEVEL,
        "nilIsZero" => "bool",
        _ => "string",
    }
}

/// Unquotes serde's `string "..."` rendering of an unexpected value.
fn unquote_debug(s: &str) -> String {
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// Rewrites a serde_yaml_ng error in the words of yaml.v3 (`yaml: unmarshal
/// errors:\n  line N: cannot unmarshal ...`). yaml.v3 reports every type
/// error of a document; serde stops at the first one.
pub(crate) fn go_error(e: &serde_yaml_ng::Error) -> String {
    let line = e.location().map_or(0, |l| l.line());
    let full = e.to_string();
    let msg = match full.rfind(" at line ") {
        Some(i) => &full[..i],
        None => full.as_str(),
    };
    let (path, detail) = match msg.split_once(": ") {
        Some((p, d)) if !p.contains(char::is_whitespace) => (p, d),
        _ => ("", msg),
    };
    let unmarshal = |d: String| format!("yaml: unmarshal errors:\n  line {line}: {d}");
    if detail.starts_with("cannot unmarshal ") {
        return unmarshal(detail.to_owned());
    }
    if let Some(key) = detail
        .strip_prefix("mapping key ")
        .and_then(|k| k.strip_suffix(" already defined"))
    {
        return unmarshal(format!("mapping key {key} already defined at line {line}"));
    }
    if let Some(rest) = detail.strip_prefix("invalid type: ") {
        let unexpected = rest.split(", expected ").next().unwrap_or(rest);
        let go_type = go_type_at(path);
        let (tag, value) = if let Some(s) = unexpected.strip_prefix("string ") {
            ("!!str", unquote_debug(s))
        } else if let Some(v) = unexpected.strip_prefix("integer ") {
            ("!!int", v.trim_matches('`').to_owned())
        } else if let Some(v) = unexpected.strip_prefix("floating point ") {
            ("!!float", v.trim_matches('`').to_owned())
        } else if let Some(v) = unexpected.strip_prefix("boolean ") {
            ("!!bool", v.trim_matches('`').to_owned())
        } else if unexpected.starts_with("sequence") {
            ("!!seq", String::new())
        } else if unexpected.starts_with("map") {
            ("!!map", String::new())
        } else {
            return format!("yaml: {msg}");
        };
        return unmarshal(terror(tag, &value, go_type));
    }
    if let Some(m) = libyaml_error(e) {
        return m;
    }
    format!("yaml: {msg}")
}

/// The 0-based line of a `<field>: Mark { line: N, ...}` in libyaml's error
/// debug output (which shows 1-based lines).
fn debug_mark_line(debug: &str, field: &str) -> u64 {
    debug
        .find(&format!("{field}: Mark {{ line: "))
        .map(|i| &debug[i + field.len() + 15..])
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse::<u64>().ok())
        .map_or(0, |n| n.saturating_sub(1))
}

/// Whether `e` is a libyaml scanner or parser error (not a decoding one).
pub(crate) fn is_syntax_error(e: &serde_yaml_ng::Error) -> bool {
    format!("{e:?}").starts_with("Error { kind: ")
}

/// A libyaml scanner or parser error the way yaml.v3's `parser.fail`
/// reports it: `yaml: line N: problem`, where N is the 0-based line of the
/// context mark (else of the problem mark), plus one for scanner errors.
fn libyaml_error(e: &serde_yaml_ng::Error) -> Option<String> {
    let debug = format!("{e:?}");
    let body = debug.strip_prefix("Error { kind: ")?;
    let kind = body.split([',', ' ']).next().unwrap_or("");
    let start = debug.find("problem: \"")? + "problem: ".len();
    // The quoted problem: up to the first unescaped closing quote.
    let quoted = &debug[start..];
    let mut end = None;
    let mut escaped = false;
    for (i, c) in quoted.char_indices().skip(1) {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => {
                end = Some(i);
                break;
            }
            _ => escaped = false,
        }
    }
    let problem = unquote_debug(&quoted[..=end?]);
    let context_line = debug_mark_line(&debug, "context_mark");
    let mut line = if context_line != 0 {
        context_line
    } else {
        debug_mark_line(&debug, "problem_mark")
    };
    if line != 0 && kind == "SCANNER" {
        line += 1;
    }
    Some(if line != 0 {
        format!("yaml: line {line}: {problem}")
    } else {
        format!("yaml: {problem}")
    })
}
