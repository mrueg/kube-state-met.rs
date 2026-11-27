//! Port of `k8s.io/apimachinery/pkg/api/resource.Quantity` (v0.37.2): parsing
//! and the accessors kube-state-metrics uses (`Value`, `MilliValue`,
//! `AsApproximateFloat64`) plus `convertValueToFloat64`. Wrapping and rounding
//! follow Go exactly; see docs/port/research/quantity.md and
//! testdata/golden/quantity.jsonl.

use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Format {
    DecimalExponent,
    BinarySI,
    DecimalSI,
}

#[derive(Clone, Debug)]
enum Amount {
    /// value * 10^scale (Go int64Amount; Scale is int32)
    Int { value: i64, scale: i32 },
    /// unscaled * 10^-scale (Go inf.Dec; inf.Scale is int32, "upside down")
    Dec { unscaled: BigInt, scale: i32 },
}

#[derive(Clone, Debug)]
pub struct Quantity {
    amount: Amount,
}

#[derive(Debug, PartialEq)]
pub enum QuantityError {
    FormatWrong,
    Numeric,
    Suffix,
}

const MAX_INT64_FACTORS: i32 = 18;
pub const MAX_MILLI_VALUE: i64 = i64::MAX / 1000;

fn is_suffix_char(b: u8) -> bool {
    b"eEinumkKMGTP".contains(&b)
}

