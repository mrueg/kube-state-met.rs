//! Ports of the Go standard library parsers and formatters the port depends
//! on: `strconv.ParseInt`/`ParseUint`/`ParseFloat`/`ParseBool`,
//! `strconv.Quote`, `time.ParseDuration`, `time.Duration.String`, `%f`, and
//! the proleptic Gregorian day arithmetic of package `time`.
//! Error texts match Go byte for byte.

/// The two `strconv` failure kinds (`strconv.ErrSyntax`, `strconv.ErrRange`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumErr {
    Syntax,
    Range,
}

impl NumErr {
    fn text(self) -> &'static str {
        match self {
            NumErr::Syntax => "invalid syntax",
            NumErr::Range => "value out of range",
        }
    }
}

/// Formats a `*strconv.NumError`: `strconv.ParseInt: parsing "x": invalid syntax`.
pub fn num_error(func: &str, input: &str, err: NumErr) -> String {
    format!("strconv.{func}: parsing {}: {}", quote(input), err.text())
}

/// Go's `strconv.Quote` (and therefore `fmt`'s `%q` for strings).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{07}' => out.push_str("\\a"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0b}' => out.push_str("\\v"),
            c if is_go_printable(c) => out.push(c),
            c if (c as u32) < 0x80 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x1_0000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// Approximates Go's `strconv.IsPrint`: letters, marks, numbers, punctuation,
/// symbols and the ASCII space. Control, format, separator (other than U+0020),
/// private use and unassigned code points are not printable.
fn is_go_printable(c: char) -> bool {
    let u = c as u32;
    if u < 0x20 || u == 0x7f {
        return false;
    }
    if u < 0x7f {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(u,
        0xad // soft hyphen (Cf)
        | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e
        | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f
        | 0xfeff | 0xfff9..=0xfffb | 0xfffe | 0xffff
        | 0xe000..=0xf8ff | 0xf_0000..=0x10_ffff
        | 0xe_0001 | 0xe_0020..=0xe_007f)
}

fn lower(c: u8) -> u8 {
    c | 0x20
}

/// Go's `underscoreOK`: underscores may only separate digits (or follow a
/// base prefix).
fn underscore_ok(s: &str) -> bool {
    let mut s = s.as_bytes();
    let mut saw = b'^';
    let mut i = 0;
    if let Some((&first, rest)) = s.split_first()
        && (first == b'-' || first == b'+')
    {
        s = rest;
    }
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(lower(s[1]), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = lower(s[1]) == b'x';
    }
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(c))) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
        i += 1;
    }
    saw != b'_'
}

/// `strconv.ParseUint(s, base, bits)` for base 0 or 2..=36.
pub fn parse_uint(s: &str, base: u32, bits: u32) -> Result<u64, NumErr> {
    if s.is_empty() {
        return Err(NumErr::Syntax);
    }
    let base0 = base == 0;
    let s0 = s;
    let mut digits = s.as_bytes();
    let mut base = u64::from(base);
    if base0 {
        base = 10;
        if digits[0] == b'0' {
            if digits.len() >= 3 && lower(digits[1]) == b'b' {
                base = 2;
                digits = &digits[2..];
            } else if digits.len() >= 3 && lower(digits[1]) == b'o' {
                base = 8;
                digits = &digits[2..];
            } else if digits.len() >= 3 && lower(digits[1]) == b'x' {
                base = 16;
                digits = &digits[2..];
            } else {
                base = 8;
                digits = &digits[1..];
            }
        }
    }
    // Go: bitSize 0 means 64; a base outside 2..=36 is an error.
    if !(2..=36).contains(&base) {
        return Err(NumErr::Syntax);
    }
    let bits = if bits == 0 { 64 } else { bits };
    let cutoff = u64::MAX / base + 1;
    let max_val: u64 = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in digits {
        let d = if c == b'_' && base0 {
            underscores = true;
            continue;
        } else if c.is_ascii_digit() {
            u64::from(c - b'0')
        } else if lower(c).is_ascii_lowercase() {
            u64::from(lower(c) - b'a' + 10)
        } else {
            return Err(NumErr::Syntax);
        };
        if d >= base {
            return Err(NumErr::Syntax);
        }
        if n >= cutoff {
            return Err(NumErr::Range);
        }
        n *= base;
        let n1 = n.wrapping_add(d);
        if n1 < n || n1 > max_val {
            return Err(NumErr::Range);
        }
        n = n1;
    }
    if underscores && !underscore_ok(s0) {
        return Err(NumErr::Syntax);
    }
    Ok(n)
}

