// Command floatfmt records golden vectors for strconv.FormatFloat(f, 'g', -1, 64),
// the float formatting kube-state-metrics uses for every sample value.
//
// Usage (from this directory):
//
//	go run . > ../../../testdata/golden/float_g.jsonl
//
// One JSON object per line: {"bits": "<16 lowercase hex digits of math.Float64bits(f)>",
// "out": "<strconv.FormatFloat(f, 'g', -1, 64)>"}. Values are deduplicated by bits and
// emitted in generation order. The random part uses a fixed seed, so output is stable for
// a given Go version.
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"math"
	"math/rand/v2"
	"os"
	"strconv"
)

type record struct {
	Bits string `json:"bits"`
	Out  string `json:"out"`
}

func main() {
	var vals []float64
	add := func(fs ...float64) { vals = append(vals, fs...) }
	lit := func(s string) float64 {
		f, err := strconv.ParseFloat(s, 64)
		if err != nil && f == 0 {
			panic(err)
		}
		return f
	}
	neighbors := func(f float64) {
		add(f, math.Nextafter(f, math.Inf(1)), math.Nextafter(f, math.Inf(-1)))
	}

	r := rand.New(rand.NewPCG(20261126, 42))

	// Special values.
	add(0, math.Copysign(0, -1), math.Inf(1), math.Inf(-1), math.NaN(),
		math.Float64frombits(0x7ff8000000000001), math.Float64frombits(0xfff8000000000000))

	// Small integers and their negatives.
	for i := 0; i <= 120; i++ {
		add(float64(i), -float64(i))
	}
	// Powers of two, 2^53 neighborhood, int64 limits.
	for e := -60; e <= 70; e++ {
		add(math.Ldexp(1, e))
	}
	add(1<<53-1, 1<<53, 1<<53+2, -(1 << 53), float64(math.MaxInt64), float64(math.MinInt64),
		float64(math.MaxInt64/1000), float64(math.MaxUint32), 1536, 1610612736, 536870912,
		17179869184, 1099511627776, 4294967296, 1e15, 1e16, 1e17, 123456789012345678)

	// Powers of ten 1e-30 .. 1e30 as literals, as math.Pow10, and their float neighbors.
	for e := -30; e <= 30; e++ {
		neighbors(lit(fmt.Sprintf("1e%d", e)))
		add(math.Pow10(e), -lit(fmt.Sprintf("1e%d", e)))
		add(lit(fmt.Sprintf("1.5e%d", e)), lit(fmt.Sprintf("9.999999e%d", e)),
			lit(fmt.Sprintf("1.23456789e%d", e)))
	}

	// %e / %f switch boundaries (exponent < -4 or >= 6 for shortest).
	for _, s := range []string{"1e-5", "1e-4", "1e5", "1e6", "1e20", "1e21", "0.0001",
		"0.00009999999999999999", "0.000099999", "0.00010000000000000002", "999999",
		"999999.9", "999999.99999999", "999999.5", "1e6", "1000000.5", "123456", "1234567",
		"99999.99", "100000", "100000.5", "12345.678", "0.00012345", "0.000012345",
		"9.9999e-5", "1.0000000000000002", "0.30000000000000004", "1e22", "1e23", "5e-324",
		"2.2250738585072009e-308", "2.2250738585072014e-308", "1.7976931348623157e308",
		"4.9406564584124654e-324", "1e-320", "1e-310", "1e300", "1e-300", "123e300"} {
		neighbors(lit(s))
		add(-lit(s))
	}

	// Typical arithmetic results.
	add(0.1+0.2, 1.0/3, 2.0/3, 0.1*3, 100*0.001, 1.1*1.1, math.Pi, math.E, math.Sqrt2,
		math.Ln2, 1/math.Pi, 0.1, 0.2, 0.3, 0.7, 1-0.9, 3*1.1, 1e-7*3, 4.35, 0.15*100)

	// Extremes and subnormals.
	add(math.MaxFloat64, -math.MaxFloat64, math.SmallestNonzeroFloat64,
		-math.SmallestNonzeroFloat64, 0x1p-1022, 0x1p-1023, 0x1.fffffffffffffp-1023,
		math.Float64frombits(0x000fffffffffffff), math.Float64frombits(1),
		math.Float64frombits(2), math.Float64frombits(3), math.Float64frombits(0x0008000000000000))

	// Shortest-digit ties: values m*2^e (e < 0) whose exact decimal expansion has exactly
	// one digit more than the shortest form, ending in 5 (e.g. 2^-25 = 2.98023223876953125e-08).
	// Go rounds such ties half-to-even; Rust's {:e} does not always agree.
	for e := -60; e <= -1; e++ {
		add(math.Ldexp(1, e), math.Ldexp(3, e), math.Ldexp(7, e), math.Ldexp(13, e),
			math.Ldexp(255, e), math.Ldexp(1<<53-1, e), math.Ldexp(float64(r.Int64N(1<<20)|1), e))
	}

	// Unix timestamps (1.5e9 range), integral and with fractions; also milliseconds.
	for i := 0; i < 150; i++ {
		ts := 1.4e9 + float64(r.IntN(4e8))
		add(ts)
		add(ts + float64(r.IntN(1000))/1000)
		add(ts + r.Float64())
		add(float64(int64(ts)*1000 + int64(r.IntN(1000))))
	}
	// Milli values divided by 1000 as kube-state-metrics' convertValueToFloat64 does.
	for i := 0; i < 150; i++ {
		m := r.Int64N(1 << 50)
		if i%3 == 0 {
			m = r.Int64N(100000)
		}
		add(float64(m)/1000, -float64(m)/1000)
	}
	// Random subnormals.
	for i := 0; i < 40; i++ {
		add(math.Float64frombits(r.Uint64() & 0x000fffffffffffff))
	}
	// Uniformly random bit patterns (finite only).
	for i := 0; i < 500; i++ {
		f := math.Float64frombits(r.Uint64())
		if math.IsNaN(f) || math.IsInf(f, 0) {
			continue
		}
		add(f)
	}
	// Random doubles in [0,1) scaled across the %e/%f boundary region.
	for i := 0; i < 300; i++ {
		add(r.Float64() * math.Pow10(r.IntN(24)-12))
	}
	// Short decimal values (few significant digits) across magnitudes.
	for i := 0; i < 200; i++ {
		add(lit(fmt.Sprintf("%de%d", r.IntN(100000), r.IntN(60)-30)))
	}

	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	seen := map[uint64]bool{}
	for _, f := range vals {
		b := math.Float64bits(f)
		if seen[b] {
			continue
		}
		seen[b] = true
		if err := enc.Encode(record{Bits: fmt.Sprintf("%016x", b), Out: strconv.FormatFloat(f, 'g', -1, 64)}); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}
}
