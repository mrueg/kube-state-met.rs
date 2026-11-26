# Go `strconv.FormatFloat(f, 'g', -1, 64)` and how to reproduce it in Rust

Status: research, verified. A std-only Rust prototype (`docs/port/research/go-float-format-prototype.rs`)
matches all 3203 lines of `testdata/golden/float_g.jsonl`. It also matches 5,000,000 extra values
(random bit patterns, ties of the form m·2^e, x/1000, timestamps) generated with Go 1.27.4.

Sources: `GO/` = `$(go env GOROOT)/src` of go1.27.4. `KSM/` = kube-state-metrics at `46cd52ff`.

## 1. Where KSM uses it

`KSM/pkg/metric/metric.go:121-143` `writeFloat` writes every sample value. It special-cases
`f == 1` → `1`, `f == 0` → `0` (**this includes -0**, so KSM never prints `-0`), `f == -1` → `-1`,
NaN → `NaN`, ±Inf → `+Inf`/`-Inf`. Everything else goes to `strconv.AppendFloat(buf, f, 'g', -1, 64)`.
ADR (`docs/port/DECISIONS.md:21`) requires byte-for-byte equality.

## 2. What Go does

`strconv.FormatFloat` forwards to `internal/strconv` (`GO/strconv/number.go:189-190`), which calls
`ftoa64` (`GO/internal/strconv/ftoa.go:59-67,217`).

1. **Specials** (`ftoa.go:237-245`): NaN → `NaN` (any payload, any sign), +Inf → `+Inf`, -Inf → `-Inf`.
2. **Zero** (`ftoa.go:262-264`): `fmtEFG(neg, nil, dp=0, nd=0, prec=-1, 'g', shortest=true)` →
   `0` or `-0`.
3. **Shortest digits** (`ftoa.go:267-284`): `shortFloat` (`GO/internal/strconv/uscale.go:169-216`)
   returns the decimal `d·10^p` with the fewest digits that parses back to `f`. When two
   candidates with that many digits both lie in the round-trip interval, it takes the one
   nearest to `f`, and on an **exact tie it rounds half to even**: `uscale(m).round()` (`uscale.go:212-213`),
   where `round()` = `(u + 1 + (u>>2)&1) >> 2` (`uscale.go:55`). The interval bounds are
   inclusive iff the mantissa is even (`nudge(±odd)`, `uscale.go:206-207`). For powers of two
   the lower bound is closer (1/4 ulp, `uscale.go:186-190`). `setDigits` gives digits `s[0:nd]` and
   decimal-point position `dp` (value = 0.s × 10^dp). For `'g'`, `prec = nd` (`ftoa.go:281-282`).
4. **%e vs %f** (`fmtEFG`, `ftoa.go:347-374`):
   ```
   eprec := prec; if eprec > nd && nd >= dp { eprec = nd }
   if shortest { eprec = 6 }          // the "eprec = 6" rule
   exp := dp - 1                       // decimal exponent of the first digit
   if exp < -4 || exp >= eprec { %e with prec = min(prec,nd) - 1 }
   else                         { %f with prec = max((prec > dp ? nd : prec) - dp, 0) }
   ```
   With shortest output that means: **`%f` iff -4 ≤ exp < 6**, i.e. 1e-4 ≤ |x| < 1e6 after
   shortest rounding. Otherwise `%e`. So `100000` but `1e+06`, `0.0001` but `1e-05`, and every
   Unix timestamp prints as `%e`: `1.501569018e+09`. The 6 does not depend on how many digits
   there are: `123456.7` → `123456.7`, `1234567` → `1.234567e+06`, `999999.9999999999` stays `%f`.
5. **%e layout** (`ftoa.go:377-427`): optional `-`, first digit, then `.` and the remaining
   `nd-1` digits if nd > 1 (no trailing zeros, because shortest digits never end in 0), then
   `e`, then **always a sign** (`+`/`-`), then the exponent with **at least two digits** (`05`, `21`,
   `308`, `324`). Three digits only when |exp| ≥ 100.
6. **%f layout** (`ftoa.go:429-465`): optional `-`. Integer part: the first `min(nd,dp)` digits
   plus `dp-nd` zeros if dp > nd, or `0` if dp ≤ 0. Then, if `nd > dp`: `.`, `max(0,-dp)` leading
   zeros, and the remaining digits. No exponent, no trailing zeros, no trailing `.`.