/// `strconv.ParseInt(s, base, bits)`.
pub fn parse_int(s: &str, base: u32, bits: u32) -> Result<i64, NumErr> {
    if s.is_empty() {
        return Err(NumErr::Syntax);
    }
    let (neg, rest) = match s.as_bytes()[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let un = match parse_uint(rest, base, bits) {
        Ok(v) => v,
        Err(NumErr::Syntax) => return Err(NumErr::Syntax),
        Err(NumErr::Range) => u64::MAX,
    };
    let bits = if bits == 0 { 64 } else { bits.min(64) };
    let cutoff = 1u64 << (bits - 1);
    if !neg && un >= cutoff {
        return Err(NumErr::Range);
    }
    if neg && un > cutoff {
        return Err(NumErr::Range);
    }
    if neg {
        Ok((un as i64).wrapping_neg())
    } else {
        Ok(un as i64)
    }
}

/// `strconv.ParseBool`.
pub fn parse_bool(s: &str) -> Result<bool, NumErr> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(NumErr::Syntax),
    }
}

fn common_prefix_len_ignore_case(s: &str, prefix: &str) -> usize {
    s.bytes()
        .zip(prefix.bytes())
        .take_while(|(a, b)| lower(*a) == *b)
        .count()
}

/// Go's `special`: infinities and NaN. Returns the value and the bytes used.
fn parse_special(s: &str) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let first = *b.first()?;
    let (sign, nsign, rest) = match first {
        b'+' => (1.0, 1, &s[1..]),
        b'-' => (-1.0, 1, &s[1..]),
        b'n' | b'N' => {
            return (common_prefix_len_ignore_case(s, "nan") == 3).then_some((f64::NAN, 3));
        }
        _ => (1.0, 0, s),
    };
    match rest.as_bytes().first() {
        Some(b'i' | b'I') => {
            let mut n = common_prefix_len_ignore_case(rest, "infinity");
            if 3 < n && n < 8 {
                n = 3;
            }
            (n == 3 || n == 8).then_some((sign * f64::INFINITY, nsign + n))
        }
        _ => None,
    }
}

/// `strconv.ParseFloat(s, 64)`.
pub fn parse_float(s: &str) -> Result<f64, NumErr> {
    if let Some((v, n)) = parse_special(s) {
        return if n == s.len() {
            Ok(v)
        } else {
            Err(NumErr::Syntax)
        };
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut hex = false;
    if i + 2 < b.len() && b[i] == b'0' && lower(b[i + 1]) == b'x' {
        hex = true;
        i += 2;
    }
    let mut underscores = false;
    let mut saw_dot = false;
    let mut saw_digits = false;
    let mut mantissa = String::new();
    while i < b.len() {
        let c = b[i];
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if saw_dot {
                break;
            }
            saw_dot = true;
            mantissa.push('.');
        } else if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(c))) {
            saw_digits = true;
            mantissa.push(char::from(c));
        } else {
            break;
        }
        i += 1;
    }
    if !saw_digits {
        return Err(NumErr::Syntax);
    }
    let mut exp: i64 = 0;
    let mut has_exp = false;
    if i < b.len() && ((!hex && lower(b[i]) == b'e') || (hex && lower(b[i]) == b'p')) {
        i += 1;
        if i >= b.len() {
            return Err(NumErr::Syntax);
        }
        let mut esign = 1;
        if b[i] == b'+' {
            i += 1;
        } else if b[i] == b'-' {
            i += 1;
            esign = -1;
        }
        if i >= b.len() || !b[i].is_ascii_digit() {
            return Err(NumErr::Syntax);
        }
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            if b[i] == b'_' {
                underscores = true;
            } else if exp < 10_000 {
                exp = exp * 10 + i64::from(b[i] - b'0');
            }
            i += 1;
        }
        exp *= esign;
        has_exp = true;
    } else if hex {
        return Err(NumErr::Syntax);
    }
    if i != b.len() {
        return Err(NumErr::Syntax);
    }
    if underscores && !underscore_ok(s) {
        return Err(NumErr::Syntax);
    }
    let v = if hex {
        hex_float(&mantissa, exp)
    } else {
        let mut text = mantissa;
        if has_exp {
            text.push_str(&format!("e{exp}"));
        }
        text.parse::<f64>().map_err(|_| NumErr::Syntax)?
    };
    if v.is_infinite() {
        return Err(NumErr::Range);
    }
    Ok(if neg { -v } else { v })
}

