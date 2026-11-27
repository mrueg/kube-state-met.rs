//! Value conversions of `registry_factory.go`: `toFloat64`, `fmt.Sprintf("%v")`
//! of unstructured values, and the `time.Parse(time.RFC3339, s)` they use.
//!
//! Objects are `serde_json::Value`s decoded the way client-go decodes
//! unstructured JSON: integers that fit an `int64` are Go `int64`, every other
//! number is a `float64`, and JSON `null` is Go `nil`.

use serde_json::Value;

use crate::gostr;

/// Go's `nil` for unstructured values: a missing value or JSON `null`.
pub(crate) fn non_nil(v: Option<&Value>) -> Option<&Value> {
    match v {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}

fn number_f64(n: &serde_json::Number) -> f64 {
    if let Some(i) = n.as_i64() {
        return i as f64;
    }
    if let Some(u) = n.as_u64() {
        return u as f64;
    }
    n.as_f64().unwrap_or(0.0)
}

/// `fmt.Sprintf("%v", v)` of an unstructured value.
pub fn go_fmt_v(v: &Value) -> String {
    let mut out = String::new();
    write_v(&mut out, v);
    out
}

fn write_v(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("<nil>"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else {
                out.push_str(&ksm_core::metric::go_float_string(number_f64(n)));
            }
        }
        Value::String(s) => out.push_str(s),
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_v(out, e);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push_str("map[");
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(k);
                out.push(':');
                if let Some(e) = m.get(k) {
                    write_v(out, e);
                }
            }
            out.push(']');
        }
    }
}

/// The Go type name `%T` prints for an unstructured value.
pub(crate) fn go_type_name(v: Option<&Value>) -> &'static str {
    match v {
        None | Some(Value::Null) => "<nil>",
        Some(Value::Bool(_)) => "bool",
        Some(Value::Number(n)) => {
            if n.as_i64().is_some() {
                "int64"
            } else {
                "float64"
            }
        }
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "[]interface {}",
        Some(Value::Object(_)) => "map[string]interface {}",
    }
}

/// `toFloat64`: converts a value to the float64 value of a metric.
pub fn to_float64(value: Option<&Value>, nil_is_zero: bool) -> Result<f64, String> {
    let Some(value) = non_nil(value) else {
        if nil_is_zero {
            return Ok(0.0);
        }
        return Err("expected number but found nil value".to_owned());
    };
    match value {
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => string_to_float64(s),
        Value::Number(n) => Ok(number_f64(n)),
        other => Err(format!("expected number but was {}", go_fmt_v(other))),
    }
}

fn string_to_float64(s: &str) -> Result<f64, String> {
    // The string is a boolean or "unknown" (metav1.Condition).
    let normalized = gostr::to_lower(s);
    if normalized == "true" || normalized == "yes" {
        return Ok(1.0);
    }
    if normalized == "false" || normalized == "no" || normalized == "unknown" {
        return Ok(0.0);
    }
    // An RFC3339 timestamp.
    if let Some(unix) = parse_rfc3339_unix(s) {
        return Ok(unix as f64);
    }
    // A quantity like "25m" or "5Gi".
    if let Ok(q) = ksm_stores::quantity::parse(s) {
        return Ok(q.as_approximate_f64());
    }
    // A percentage: validation.IsValidPercent is ^[0-9]+%$.
    if let Some(digits) = s.strip_suffix('%')
        && !digits.is_empty()
        && digits.bytes().all(|c| c.is_ascii_digit())
    {
        return go_parse_float(digits).map(|t| t / 100.0);
    }
    go_parse_float(s)
}