The tie case is not hypothetical. 2^-25 = 2.98023223876953125e-08 exactly, and both
`2.9802322387695312e-08` and `…313e-08` round-trip. Go prints `…312` (even). Rust prints `…313`.
The golden file has three such values: bits `3e60000000000000`, `3eba000000000000`, `402ba13200000000`.

Version stability: a 5,000,000-value run produced byte-identical output with go1.20 (old Ryū
code, `strconv/ftoaryu.go`) and go1.27.4 (new `uscale.go`). The tie-to-even behaviour is the same in both.

## 3. Reproducing it in Rust

Rust's `format!("{:e}", x)` (rustc 1.100) gives the same shortest digit count and the same
digits, **except on exact ties, where it rounds up**. That is the only difference found. It
gives `"<d>[.<ddd>]e<exp>"` with no `+` and no zero padding (`1e6`, `1.5e-7`, `5e-324`).

```rust
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
```

**Tie fix-up** (`go_tie_digits`). An exact tie between two `nd`-digit candidates needs the
exact value of `f` to have exactly `nd+1` significant decimal digits, the last one being 5:

1. If `nd < 16`, return None. For normal doubles the digit spacing at ≤15 digits (≥1e-15 relative)
   is wider than the round-trip interval (≤2.2e-16 relative), so two candidates cannot both fit.
   Subnormals have ~750-digit exact expansions and so never tie.
2. Write `f = m·2^e` with `m` odd (strip trailing zero bits of the 53-bit mantissa).
3. If `e < 0`: `n = m·5^(-e)` (u128, `checked_mul`; overflow → None). Then `f = n·10^e` exactly,
   and `n` always ends in 5. If `e ≥ 0`: a tie requires `m % 5^(e+1) == 0`; then `n = m / 5^e`.
4. If `n` has exactly `nd+1` digits and ends in `5`: `q = n/10`; if `q` is odd, `q += 1` (half to even).
5. Go only applies tie-breaking when **both** candidates round-trip (`uscale.go:209-214`). Parse
   `q` with the same exponent (`"{q0}.{q_rest}e{exp}"`) and return `Some(q)` only if it gives back `f`.
   This matters for powers of two, whose interval is asymmetric: 2^-24 = 5.9604644775390625e-08
   prints as `5.960464477539063e-08` in Go because `…062` does not round-trip.

Cost: the fix-up runs only for 16–17-digit outputs, and the u128 work is at most 25 multiplications.
KSM values (integers, x/1000 values, timestamps) almost never hit the parse in step 5.

Alternatives considered: the `ryu` crate (same shortest digits, but its own `e` formatting and
the same tie question; not checked), and copying Go's `uscale.go` (~300 lines plus a
pow10 table). Neither is needed: std's `{:e}` plus the fix-up is exact on every value tested.
Pin a test on `float_g.jsonl` in case a future rustc changes `{:e}`.

## 4. Golden vectors

`testdata/golden/float_g.jsonl` is written by `tools/golden-go/floatfmt/main.go`
(`cd tools/golden-go/floatfmt && go run . > ../../../testdata/golden/float_g.jsonl`; std only,
fixed PCG seed). It has 3203 lines `{"bits":"<16 lowercase hex digits of Float64bits>","out":"<FormatFloat g -1 64>"}`,
deduplicated by bits: ±0, ±Inf, two NaN payloads; integers -120…120; 2^-60…2^70; 2^53±, ±2^63
and other int64 limits; binary sizes; powers of ten 1e-30…1e30 (literal, `math.Pow10`, ±1 ulp,
negated, 1.5/9.999999/1.23456789 mantissas); the %e/%f boundaries (1e-5, 1e-4, 1e5, 1e6,
999999.9…, 1e20, 1e21, 1e22, 1e23, each ±1 ulp and negated); typical arithmetic results (0.1+0.2, 1/3, π …);
MaxFloat64, smallest normal and subnormal, random subnormals; tie candidates m·2^e for e in -60…-1;
600 Unix timestamps (integer, ms fractions, random fractions, epoch ms); 300 milli/1000 values
(KSM's `convertValueToFloat64`); 500 random finite bit patterns; 300 random values across 1e-12…1e12;
200 short decimals across 1e-30…1e30.

Note that `out` is the raw `FormatFloat` result: `-0` appears there although KSM's `writeFloat`
would print `0`.