/// Converts a hexadecimal mantissa (with optional point) and binary exponent.
fn hex_float(mantissa: &str, exp: i64) -> f64 {
    let mut m: u64 = 0;
    let mut e = exp;
    let mut sticky = false;
    let mut after_dot = false;
    for c in mantissa.chars() {
        if c == '.' {
            after_dot = true;
            continue;
        }
        let d = u64::from(c.to_digit(16).unwrap_or(0));
        if m >> 60 == 0 {
            m = (m << 4) | d;
            if after_dot {
                e -= 4;
            }
        } else {
            sticky |= d != 0;
            if !after_dot {
                e += 4;
            }
        }
    }
    if sticky {
        m |= 1;
    }
    round_to_f64(m, e)
}

/// `m * 2^e` rounded once to the nearest f64, ties to even (Go's
/// `atofHex`); subnormal results are rounded at their own precision.
fn round_to_f64(m: u64, e: i64) -> f64 {
    if m == 0 {
        return 0.0;
    }
    let lz = m.leading_zeros();
    let m = m << lz;
    let e = e - i64::from(lz);
    // The value is in [2^top, 2^(top+1)).
    let mut top = e + 63;
    // Significant bits kept: 53, fewer for subnormals.
    let prec = if top < -1022 { 53 - (-1022 - top) } else { 53 };
    if prec < 0 {
        return 0.0;
    }
    let shift = (64 - prec) as u32;
    let (mut mant, rem, half) = if shift >= 64 {
        (0u64, m, 1u64 << 63)
    } else {
        (m >> shift, m & ((1u64 << shift) - 1), 1u64 << (shift - 1))
    };
    if rem > half || (rem == half && mant & 1 == 1) {
        mant += 1;
    }
    if top < -1022 {
        // Subnormal: the bits are the multiple of 2^-1074 (a carry into bit
        // 52 is the smallest normal, which these bits also encode).
        return f64::from_bits(mant);
    }
    if mant == 1 << 53 {
        mant >>= 1;
        top += 1;
    }
    if top > 1023 {
        return f64::INFINITY;
    }
    let biased = (top + 1023) as u64;
    f64::from_bits((biased << 52) | (mant & ((1 << 52) - 1)))
}

/// Go's `fmt.Sprintf("%f", f)`.
pub fn format_f(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_owned()
    } else if f == f64::INFINITY {
        "+Inf".to_owned()
    } else if f == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else {
        format!("{f:.6}")
    }
}

/// `time`'s private `quote`: non-ASCII and control bytes become `\xNN`.
fn time_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if (c as u32) >= 0x80 || c < ' ' {
            let mut buf = [0u8; 4];
            for byte in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("\\x{byte:02x}"));
            }
        } else {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out.push('"');
    out
}

const NANOS_LIMIT: u64 = 1 << 63;

fn unit_nanos(unit: &str) -> Option<u64> {
    Some(match unit {
        "ns" => 1,
        "us" | "\u{b5}s" | "\u{3bc}s" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        _ => return None,
    })
}

fn leading_int(s: &str) -> Option<(u64, &str)> {
    let b = s.as_bytes();
    let mut x: u64 = 0;
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        if x > NANOS_LIMIT / 10 {
            return None;
        }
        x = x * 10 + u64::from(b[i] - b'0');
        if x > NANOS_LIMIT {
            return None;
        }
        i += 1;
    }
    Some((x, &s[i..]))
}

fn leading_fraction(s: &str) -> (u64, f64, &str) {
    let b = s.as_bytes();
    let mut x: u64 = 0;
    let mut scale = 1.0;
    let mut overflow = false;
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        let c = u64::from(b[i] - b'0');
        i += 1;
        if overflow {
            continue;
        }
        if x > (NANOS_LIMIT - 1) / 10 {
            overflow = true;
            continue;
        }
        let y = x * 10 + c;
        if y > NANOS_LIMIT {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, &s[i..])
}