/// `strconv.ParseFloat(s, 64)` with its error message.
pub(crate) fn go_parse_float(s: &str) -> Result<f64, String> {
    use ksm_core::gostd::{num_error, parse_float};
    parse_float(s).map_err(|e| num_error("ParseFloat", s, e))
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in(month: i64, year: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn is_digit(b: &[u8], i: usize) -> bool {
    b.get(i).is_some_and(u8::is_ascii_digit)
}

/// Go's `getnum`: one or two digits (exactly two when `fixed`).
fn getnum(b: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    if !is_digit(b, 0) {
        return None;
    }
    if !is_digit(b, 1) {
        if fixed {
            return None;
        }
        return Some((i64::from(b[0] - b'0'), &b[1..]));
    }
    Some((
        i64::from(b[0] - b'0') * 10 + i64::from(b[1] - b'0'),
        &b[2..],
    ))
}

fn skip(b: &[u8], lit: u8) -> Option<&[u8]> {
    match b.first() {
        Some(&c) if c == lit => Some(&b[1..]),
        _ => None,
    }
}

/// `time.Parse(time.RFC3339, s).Unix()`, or `None` where Go returns an error.
///
/// Go tries a strict fast path first and falls back to its generic layout
/// parser; the generic parser accepts everything the fast path accepts, with
/// the same result, so only the generic one is ported (layout
/// `2006-01-02T15:04:05Z07:00`).
pub(crate) fn parse_rfc3339_unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    // stdLongYear
    if b.len() < 4 || !is_digit(b, 0) || !b[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let year = b[..4]
        .iter()
        .fold(0i64, |acc, c| acc * 10 + i64::from(c - b'0'));
    let b = skip(&b[4..], b'-')?;
    // stdZeroMonth
    let (month, b) = getnum(b, true)?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let b = skip(b, b'-')?;
    // stdZeroDay
    let (day, b) = getnum(b, true)?;
    let b = skip(b, b'T')?;
    // stdHour
    let (hour, b) = getnum(b, false)?;
    if !(0..24).contains(&hour) {
        return None;
    }
    let b = skip(b, b':')?;
    // stdZeroMinute
    let (min, b) = getnum(b, true)?;
    if !(0..60).contains(&min) {
        return None;
    }
    let b = skip(b, b':')?;
    // stdZeroSecond
    let (sec, mut b) = getnum(b, true)?;
    if !(0..60).contains(&sec) {
        return None;
    }
    // A fractional second the layout does not have: '.' or ',' then digits.
    if b.len() >= 2 && (b[0] == b'.' || b[0] == b',') && is_digit(b, 1) {
        let mut n = 2;
        while n < b.len() && is_digit(b, n) {
            n += 1;
        }
        b = &b[n..];
    }
    // stdISO8601ColonTZ
    let offset = if b.first() == Some(&b'Z') {
        b = &b[1..];
        0
    } else {
        if b.len() < 6 || b[3] != b':' {
            return None;
        }
        let (hr, _) = getnum(&b[1..3], true)?;
        let (mm, _) = getnum(&b[4..6], true)?;
        if hr > 24 || mm > 60 {
            return None;
        }
        let off = (hr * 60 + mm) * 60;
        let off = match b[0] {
            b'+' => off,
            b'-' => -off,
            _ => return None,
        };
        b = &b[6..];
        off
    };
    if !b.is_empty() {
        return None;
    }
    if day < 1 || day > days_in(month, year) {
        return None;
    }
    let days = ksm_core::gostd::days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + min * 60 + sec - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339() {
        assert_eq!(
            parse_rfc3339_unix("2022-06-28T00:00:00Z"),
            Some(1_656_374_400)
        );
        assert_eq!(parse_rfc3339_unix("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_rfc3339_unix("1970-01-01T1:00:00.5+01:00"), Some(0));
        assert_eq!(parse_rfc3339_unix("2023-02-29T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_unix("2022-06-28 00:00:00Z"), None);
    }

    #[test]
    fn hex() {
        assert_eq!(go_parse_float("0x1p-2"), Ok(0.25));
        assert_eq!(go_parse_float("-0X1.8P1"), Ok(-3.0));
        assert!(go_parse_float("0x1").is_err());
    }
}
