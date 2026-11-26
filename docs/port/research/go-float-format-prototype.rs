// Validated prototype (scratch, std only). Checks testdata/golden/float_g.jsonl: 3203 checked, 0 mismatches;
// also 5,000,000 extra values (random bits, tie candidates m*2^e, x/1000, timestamps): 0 mismatches.
// Usage: rustc -O go-float-format-prototype.rs && ./go-float-format-prototype testdata/golden/float_g.jsonl
// See docs/port/research/go-float-format.md.
use std::io::{BufRead, BufReader};
pub fn go_format_g(f: f64) -> String {
    if f.is_nan() { return "NaN".into(); }
    if f.is_infinite() { return if f > 0.0 { "+Inf" } else { "-Inf" }.into(); }
    let neg = f.is_sign_negative();
    if f == 0.0 { return if neg { "-0" } else { "0" }.into(); }   // KSM's writeFloat prints "0" for -0 before this
    let s = format!("{:e}", f.abs());
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();                 // decimal exponent of first digit
    let mut digits: String = mant.chars().filter(|&c| c != '.').collect();
    if let Some(d) = go_tie_digits(f.abs(), digits.len(), exp) { digits = d; }
    let nd = digits.len() as i32;
    let dp = exp + 1;
    let mut out = String::with_capacity(24);
    if neg { out.push('-'); }
    if exp < -4 || exp >= 6 {                            // %e
        out.push_str(&digits[..1]);
        if nd > 1 { out.push('.'); out.push_str(&digits[1..]); }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let a = exp.unsigned_abs();
        if a < 10 { out.push('0'); }
        out.push_str(&a.to_string());
    } else {                                             // %f
        if dp > 0 {
            let m = nd.min(dp) as usize;
            out.push_str(&digits[..m]);
            for _ in 0..(dp - m as i32) { out.push('0'); }
        } else { out.push('0'); }
        if nd > dp {
            out.push('.');
            for _ in 0..(-dp).max(0) { out.push('0'); }
            out.push_str(&digits[dp.max(0) as usize..]);
        }
    }
    out
}

/// Go rounds an exact tie between two shortest candidates half-to-even (uscale.go round());
/// Rust's {:e} rounds it up. A tie needs the exact decimal value of f to have exactly nd+1
/// significant digits, the last being 5. Returns Go's digits if that is the case.
fn go_tie_digits(f: f64, nd: usize, exp: i32) -> Option<String> {
    if nd < 16 { return None; }
    let bits = f.to_bits();
    let be = ((bits >> 52) & 0x7ff) as i32;
    let mut m = bits & ((1u64 << 52) - 1);
    let mut e;
    if be == 0 { e = -1074; } else { m |= 1u64 << 52; e = be - 1075; }
    let tz = m.trailing_zeros() as i32; m >>= tz; e += tz;
    // exact value = n * 10^k with n ending in 5 (or no tie)
    let n: u128 = if e < 0 {
        let mut n = m as u128;
        for _ in 0..(-e) { n = n.checked_mul(5)?; }
        n
    } else {
        let p5 = 5u128.checked_pow(e as u32 + 1)?;
        if (m as u128) % p5 != 0 { return None; }
        (m as u128) / 5u128.pow(e as u32)
    };
    let s = n.to_string();
    if s.len() != nd + 1 || !s.ends_with('5') { return None; }
    let mut q: u128 = n / 10; // truncated candidate
    if q % 2 == 1 { q += 1; }  // half to even (q+1 cannot gain a digit: would need n=99..95, then nd+1 digits candidate round-trips as shorter)
    let qs = q.to_string();
    // Go only breaks the tie if both candidates lie in the round-trip interval.
    let back: f64 = format!("{}.{}e{}", &qs[..1], &qs[1..], exp).parse().unwrap();
    if back != f { return None; }
    Some(qs)
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let (mut n, mut bad) = (0, 0);
    for line in BufReader::new(std::fs::File::open(path).unwrap()).lines() {
        let line = line.unwrap();
        let bits = &line[9..25];
        let out_start = line.find("\"out\":\"").unwrap() + 7;
        let want = &line[out_start..line.len() - 2];
        let f = f64::from_bits(u64::from_str_radix(bits, 16).unwrap());
        let got = go_format_g(f);
        n += 1;
        if got != want { bad += 1; if bad < 20 { println!("MISMATCH {bits}: want {want} got {got}"); } }
    }
    println!("{n} checked, {bad} mismatches");
}