/// `time.ParseDuration`, returning nanoseconds.
pub fn parse_duration(orig: &str) -> Result<i64, String> {
    let invalid = || format!("time: invalid duration {}", time_quote(orig));
    let mut s = orig;
    let mut neg = false;
    if let Some(&c) = s.as_bytes().first()
        && (c == b'-' || c == b'+')
    {
        neg = c == b'-';
        s = &s[1..];
    }
    if s == "0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(invalid());
    }
    let mut d: u64 = 0;
    while !s.is_empty() {
        let first = s.as_bytes()[0];
        if !(first == b'.' || first.is_ascii_digit()) {
            return Err(invalid());
        }
        let pl = s.len();
        let (mut v, rest) = leading_int(s).ok_or_else(invalid)?;
        s = rest;
        let pre = pl != s.len();
        let mut post = false;
        let mut f = 0;
        let mut scale = 1.0;
        if let Some(rest) = s.strip_prefix('.') {
            let pl = rest.len();
            let (ff, sc, rest) = leading_fraction(rest);
            f = ff;
            scale = sc;
            s = rest;
            post = pl != s.len();
        }
        if !pre && !post {
            return Err(invalid());
        }
        let i = s
            .bytes()
            .position(|c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if i == 0 {
            return Err(format!(
                "time: missing unit in duration {}",
                time_quote(orig)
            ));
        }
        let u = &s[..i];
        s = &s[i..];
        let Some(unit) = unit_nanos(u) else {
            return Err(format!(
                "time: unknown unit {} in duration {}",
                time_quote(u),
                time_quote(orig)
            ));
        };
        if v > NANOS_LIMIT / unit {
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > NANOS_LIMIT {
                return Err(invalid());
            }
        }
        d += v;
        if d > NANOS_LIMIT {
            return Err(invalid());
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > NANOS_LIMIT - 1 {
        return Err(invalid());
    }
    Ok(d as i64)
}

fn fmt_frac(out: &mut Vec<u8>, mut v: u64, prec: usize) -> u64 {
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            out.push(b'0' + digit as u8);
        }
        v /= 10;
    }
    if print {
        out.push(b'.');
    }
    v
}

fn fmt_int(out: &mut Vec<u8>, mut v: u64) {
    if v == 0 {
        out.push(b'0');
        return;
    }
    while v > 0 {
        out.push(b'0' + (v % 10) as u8);
        v /= 10;
    }
}