/// quantity.go:161 parseQuantityString. Returns (positive, value, num, denom, suffix).
fn parse_quantity_string(s: &str) -> Result<(bool, &str, String, &str, &str), QuantityError> {
    let b = s.as_bytes();
    let end = b.len();
    let mut positive = true;
    let mut pos = 0;
    if pos < end {
        match b[0] {
            b'-' => {
                positive = false;
                pos += 1
            }
            b'+' => pos += 1,
            _ => {}
        }
    }
    // strip leading zeros
    let mut i = pos;
    loop {
        if i >= end {
            return Ok((positive, "0", "0".into(), "", ""));
        }
        if b[i] == b'0' {
            pos += 1;
            i += 1;
        } else {
            break;
        }
    }
    // numerator
    let mut i = pos;
    let num;
    loop {
        if i >= end {
            return Ok((positive, &s[..end], s[pos..end].to_string(), "", ""));
        }
        if b[i].is_ascii_digit() {
            i += 1;
        } else {
            num = &s[pos..i];
            pos = i;
            break;
        }
    }
    let num = if num.is_empty() {
        "0".to_string()
    } else {
        num.to_string()
    };
    let mut denom = "";
    if pos < end && b[pos] == b'.' {
        pos += 1;
        let mut i = pos;
        loop {
            if i >= end {
                return Ok((positive, &s[..end], num, &s[pos..end], ""));
            }
            if b[i].is_ascii_digit() {
                i += 1;
            } else {
                denom = &s[pos..i];
                pos = i;
                break;
            }
        }
    }
    let value = &s[..pos];
    let suffix_start = pos;
    let mut i = pos;
    loop {
        if i >= end {
            return Ok((positive, value, num, denom, &s[suffix_start..end]));
        }
        if !is_suffix_char(b[i]) {
            pos = i;
            break;
        }
        i += 1;
    }
    if pos < end && (b[pos] == b'-' || b[pos] == b'+') {
        pos += 1;
    }
    let mut i = pos;
    loop {
        if i >= end {
            return Ok((positive, value, num, denom, &s[suffix_start..end]));
        }
        if b[i].is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    Err(QuantityError::FormatWrong)
}

/// suffix.go:88 fastLookup.interpret + :180 suffixHandler.interpret -> (base, exponent, format)
fn interpret(suf: &str) -> Option<(i32, i32, Format)> {
    use Format::*;
    Some(match suf {
        "" => (10, 0, DecimalSI),
        "n" => (10, -9, DecimalSI),
        "u" => (10, -6, DecimalSI),
        "m" => (10, -3, DecimalSI),
        "k" => (10, 3, DecimalSI),
        "M" => (10, 6, DecimalSI),
        "G" => (10, 9, DecimalSI),
        "T" => (10, 12, DecimalSI),
        "P" => (10, 15, DecimalSI),
        "E" => (10, 18, DecimalSI),
        "Ki" => (2, 10, BinarySI),
        "Mi" => (2, 20, BinarySI),
        "Gi" => (2, 30, BinarySI),
        "Ti" => (2, 40, BinarySI),
        "Pi" => (2, 50, BinarySI),
        "Ei" => (2, 60, BinarySI),
        _ => {
            let bs = suf.as_bytes();
            if bs.len() > 1 && (bs[0] == b'e' || bs[0] == b'E') {
                // strconv.ParseInt(.., 10, 64) then int32(...) truncation
                let p: i64 = suf[1..].parse().ok()?;
                (10, p as i32, DecimalExponent)
            } else {
                return None;
            }
        }
    })
}

/// inf.Dec SetString (inf.v0 dec.go:470 scan + :525): optional sign first, digits, one '.'
fn inf_set_string(s: &str) -> Option<(BigInt, i32)> {
    let mut digits = String::new();
    let (mut dp, mut seen_digit) = (None, false);
    for (idx, c) in s.char_indices() {
        match c {
            '+' | '-' if idx == 0 => digits.push(c),
            '.' if dp.is_none() => {
                dp = Some(digits.trim_start_matches(['+', '-']).len());
            }
            '0'..='9' => {
                seen_digit = true;
                digits.push(c)
            }
            _ => return None,
        }
    }
    if !seen_digit {
        return None;
    }
    let ndig = digits.trim_start_matches(['+', '-']).len();
    let scale = dp.map(|d| (ndig - d) as i32).unwrap_or(0);
    Some((digits.parse().ok()?, scale))
}

fn pow10_big(n: u32) -> BigInt {
    num_traits::pow(BigInt::from(10), n as usize)
}

/// inf.Dec Round(x, s, inf.RoundUp) for x >= 0 (callers pass magnitude).
fn round_up_to_scale(u: &BigInt, scale: i32, s: i32) -> BigInt {
    if scale <= s {
        u * pow10_big((s as i64 - scale as i64) as u32)
    } else {
        let (q, r) = u.div_mod_floor(&pow10_big((scale as i64 - s as i64) as u32));
        if r.is_zero() { q } else { q + 1 }
    }
}

/// `resource.ParseQuantity`.
pub fn parse(s: &str) -> Result<Quantity, QuantityError> {
    if s.is_empty() {
        return Err(QuantityError::FormatWrong);
    }
    if s == "0" {
        return Ok(zero());
    }
    let (positive, value, num, denom, suf) = parse_quantity_string(s)?;
    let (base, exponent, format) = interpret(suf).ok_or(QuantityError::Suffix)?;
    let precision: i32;
    let mut scale: i32 = 0;
    let mut mantissa: i64 = 1;
    match format {
        Format::DecimalExponent | Format::DecimalSI => {
            scale = exponent;
            precision = MAX_INT64_FACTORS - (num.len() + denom.len()) as i32;
        }
        Format::BinarySI => {
            if exponent >= 0 && denom.is_empty() {
                mantissa <<= exponent as u64;
                precision = 15 - num.len() as i32 - (exponent * 3 / 10) - 1;
            } else {
                precision = -1;
            }
        }
    }
    if precision >= 0 {
        scale = scale.wrapping_sub(denom.len() as i32);
        if scale >= -9 {
            let shifted = format!("{num}{denom}");
            let v: i64 = shifted.parse().map_err(|_| QuantityError::Numeric)?;
            if let Some(mut result) = v.checked_mul(mantissa) {
                if !positive {
                    result = -result;
                }
                return Ok(Quantity {
                    amount: Amount::Int {
                        value: result,
                        scale,
                    },
                });
            }
        }
    }
    let (mut u, mut dscale) = inf_set_string(value).ok_or(QuantityError::Numeric)?;
    if base == 10 {
        dscale = dscale.wrapping_sub(exponent);
    } else {
        u <<= exponent as usize;
    }
    let negative = u.sign() == Sign::Minus;
    let mut mag = u.abs();
    if !mag.is_zero() {
        mag = round_up_to_scale(&mag, dscale, 9);
        dscale = 9;
    }
    // BinarySI cap at MaxInt64 (scale 0)
    if format == Format::BinarySI {
        let as_scaled_cmp = cmp_dec(&mag, dscale, &BigInt::from(i64::MAX), 0);
        if as_scaled_cmp == std::cmp::Ordering::Greater {
            mag = BigInt::from(i64::MAX);
            dscale = 0;
        }
    }
    if negative {
        mag = -mag;
    }
    Ok(Quantity {
        amount: Amount::Dec {
            unscaled: mag,
            scale: dscale,
        },
    })
}

fn cmp_dec(a: &BigInt, sa: i32, b: &BigInt, sb: i32) -> std::cmp::Ordering {
    // a*10^-sa vs b*10^-sb
    if sa == sb {
        return a.cmp(b);
    }
    if sa > sb {
        a.cmp(&(b * pow10_big((sa - sb) as u32)))
    } else {
        (a * pow10_big((sb - sa) as u32)).cmp(b)
    }
}

/// big.Int.Int64(): low 64 bits of |x|, negated if x < 0 (wrapping)
fn big_int64(x: &BigInt) -> i64 {
    let (sign, digits) = x.to_u64_digits();
    let low = digits.first().copied().unwrap_or(0) as i64;
    if sign == Sign::Minus {
        low.wrapping_neg()
    } else {
        low
    }
}

/// Go int64(math.Pow10(n)) on amd64: out-of-range float->int conversion yields i64::MIN.
fn go_int64_pow10(n: i64) -> i64 {
    if (0..=18).contains(&n) {
        10i64.pow(n as u32)
    } else {
        i64::MIN
    }
}

fn positive_scale_i64(v: i64, k: i32) -> Option<i64> {
    if k <= 0 {
        return Some(v);
    }
    if v == 0 {
        return Some(0);
    }
    let mut x = v;
    for _ in 0..k {
        x = x.checked_mul(10)?;
    }
    Some(x)
}
/// math.go:140 positiveScaleInt64 as seen by ScaledValue (which ignores `ok`): for k in
/// {1,2,3,6,9} the specialised helpers return the WRAPPED product even on overflow; for every
/// other k the generic loop returns 0 on overflow.
fn positive_scale_i64_wrapping(v: i64, k: i32) -> i64 {
    match k {
        i32::MIN..=0 => v,
        1 | 2 | 3 | 6 | 9 => {
            if v == i64::MIN {
                0
            } else {
                v.wrapping_mul(10i64.pow(k as u32))
            }
        }
        _ => positive_scale_i64(v, k).unwrap_or(0),
    }
}
fn negative_scale_i64(v: i64, k: i32) -> (i64, bool) {
    if k == 0 {
        return (v, true);
    }
    let (q, r) = if k >= 19 {
        (0, v)
    } else {
        let d = 10i64.pow(k as u32);
        (v / d, v % d)
    };
    if r != 0 {
        (if v > 0 { q + 1 } else { q - 1 }, false)
    } else {
        (q, true)
    }
}

impl Quantity {
    /// quantity.go:828 ScaledValue
    pub fn scaled_value(&self, scale: i32) -> i64 {
        match &self.amount {
            Amount::Int { value, scale: s } => {
                if *s < scale {
                    negative_scale_i64(*value, scale.wrapping_sub(*s)).0
                } else {
                    positive_scale_i64_wrapping(*value, s.wrapping_sub(scale))
                }
            }
            Amount::Dec { unscaled, scale: s } => {
                let dif = *s as i64 - (-(scale as i64));
                if dif == 0 {
                    return big_int64(unscaled);
                }
                if dif < 0 {
                    return big_int64(unscaled).wrapping_mul(go_int64_pow10(-dif));
                }
                if *unscaled < BigInt::from(i64::MAX) && dif < 19 {
                    let u = big_int64(unscaled);
                    let d = 10i64.pow(dif as u32);
                    return if u % d != 0 { u / d + 1 } else { u / d };
                }
                let (q, r) = unscaled.div_mod_floor(&pow10_big(dif as u32));
                if r.is_zero() {
                    big_int64(&q)
                } else {
                    big_int64(&q).wrapping_add(1)
                }
            }
        }
    }
    pub fn value(&self) -> i64 {
        self.scaled_value(0)
    }
    pub fn milli_value(&self) -> i64 {
        self.scaled_value(-3)
    }
    pub fn is_zero(&self) -> bool {
        match &self.amount {
            Amount::Int { value, .. } => *value == 0,
            Amount::Dec { unscaled, .. } => unscaled.is_zero(),
        }
    }

    /// quantity.go:468 AsApproximateFloat64
    pub fn as_approximate_f64(&self) -> f64 {
        let (base, exponent) = match &self.amount {
            Amount::Dec { unscaled, scale } => (big_to_f64(unscaled), -(*scale as i64)),
            Amount::Int { value, scale } => (*value as f64, *scale as i64),
        };
        if exponent == 0 {
            return base;
        }
        base * go_pow10(exponent)
    }
}

/// math.Pow10 (math/pow10.go:31) with Go's tables.
fn go_pow10(n: i64) -> f64 {
    const TAB: [f64; 32] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22, 1e23, 1e24, 1e25, 1e26, 1e27, 1e28, 1e29, 1e30, 1e31,
    ];
    const POS32: [f64; 10] = [
        1e0, 1e32, 1e64, 1e96, 1e128, 1e160, 1e192, 1e224, 1e256, 1e288,
    ];
    const NEG32: [f64; 11] = [
        1e-0, 1e-32, 1e-64, 1e-96, 1e-128, 1e-160, 1e-192, 1e-224, 1e-256, 1e-288, 1e-320,
    ];
    if (0..=308).contains(&n) {
        return POS32[(n / 32) as usize] * TAB[(n % 32) as usize];
    }
    if (-323..0).contains(&n) {
        let m = -n;
        return NEG32[(m / 32) as usize] / TAB[(m % 32) as usize];
    }
    if n > 0 { f64::INFINITY } else { 0.0 }
}

