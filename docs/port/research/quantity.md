# `resource.Quantity`: what kube-state-metrics needs and how to port it

Status: research, verified. Every behaviour below was checked against the Go code and against
`testdata/golden/quantity.jsonl` (813 inputs). A Rust prototype that follows this document
(`docs/port/research/quantity-prototype.rs`) reproduces all 813 lines: `value`, `milli`,
`approx`, `ksm_float`, `string` and the error kind.

Source abbreviations (all primary sources, as checked out on this machine):

| Prefix | Path |
|---|---|
| `AM/` | `$(go env GOMODCACHE)/k8s.io/apimachinery@v0.37.2/pkg/api/resource/` |
| `INF/` | `$(go env GOMODCACHE)/gopkg.in/inf.v0@v0.9.1/` (the version KSM's `go.mod` resolves) |
| `GO/` | `$(go env GOROOT)/src/` (go1.27.4) |
| `KSM/` | `/home/mrueg/repos/kube-state-metrics` at `46cd52ff` (`go.mod:23` pins apimachinery v0.37.2) |

## 1. What kube-state-metrics calls

Only these four entry points are used. Nobody calls `AsDec`, `Cmp`, `String` or `IsZero` on a
Quantity. Every `IsZero` hit in `internal/store` is a `metav1.Time`.

| API | Call sites |
|---|---|
| `convertValueToFloat64(q)` = `if q.Value() > MaxMilliValue { float64(q.Value()) } else { float64(q.MilliValue())/1000 }` | defined at `KSM/internal/store/utils.go:418-423`. Used by: node allocatable `node.go:356,368,376,385,394,403` (cpu, memory/storage/ephemeral-storage, pods, hugepages, attachable volumes, extended resources); node capacity `node.go:439,451,459,468,477,486`; pod container limits/requests `pod.go:195,259` (cpu only); init-container limits/requests `pod.go:825,889` (cpu only); `kube_pod_overhead_cpu_cores` `pod.go:1287`; limit range `limitrange.go:52,59,66,73,80`; resource quota `resourcequota.go:77,83`; HPA spec target and status current value/averageValue `horizontalpodautoscaler.go:248,251,328,331` |
| `float64(q.Value())` | pod container limits/requests for memory, storage, ephemeral-storage, hugepages, attachable volumes and extended resources `pod.go:204,210,215,221,268,274,280,286`; the same for init containers `pod.go:834,840,845,851,898,904,910,916`; `kube_pod_overhead_memory_bytes` `pod.go:1313`; `kube_persistentvolume_capacity_bytes` `persistentvolume.go:361` (a missing `storage` key gives a zero Quantity, so 0); PVC `requests.storage` `persistentvolumeclaim.go:257` (only if the key is present) |
| `q.AsApproximateFloat64()` | HPA `behavior.scaleUp/scaleDown.tolerance` `horizontalpodautoscaler.go:524,547` |
| `resource.ParseQuantity(s)` + `AsApproximateFloat64()` | custom-resource-state `toFloat64` `KSM/pkg/customresourcestate/registry_factory.go:793-794`, tried after "true/yes/false/no/unknown" (`:781-787`) and after `time.Parse(time.RFC3339)` (`:789-791`), and before percent (`:797-800`) and `strconv.ParseFloat` (`:802`). Every string that ParseQuantity accepts never reaches `ParseFloat` |

Where the strings come from:

- Typed objects: KSM asks for protobuf (`KSM/pkg/util/utils.go:51-52`). The protobuf decoder calls
  `ParseQuantity(s)` without trimming (`AM/quantity_proto.go:151`). The JSON decoder trims first
  (`AM/quantity.go:744`, `strings.TrimSpace`). The API server always sends the canonical
  `String()` form, so the Rust port (k8s-openapi, JSON) should parse the string as is, with no
  trimming. Then it matches KSM's protobuf path exactly.
- Custom resources: whatever string sits in the unstructured object, with no trimming
  (`registry_factory.go:793`). Here non-canonical, odd and invalid input really happens.

## 2. Grammar actually accepted by `ParseQuantity`

The documented BNF (`AM/quantity.go:40-58`) and the regex in the error message
(`AM/quantity.go:150`) are **not** what the code checks. The real scanner is
`parseQuantityString` (`AM/quantity.go:161-275`) plus the suffix lookup (`AM/suffix.go:88-106,180-198`).

```
input     := sign? zeros digits? ('.' digits?)? suffixch* (('+'|'-')? digits?)    -- else ErrFormatWrong
sign      := '+' | '-'                                     (quantity.go:167-175)
zeros     := '0'*   stripped; if the string ends inside them: num="0", value="0" (178-191)
num       := ASCII digits after the zeros; "" becomes "0"  (194-213)
denom     := ASCII digits after one '.'; may be empty ("1." and "1.G" are OK)  (216-238)
suffixch  := any of e E i n u m k K M G T P                (243-252)
```

Then the suffix text (everything after the number, including the optional sign and digits) is
looked up (`AM/suffix.go`):

| Suffix | base, exponent, format | Source |
|---|---|---|
| `""` `n` `u` `m` `k` `M` `G` | 10; 0, -9, -6, -3, 3, 6, 9; DecimalSI | fast path `suffix.go:89-104` |
| `T` `P` `E` | 10; 12, 15, 18; DecimalSI | table `suffix.go:130-132`, checked first at `:182` |
| `Ki` `Mi` `Gi` `Ti` `Pi` `Ei` | 2; 10, 20 … 60; BinarySI | `suffix.go:113-118` |
| `e<int>` / `E<int>` (length > 1) | 10; `int32(strconv.ParseInt(rest,10,64))`; DecimalExponent | `suffix.go:189-195` |
| anything else | `ErrSuffix` | `suffix.go:197`, `quantity.go:291-294` |

Consequences, all present in the golden file:

- `E` alone is exa (decimal SI), not an exponent: `"1E"` = 1e18. `"1e"` → `ErrSuffix`.
- Exponents go through `ParseInt` (it accepts a leading `+`/`-`, rejects `""`) and are then
  **cast to int32 with wrap-around**: `"1e4294967296"` parses as 1 and `"1e4294967299"` as 1000.
  `"1e9223372036854775808"` → `ErrSuffix` (ParseInt range error).
- Case matters: `1K`, `1ki`, `1KI`, `1mi`, `1g`, `1MB`, `1KiB` → `ErrSuffix`. A digit after a
  non-exponent suffix (`1Ki5`, `1n1`, `1G1`) → `ErrSuffix`. A non-suffix character after the
  suffix (`1e3.5`, `1e3m`, `1Gi `) → `ErrFormatWrong`.
- There is no whitespace handling: `" 1"` and `"1 "` → `ErrFormatWrong`. Only ASCII digits.
- Input without digits parses as **zero**: `"."`, `"+"`, `"-"`, `"+."`, `"m"`, `"k"`, `"E"`,
  `"Ki"`, `"Ti"`, `".Ki"`, `"e3"`. **Except** when it takes the inf.Dec path (below), where
  `inf.Dec.SetString` needs at least one digit (`INF/dec.go:505-507`): `"Ei"`, `"Pi"`, `".Ei"`,
  `"-Ei"`, `".e-10"`, `"e-10"` → `ErrNumeric`.
- `""` → `ErrFormatWrong` (`quantity.go:279-281`). `"0"` has its own fast path (`:282-284`).
- Error texts: `ErrFormatWrong` = "quantities must match the regular expression
  '^([+-]?[0-9.]+)([eEinumkKMGTP]*[-+]?[0-9]*)$'", `ErrNumeric` = "unable to parse numeric part of
  quantity", `ErrSuffix` = "unable to parse quantity's suffix" (`quantity.go:155-157`). KSM only
  cares whether parsing failed. The golden file records the text.

### 2.1 Two internal representations (`AM/quantity.go:296-391`)

A Quantity is either `int64Amount{value int64, scale int32}` = value·10^scale
(`AM/amount.go:58-61`) or `inf.Dec` = unscaled·10^(-scale), arbitrary precision.

**int64 fast path** (`quantity.go:296-346`):

- Decimal SI and exponent forms: `scale = exponent`, `precision = 18 - len(num) - len(denom)`
  (`maxInt64Factors = 18`, `AM/math.go:28`). `num` has its leading zeros stripped.
- BinarySI without a fraction (`len(denom)==0`; `"1.Gi"` counts as no fraction): `mantissa = 1<<exponent`,
  `precision = 15 - len(num) - exponent*3/10 - 1` (`:306-310`). So Ki allows up to 11 digits,
  Mi 8, Gi 5, Ti 2, and **Pi and Ei always take the inf.Dec path**. A BinarySI value with a
  fraction always takes inf.Dec (`:311-312`).
- Taken if `precision >= 0` and `scale - len(denom) >= -9`. Then `value = ParseInt(num+denom)`,
  `result = value*mantissa` (overflow-checked; on overflow it falls through to inf.Dec), negated if the sign was `-`.
- **String cache** (`:332-342`): the input string itself is kept as `q.s` and later returned by
  `String()` if (BinarySI) `value&7 != 0`, or (decimal) `scale%3==0 && !HasSuffix(num+denom,"000") && (num+denom)[0] != '0'`.
  So `String()` of `"+5"` is `"+5"`, of `"0001"` is `"0001"`, of `"1.500"` is `"1.500"`, and of `"1E3"` is `"1E3"`.

**inf.Dec path** (`quantity.go:348-391`):

1. `amount.SetString(value)`, where `value` is the original text up to the end of the fraction,
   sign included (`INF/dec.go:470-537`). Scale = number of fraction digits.
2. base 10: `scale += -exponent` (int32, wraps). base 2: `unscaled <<= exponent` (`:354-361`).
3. Take |amount|. If it is non-zero, **round to scale 9 (nano) with `inf.RoundUp`**, which
   rounds away from zero (`INF/rounder.go:109-116`, `Round` = `QuoRound(x,1,s,r)` `INF/dec.go:243-245`).
   Any non-zero magnitude below 1n becomes 1n (`"1e-400"`, `"0.1n"` → 1n). After this step every
   non-zero Dec has **scale exactly 9**. This was checked by reflection on all 813 inputs.
4. **BinarySI only**: cap at MaxInt64 (`:379-381`). The result is `inf.NewDec(MaxInt64, 0)`, so
   scale 0. Decimal formats are **never capped**, so `"100000000000000000000"` stays 1e20.
5. BinarySI with 0 < |x| < 1 → format DecimalSI (`:383-386`).
6. Restore the sign. Negative binaries are therefore capped at -MaxInt64.

There is no general "max precision" beyond this. Values are exact to 1n (with rounding up away
from zero below that). They are unbounded for decimal formats and capped at ±(2^63-1) for BinarySI.
The type comment (`quantity.go:61-64`) claims a 2^63-1 cap for all forms. The code does not do that.

## 3. Accessors

### 3.1 `Value()` and `MilliValue()`

`Value() = ScaledValue(0)`, `MilliValue() = ScaledValue(Milli=-3)` (`quantity.go:814-822`).
The doc says "ceil(q / 10^scale)" (`:824`). That is true only for non-negative values on the int64 path.

**int64 path**: `int64Amount.AsScaledInt64(scale)` (`amount.go:95-101`). Its `ok` result is
**ignored** by `ScaledValue` (`quantity.go:830-831`):

- `a.scale < scale` → `negativeScaleInt64(value, scale - a.scale)` (`math.go:169-199`):
  truncating division, then if anything was dropped, **±1 away from zero**. So `"-1.5"` → -2 and `"-1n"` → Value -1, Milli -1.
- otherwise → `positiveScaleInt64(value, a.scale - scale)` (`math.go:140-164`), with the scale
  difference computed in **int32 and wrapping**: `"1e2147483647"` has MilliValue 1, because
  2147483647+3 wraps negative and a negative scale returns the value unchanged.
  On overflow:
  - k ∈ {1,2,3,6,9}: the helpers `int64MultiplyScale10/100/1000` and `int64MultiplyScale(…,1e6|1e9)`
    `return c, c/b == a` (`math.go:95-96,108-109,121-122,134-135`). That is the **wrapped product**
    (the caller drops `ok`). Exception: value == MinInt64 → 0.
  - any other k (4, 5, 7, 8, ≥10): the generic loop returns **0** (`math.go:154-163`).
  - Golden: `"100000000000000000e4"` → Value 0. `"100000000000000000e6"` → Value 200376420520689664 (1e23 mod 2^64).
    `"1e19"` → Value 0, Milli 0.

**inf.Dec path**: `scaledValue(unscaled, decScale, -scale)` (`AM/scale_int.go:43-95`) with
`dif = decScale - newScale`:

- `dif == 0` → `unscaled.Int64()`: the low 64 bits of |x|, negated if x<0, **wrapping**
  (`GO/math/big/int.go:512-518`).
- `dif < 0` → `unscaled.Int64() * int64(math.Pow10(-dif))`, wrapping. For -dif > 18 the
  float→int64 conversion is out of range. On amd64 that gives MinInt64 (arm64 saturates). This
  only matters with a non-zero unscaled, which in practice means capped BinarySI (scale 0) with
  MilliValue (dif=-3): `"16Ei"` → Milli = MaxInt64·1000 wrapped = -1000.
- `dif > 0` and `unscaled < MaxInt64` and `dif < 19` (fast path, `:60-69`): `u := unscaled.Int64()`
  (**wraps for unscaled < MinInt64**), `u/10^dif` with Go's truncating `/`, **then +1 if `u%10^dif != 0`**.
  For negative values that is ceil+1, not ceil and not "away from zero":
  `"-1.0000000001"` → Value **0**, Milli **-999**. `"-1.50000000000000000000"` → Value **0**, but
  `"-1.5"` (int64 path) → Value **-2**.
- otherwise: `big.Int.DivMod` (**Euclidean**, `GO/math/big/int.go:346`), `q.Int64()` (+1 if the
  remainder is non-zero), wrapping.

`MaxMilliValue = (2^63-1)/1000 = 9223372036854775` (`AM/math.go:49`). It is "the maximum value
we can represent milli-units for. Compare with the return value of Quantity.Value()" (`:46-48`).

### 3.2 `convertValueToFloat64` (`KSM/internal/store/utils.go:418-423`)

Only the upper bound is checked. Negative values below -MaxMilliValue go through `MilliValue()`
and overflow: `"-9223372036854776"` → ksm_float `9.223372036854776e+15` (wrong sign). Values
whose `Value()` already wrapped to something ≤ MaxMilliValue also go the milli route:
`"9223372036854776k"` → `192`. A positive Value ≤ MaxMilliValue never overflows MilliValue.

### 3.3 `AsApproximateFloat64()` (`AM/quantity.go:468-483`)

```
Dec:   base = big.Float.SetInt(unscaled).Float64()   // exact, then round to nearest, ties even; ±Inf on overflow
       exponent = -decScale                           // -9 for every non-zero non-capped Dec
int64: base = float64(value)                          // round to nearest, ties even
       exponent = scale
if exponent == 0 { return base }
return base * math.Pow10(exponent)
```

- `big.Float.SetInt` uses precision `max(bits,64)`, so it is exact (`GO/math/big/float.go:601-607`).
  `Float64()` returns the nearest float64 (`:955-960`).
- `math.Pow10` is table-based, **not** correctly rounded for every n (`GO/math/pow10.go:8-47`):
  `0≤n≤308: postab32[n/32]*tab[n%32]`, `-323≤n<0: negtab32[-n/32]/tab[-n%32]`, `n>308: +Inf`,
  `n<-323: 0`. The port must copy these tables and formulas. `f64::powi` or `10f64.powf` will
  not give the same bits.
- One rounding happens in `base`, one in `Pow10`, one in the multiply. Copying the same three
  IEEE operations gives identical bits.
- Traps: zero with a huge exponent gives **NaN** (`"0e400"`: 0·Inf). A huge exponent gives ±Inf
  (`"1e309"`, `"-1e400"`). Neither is clamped. CRS would emit `NaN`/`+Inf` samples.
- AsApproximateFloat64 is not exact even for simple input: `"1.5n"` → `2e-09` (rounded up to 2n
  at parse), `"92233720368547758.1"` → `9.223372036854778e+16`.

### 3.4 `String()` (not used by KSM; recorded for completeness)

`quantity.go:680-691` → the cached `q.s` if set (see the string cache above), else
`CanonicalizeBytes` (`:425-462`):

1. zero → `"0"`.
2. BinarySI: if -1024 < q < 1024 → emit as DecimalSI. Otherwise round to scale 0 (up, away from
   zero). If that is inexact → DecimalSI. If exact → mantissa = value with factors of 1024 removed,
   suffix Ki…Ei (`amount.go:286-293,332-337`). A 1024-exponent with no suffix entry (0) → no suffix.
3. Decimal: remove factors of 10, then lower the exponent to a multiple of 3 (Go's `%`, so the
   remainder is negative for negative exponents) (`amount.go:257-281,312-327`). Suffix: DecimalSI
   table n…E for exponents -9…18. **Any other exponent → no suffix at all**, so `"1000E"` → `"1"`
   and `"1e21"` (DecimalExponent) → `"1e21"`. DecimalExponent → `e<exp>` (lowercase), or nothing for 0.

## 4. Recommended Rust design

**Use `num-bigint` (0.5, with `num-integer` and `num-traits`) for the inf.Dec representation and plain
`i64`/`i32` with explicit wrapping for the int64 representation.** Do not use i128: the Dec path
is reachable from ordinary-looking CRS strings (≥19 significant digits, any fractional binary,
any `Pi`/`Ei`, fractions finer than 1n) and needs `unscaled` far beyond 2^127 (e.g. 30 digits ·1e9).
The wrapped `Int64()` results also depend on all of its low bits. A general big-decimal crate
(`bigdecimal`, `rust_decimal`) is not a good fit: the semantics needed are inf.Dec's specific
`RoundUp`, big.Int's `Int64` wrap and Euclidean `DivMod`, which you end up writing by hand anyway.
rust_decimal's 96-bit mantissa is too small.

```rust
pub enum Format { DecimalExponent, BinarySI, DecimalSI }
enum Amount {
    Int { value: i64, scale: i32 },          // value * 10^scale
    Dec { unscaled: BigInt, scale: i32 },     // unscaled * 10^-scale (scale==9 unless zero or capped)
}
pub struct Quantity { amount: Amount, format: Format, s: Option<String> }
pub enum ParseError { FormatWrong, Numeric, Suffix }

pub fn parse_quantity(s: &str) -> Result<Quantity, ParseError>;
impl Quantity {
    pub fn value(&self) -> i64;               // ScaledValue(0)
    pub fn milli_value(&self) -> i64;         // ScaledValue(-3)
    pub fn as_approximate_f64(&self) -> f64;
    pub fn to_go_string(&self) -> String;     // optional, String()
}
pub const MAX_MILLI_VALUE: i64 = i64::MAX / 1000;
pub fn convert_value_to_f64(q: &Quantity) -> f64; // KSM utils.go:418
```

For typed k8s-openapi `Quantity(String)` fields, parse once per sample. If parsing fails (it cannot
for API-server data), Go would have failed to decode the object. Treat it as a zero Quantity and log.

Implementation notes (all followed by the validated prototype `quantity-prototype.rs`):

1. Scanner: byte-for-byte port of `parseQuantityString`. Work on bytes, ASCII digits only. Keep
   `value` (the slice with sign) for the Dec path and `num` (zeros stripped, `""`→`"0"`) and `denom`.
2. Suffix: the match table above. Parse the exponent with `str::parse::<i64>()` (same accept set
   as Go `ParseInt(…,10,64)` for these inputs, since the scanner allows only sign+ASCII digits), then `as i32`.
3. int64 path: `precision` as in §2.1; `value.checked_mul(mantissa)`; set the string cache by the same rule.
4. Dec path: `inf.Dec.SetString` → BigInt + fraction-digit count (no digits → `Numeric`);
   `scale.wrapping_sub(exponent)` or `<<= exponent`; magnitude round-up to scale 9
   (multiply by 10^(9-scale), or `div_mod_floor` by 10^(scale-9) and +1 if the remainder ≠ 0); BinarySI cap
   `(MaxInt64, scale 0)`; BinarySI→DecimalSI when 0<|x|<1; restore the sign.
5. `scaled_value(scale)`:
   - Int: `if s < scale { neg_scale(value, scale.wrapping_sub(s)) } else { pos_scale_wrapping(value, s.wrapping_sub(scale)) }`,
     where `pos_scale_wrapping(v,k)` = `v` for k≤0; for k∈{1,2,3,6,9} `if v==i64::MIN {0} else {v.wrapping_mul(10^k)}`;
     else `v.checked_mul(10^k)` stepwise → 0 on overflow (short-circuit v==0, because Go loops k times).
     `neg_scale(v,k)`: `q=v/10^k, r=v%10^k` (k≥19: q=0,r=v); `r!=0` → `q±1` away from zero.
   - Dec: `dif = scale_dec as i64 + scale as i64` (newScale = -scale). `dif==0` → `go_int64(&u)`.
     `dif<0` → `go_int64(&u).wrapping_mul(if -dif<=18 {10^-dif} else {i64::MIN})`.
     `u < i64::MAX && dif < 19` → `let x = go_int64(&u); x/10^dif + (x%10^dif != 0) as i64`.
     Else `u.div_mod_floor(10^dif)` → `go_int64(q)` `wrapping_add(1)` if r≠0.
     `go_int64(x)` = low u64 digit of |x| as i64, `wrapping_neg` if negative.
6. `as_approximate_f64`: `i64 as f64` (Rust and Go both round to nearest, ties even). For BigInt write a
   correctly rounded conversion: if bits ≤ 64 → `u64 as f64`; else take the top 64 bits, OR a sticky
   bit into bit 0 when any lower bit is set, convert, and scale by 2^shift (→ ±Inf on overflow).
   Do not rely on `ToPrimitive::to_f64` without testing it. Then `if exp==0 {base} else {base*go_pow10(exp)}`.
7. Keep the exponent arithmetic in `i32` with explicit wrapping wherever Go uses `Scale`/`inf.Scale`.

DoS note: Go itself effectively hangs on `"1e-2147483648"` with 19 or more mantissa digits, or on
`"<19+ digits>e2147483647"`. The Dec scale wraps to ±2^31 and inf.Dec then computes 10^(2^31).
`"0e2147483647"` makes `Value()` loop 2^31 times. Such inputs never come from the API server.
In CRS they are attacker-controlled. The port should short-circuit zero (same result) and refuse
or clamp a Dec rescale above some bound (e.g. 10^4000). For those inputs it then does not match
Go, which would never finish anyway. Document that as an intentional difference.

## 5. Edge-case checklist (each is a line in the golden file)

| Input | Value | Milli | approx | ksm_float | String | Why |
|---|---|---|---|---|---|---|
| `100m` | 1 | 100 | 0.1 | 0.1 | 100m | ceil |
| `1.5Gi` | 1610612736 | 1610612736000 | 1.610612736e+09 | same | 1536Mi | Dec path (fractional binary) |
| `-1.5` | -2 | -1500 | -1.5 | -1.5 | -1500m | int path: away from zero |
| `-1.50000000000000000000` | **0** | -1500 | -1.5 | -1.5 | -1500m | Dec path: trunc+1 |
| `-1.0000000001` | **0** | **-999** | -1.000000001 | **-0.999** | -1000000001n | rounded to -1.000000001 at parse, then trunc+1 |
| `1e-400` / `0.1n` | 1 | 1 | 1e-09 | 0.001 | 1e-9 / 1n | rounded up to 1n |
| `0e400` | 0 | 0 | **NaN** | 0 | 0 | 0·Pow10(400)=0·Inf |
| `1e309` | 0 | 0 | +Inf | 0 | 1e309 | generic overflow → 0 |
| `1e19` | 0 | 0 | 1e+19 | 0 | 10e18 | scale 19: generic loop → 0 |
| `100000000000000000k` | 7766279631452241920 | 200376420520689664 | 1e+20 | 7.766279631452242e+18 | 100E | scale 3/6: wrapped product |
| `100000000000000000000` | 7766279631452241920 | 200376420520689664 | 1e+20 | 7.766279631452242e+18 | 100E | Dec path, Int64() wrap |
| `9223372036854775807` | MaxInt64 | -1000 | 9.223372036854776e+18 | 9.223372036854776e+18 | 9223372036854775807 | Dec (19 digits) |
| `9223372036854775.808` | 9223372036854776 | MinInt64 | | 9.223372036854776e+15 | 9223372036854775808m | just above MaxMilliValue |
| `-9223372036854776` | | 9223372036854775616 | | **+9.223372036854776e+15** | | negative bound not checked |
| `16Ei` / `-16Ei` | ±MaxInt64 | ∓1000 | ±9.223372036854776e+18 | 9.22e18 / **1** | ±9223372036854775807 | BinarySI cap |
| `1e2147483647` | 0 | **1** | +Inf | **0.001** | 10e2147483646 | int32 scale wrap |
| `1e4294967296` | 1 | 1000 | 1 | 1 | 1e4294967296 | exponent int32 wrap |
| `+5`, `0001`, `1.500`, `1E3` | | | | | verbatim | string cache |
| `1000E` | 0 | 0 | 1e+21 | 0 | **1** | no suffix for 10^21 |
| `.`, `+`, `-`, `Ki`, `m` | 0 | 0 | 0 | 0 | 0 | no digits = zero |
| `Ei`, `.e-10` | error (Numeric) | | | | | Dec path needs a digit |
| `1E` | 1e18 | **0** (scale 21 overflow) | 1e+18 | 1e+18 | 1E | exa, not exponent |

## 6. Golden vectors

`testdata/golden/quantity.jsonl` is produced by `tools/golden-go/quantity/main.go`
(`cd tools/golden-go/quantity && go run . > ../../../testdata/golden/quantity.jsonl`). It has its own
`go.mod` requiring `k8s.io/apimachinery v0.37.2` and resolves from the module cache.

One line per distinct input (813: 738 valid, 75 invalid), in generation order:

```json
{"in":"1.5Gi","ok":true,"value":1610612736,"milli":1610612736000,"approx":"1.610612736e+09","ksm_float":"1.610612736e+09","string":"1536Mi"}
{"in":"1K","ok":false,"err":"unable to parse quantity's suffix"}
```

- `value`/`milli` are JSON integers (full int64 range; parse as `i64`, not f64).
- `approx`/`ksm_float` are `strconv.FormatFloat(f,'g',-1,64)` strings and may be `NaN`, `+Inf`, `-Inf`.
  Compare them as parsed floats or with the Go-compatible formatter (`go-float-format.md`).
- For `ok:false` only `err` is present.
- Each accessor runs on a fresh parse, so no cached or promoted state leaks between fields.
- Coverage: typical k8s values; every suffix × 21 mantissas (signs, fractions, 18/19/23 digits,
  MaxInt64); zero spellings; tiny values; negative fractions; fractional binary and the caps; values
  around MaxMilliValue and MaxInt64; values above int64; exponent forms and int32 wraps; the
  wrap-vs-zero overflow scales; string-cache quirks; digit-less inputs; 75 invalid strings.
- Recorded on linux/amd64. Only the out-of-range `int64(math.Pow10(n>18))` conversion is
  architecture-dependent, and no golden input reaches it with a non-zero mantissa.