/// `time.Duration(nanos).String()`.
pub fn format_duration(nanos: i64) -> String {
    // Built back to front, as in Go, then reversed.
    let mut out: Vec<u8> = Vec::with_capacity(32);
    let neg = nanos < 0;
    let mut u = nanos.unsigned_abs();
    if u < 1_000_000_000 {
        let prec;
        out.push(b's');
        if u == 0 {
            return "0s".to_owned();
        } else if u < 1_000 {
            prec = 0;
            out.push(b'n');
        } else if u < 1_000_000 {
            prec = 3;
            // "µ" is 0xC2 0xB5; pushed reversed.
            out.push(0xb5);
            out.push(0xc2);
        } else {
            prec = 6;
            out.push(b'm');
        }
        u = fmt_frac(&mut out, u, prec);
        fmt_int(&mut out, u);
    } else {
        out.push(b's');
        u = fmt_frac(&mut out, u, 9);
        fmt_int(&mut out, u % 60);
        u /= 60;
        if u > 0 {
            out.push(b'm');
            fmt_int(&mut out, u % 60);
            u /= 60;
            if u > 0 {
                out.push(b'h');
                fmt_int(&mut out, u);
            }
        }
    }
    if neg {
        out.push(b'-');
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

/// Days since 1970-01-01 of a proleptic Gregorian date (month 1-12).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints() {
        assert_eq!(parse_int("0x10", 0, 64), Ok(16));
        assert_eq!(parse_int("010", 0, 64), Ok(8));
        assert_eq!(parse_int("0o10", 0, 64), Ok(8));
        assert_eq!(parse_int("0b10", 0, 64), Ok(2));
        assert_eq!(parse_int("1_000", 0, 64), Ok(1000));
        assert_eq!(parse_int("1__000", 0, 64), Err(NumErr::Syntax));
        assert_eq!(parse_int("-2147483648", 0, 32), Ok(-2_147_483_648));
        assert_eq!(parse_int("2147483648", 0, 32), Err(NumErr::Range));
        assert_eq!(parse_int("1_0", 10, 32), Err(NumErr::Syntax));
        assert_eq!(parse_int("", 0, 64), Err(NumErr::Syntax));
        assert_eq!(parse_int("-", 0, 64), Err(NumErr::Syntax));
        assert_eq!(
            num_error("ParseInt", "abc", NumErr::Syntax),
            r#"strconv.ParseInt: parsing "abc": invalid syntax"#
        );
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("1h2m3.5s"), Ok(3_723_500_000_000));
        assert_eq!(parse_duration("-1.5h"), Ok(-5_400_000_000_000));
        assert_eq!(parse_duration("0"), Ok(0));
        assert_eq!(
            parse_duration("1"),
            Err(r#"time: missing unit in duration "1""#.to_owned())
        );
        assert_eq!(
            parse_duration("1x"),
            Err(r#"time: unknown unit "x" in duration "1x""#.to_owned())
        );
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(300_000_000_000), "5m0s");
        assert_eq!(format_duration(-1_500), "-1.5\u{b5}s");
        assert_eq!(format_duration(3_723_500_000_000), "1h2m3.5s");
        assert_eq!(format_duration(i64::MIN), "-2562047h47m16.854775808s");
    }

    #[test]
    fn floats() {
        assert_eq!(parse_float("0.9"), Ok(0.9));
        assert_eq!(parse_float("1_0.5"), Ok(10.5));
        assert_eq!(parse_float("0x1p-2"), Ok(0.25));
        assert_eq!(parse_float("1e400"), Err(NumErr::Range));
        assert_eq!(parse_float("inf"), Ok(f64::INFINITY));
        assert_eq!(parse_float("+nan"), Err(NumErr::Syntax));
        // Checked against Go's strconv.ParseFloat.
        for (s, want) in [
            ("1_000", Ok(1000.0)),
            ("0x1_0p0", Ok(16.0)),
            ("+Inf", Ok(f64::INFINITY)),
            ("infinity", Ok(f64::INFINITY)),
            ("-nan", Err(NumErr::Syntax)),
            ("0x1", Err(NumErr::Syntax)),
            ("0x", Err(NumErr::Syntax)),
            (".5", Ok(0.5)),
            ("1.", Ok(1.0)),
            ("-0X1.8P1", Ok(-3.0)),
            ("1__0", Err(NumErr::Syntax)),
            ("_1", Err(NumErr::Syntax)),
            ("0x_1p0", Ok(1.0)),
            ("", Err(NumErr::Syntax)),
        ] {
            assert_eq!(parse_float(s), want, "{s}");
        }
        assert_eq!(format_f(0.9), "0.900000");
        assert_eq!(format_f(f64::NEG_INFINITY), "-Inf");
    }

    #[test]
    fn quoting() {
        assert_eq!(
            quote("a\"b\\\n\u{1}é\u{a0}"),
            "\"a\\\"b\\\\\\n\\x01é\\u00a0\""
        );
    }

    /// Go-verified bit patterns (`strconv.ParseFloat(s, 64)`); `true`: a
    /// range error.
    #[test]
    fn hex_floats_round_once() {
        let cases: &[(&str, u64, bool)] = &[
            ("0x1.400000000000001p-1073", 0x3, false),
            ("0x1p-1074", 0x1, false),
            ("0x1p-1075", 0x0, false),
            ("0x1.0000000000001p-1075", 0x1, false),
            ("0x1.8p-1075", 0x1, false),
            ("0x1.fffffffffffffp1023", 0x7fefffffffffffff, false),
            ("0x1.fffffffffffff8p1023", 0x7ff0000000000000, true),
            ("0x1p1024", 0x7ff0000000000000, true),
            ("0x1.0000000000000801p0", 0x3ff0000000000001, false),
            ("0x1.00000000000008p0", 0x3ff0000000000000, false),
            ("0x1.00000000000018p0", 0x3ff0000000000002, false),
            ("0x.8p-1022", 0x8000000000000, false),
            ("0x1.ffffffffffffffp-1023", 0x10000000000000, false),
            ("0xabcdef.123456789abcdefp-30", 0x3f8579bde2468acf, false),
            ("0x0p0", 0x0, false),
            ("0x1_0p0", 0x4030000000000000, false),
        ];
        for (s, bits, range) in cases {
            match parse_float(s) {
                Ok(v) => {
                    assert!(!range, "{s}: want a range error, got {v}");
                    assert_eq!(v.to_bits(), *bits, "{s}");
                }
                Err(e) => assert!(*range && e == NumErr::Range, "{s}: {e:?}"),
            }
        }
    }
}