/// big.Float.SetInt(x).Float64(): exact integer rounded to nearest, ties to even.
fn big_to_f64(x: &BigInt) -> f64 {
    let neg = x.is_negative();
    let mag = x.magnitude();
    let bits = mag.bits();
    let f = if bits <= 64 {
        mag.to_u64().unwrap_or(u64::MAX) as f64 /* INFALLIBLE: bits <= 64 */
    } else {
        let shift = bits - 64;
        let top = (mag >> shift).to_u64().unwrap_or(u64::MAX); // INFALLIBLE: exactly 64 bits remain
        let sticky = mag.trailing_zeros().is_some_and(|tz| tz < shift);
        let top = top | sticky as u64; // 64 > 53+2, so OR-ing the sticky bit keeps rounding correct
        let f = top as f64;
        if shift > 2000 {
            f64::INFINITY
        } else {
            f * 2f64.powi(shift as i32)
        }
    };
    if neg { -f } else { f }
}

/// `convertValueToFloat64` of `internal/store/utils.go`.
pub fn ksm_float(q: &Quantity) -> f64 {
    if q.value() > MAX_MILLI_VALUE {
        q.value() as f64
    } else {
        q.milli_value() as f64 / 1000.0
    }
}

impl std::fmt::Display for QuantityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            QuantityError::FormatWrong => "quantities must match the regular expression '^([+-]?[0-9.]+)([eEinumkKMGTP]*[-+]?[0-9]*)$'",
            QuantityError::Numeric => "unable to parse numeric part of quantity",
            QuantityError::Suffix => "unable to parse quantity's suffix",
        })
    }
}

impl std::error::Error for QuantityError {}

/// The quantity a k8s-openapi `Quantity` holds. The API server only sends
/// valid quantities (Go fails to decode the whole object otherwise), so an
/// unparseable one is treated as zero.
pub fn from_k8s(q: &k8s_openapi::apimachinery::pkg::api::resource::Quantity) -> Quantity {
    parse(&q.0).unwrap_or_else(|_| zero())
}

/// The zero quantity.
pub fn zero() -> Quantity {
    Quantity {
        amount: Amount::Int { value: 0, scale: 0 },
    }
}

/// `float64(q.Value())`.
#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts int64 to float64 the same way"
)]
pub fn value_f64(q: &k8s_openapi::apimachinery::pkg::api::resource::Quantity) -> f64 {
    from_k8s(q).value() as f64
}

/// `convertValueToFloat64(&q)` of a k8s-openapi quantity.
pub fn ksm_f64(q: &k8s_openapi::apimachinery::pkg::api::resource::Quantity) -> f64 {
    ksm_float(&from_k8s(q))
}
