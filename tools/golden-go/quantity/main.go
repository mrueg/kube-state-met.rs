// Command quantity records golden vectors for k8s.io/apimachinery resource.Quantity
// (v0.37.2) as used by kube-state-metrics.
//
// Usage (from this directory):
//
//	go run . > ../../../testdata/golden/quantity.jsonl
//
// One JSON object per line:
//
//	{"in": "...", "ok": true, "value": <q.Value()>, "milli": <q.MilliValue()>,
//	 "approx": "<FormatFloat(q.AsApproximateFloat64(),'g',-1,64)>",
//	 "ksm_float": "<FormatFloat(convertValueToFloat64(&q),'g',-1,64)>",
//	 "string": "<q.String()>"}
//
// For inputs that ParseQuantity rejects: {"in": "...", "ok": false, "err": "<error text>"}
// (the numeric fields are omitted).
//
// Every accessor runs on its own freshly parsed Quantity so that no accessor can
// observe state cached or promoted by another (AsDec/String mutate the receiver).
//
// The results of Value()/MilliValue() for values far outside int64 depend on Go's
// wrapping int64 arithmetic and are recorded as produced on linux/amd64.
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strconv"

	"k8s.io/apimachinery/pkg/api/resource"
)

// convertValueToFloat64 is a verbatim copy of
// kube-state-metrics internal/store/utils.go:418-423.
func convertValueToFloat64(q *resource.Quantity) float64 {
	if q.Value() > resource.MaxMilliValue {
		return float64(q.Value())
	}
	return float64(q.MilliValue()) / 1000
}

type record struct {
	In       string  `json:"in"`
	OK       bool    `json:"ok"`
	Err      string  `json:"err,omitempty"`
	Value    *int64  `json:"value,omitempty"`
	Milli    *int64  `json:"milli,omitempty"`
	Approx   *string `json:"approx,omitempty"`
	KSMFloat *string `json:"ksm_float,omitempty"`
	String   *string `json:"string,omitempty"`
}

func g(f float64) *string {
	s := strconv.FormatFloat(f, 'g', -1, 64)
	return &s
}

func parse(s string) resource.Quantity {
	q, err := resource.ParseQuantity(s)
	if err != nil {
		panic(err)
	}
	return q
}

func inputs() []string {
	var in []string
	add := func(xs ...string) { in = append(in, xs...) }

	// Typical Kubernetes values (node allocatable, container requests/limits, PVC sizes, HPA).
	add("100m", "1.5Gi", "512Mi", "2", "0.5", "1e3", "1E3", "250m", "500m", "1", "4", "8Gi",
		"16Gi", "128974848", "129e6", "129M", "123Mi", "1Ki", "1k", "500Mi", "1Gi", "10Gi",
		"110", "3.5", "0.1", "0.25", "3920m", "7820Mi", "32879380Ki", "95551679124",
		"104857600", "64Mi", "200Mi", "1G", "40Gi", "10G", "8000m", "1500m", "2000m",
		"0.001", "1m", "2.5", "1536Mi", "1.5Ti", "100Gi", "5Ti", "20Mi", "256Mi", "1Mi",
		"1000", "1024", "10", "0.01", "0.05", "0.75", "33554432", "16Ki", "4Gi", "32Gi")

	// Zero spellings.
	add("0", "-0", "+0", "00", "0.0", "0.000", "-0.0", "0m", "0Ki", "0Ei", "0e3", "0E3",
		"0e-3", "0e400", "0e-400", "0e100000", "0.00000000000000000000", "0n", "0.0e5",
		"000000000000000000000000", "0.0000000000000000000e30")

	// Cross product of mantissas and every suffix form.
	nums := []string{"1", "-1", "+1", "1.5", "-1.5", "0.5", ".5", "1.", "7", "8", "1023", "1024",
		"1000", "123456789", "0.001", "-0.001", "100000000000000000", "999999999999999999",
		"1000000000000000000", "9223372036854775807", "12345678901234567890123"}
	sufs := []string{"", "n", "u", "m", "k", "M", "G", "T", "P", "E", "Ki", "Mi", "Gi", "Ti",
		"Pi", "Ei", "e0", "e3", "e-3", "E3", "E-9", "e18", "e19"}
	for _, n := range nums {
		for _, s := range sufs {
			add(n + s)
		}
	}

	// Tiny values: everything below 1n rounds up (away from zero) to 1n.
	add("1n", "0.5n", "0.1n", "1.5n", "1e-9", "1e-10", "1e-12", "1e-18", "1e-400",
		"0.000000001", "0.0000000005", "0.0000000001", "-1n", "-0.1n", "-0.5n", "-1e-10",
		"1u", "0.001m", "0.0001m", "1.0000000001", "1.000000001", "0.0000000015",
		"1e-9k", "5e-324", "1.000000000000000000001")

	// Negative fractions: int64 fast path vs inf.Dec path round differently.
	add("-1.5", "-1.0000000001", "-1.50000000000000000000", "-0.5", "-0.0000000001",
		"-1.5Ki", "-1.1Ki", "-100m", "-1m", "-0.1m", "-1.5m", "-1.0005", "-2.5", "-1.9",
		"-1.1", "-1.000000001", "-0.0005", "-1500m", "-1.5k", "-1.25Gi", "-0.5Gi",
		"-1.00000000000000000001", "-999.9999999999", "-1.5e-3", "-15e-1")

	// Fractional BinarySI (inf.Dec path) and binary canonicalization.
	add("1.1Ki", "1.5Ki", "0.5Ki", "0.5Gi", "0.25Ki", "0.001Ki", "1.0Ki", "2.Ki", "1.Gi",
		"1.5Mi", "1.75Gi", "0.1Mi", "3.3Ti", "1.5Pi", "1.5Ei", "0.5Ei", "1023Ki", "1025Ki",
		"1048576Ki", "1024Mi", "2048Ki", "1536Ki", "8Pi", "8191Pi", "8192Pi", "8193Pi",
		"7Ei", "8Ei", "16Ei", "-8Ei", "-16Ei", "-7Ei", "8388607Ti", "8388608Ti", "8388609Ti",
		"9007199254740993Ki", "1e3Ki")

	// Around MaxMilliValue (= 9223372036854775) and MaxInt64.
	add("9223372036854775", "9223372036854776", "9223372036854774", "9223372036854775.807",
		"9223372036854775.808", "9223372036854775.0001", "9223372036854774.999",
		"9223372036854775807m", "9223372036854775808m", "9223372036854776000m",
		"9223372036854775k", "9223372036854776k", "-9223372036854775", "-9223372036854776",
		"-9223372036854777", "9.223372036854775e15", "9.223372036854776e15",
		"9223372036854775806", "9223372036854775808", "-9223372036854775807",
		"-9223372036854775808", "-9223372036854775809", "18446744073709551615",
		"18446744073709551616", "18446744073709551617", "9223372036854775807n",
		"9223372036854775807u", "9223372036854775807k", "9.223372036854775807E", "9.3E",
		"9223372036854775.5", "9223372036854775.4", "922337203685477580", "922337203685477581",
		"92233720368547758", "92233720368547758.1", "9223372036854775807000m")

	// Huge values (> int64), mostly decimal; inf.Dec is not capped for decimal formats.
	add("1e18", "9e18", "9.3e18", "1e19", "1e20", "1e21", "1e100", "1e308", "1e309", "1e400",
		"-1e19", "-1e400", "100000000000000000000", "-100000000000000000000",
		"1000000000000000000000000000000", "123456789012345678901234567890e10",
		"1234567890123456789e10", "1000E", "1000000E", "1e2147483647", "1e4294967296",
		"1e4294967299", "1e-4294967296", "1E21", "99999999999999999999999999999999999999999",
		"340282366920938463463374607431768211456", "1.7976931348623157e308",
		"2e308", "1e-9223372036854775808")

	// positiveScaleInt64 overflow: scales 1,2,3,6,9 wrap, every other scale yields 0.
	add("100000000000000000e1", "100000000000000000e2", "100000000000000000e4",
		"100000000000000000e5", "100000000000000000e6", "100000000000000000e7",
		"100000000000000000e8", "100000000000000000e9", "100000000000000000T",
		"-100000000000000000k", "-922337203685477580", "-9223372036854775k", "9223372036854775808k",
		"922337203685477581e1", "92233720368547759e2", "9223372036854776e3", "922337203685478e4")

	// Exponent forms.
	add("1e+3", "1E+3", "1e-3", "1E-3", "1.5e3", "15e2", "1500e0", "-1e3", "1e0", "1e00",
		"1e-0", "1e-9", "12e-1", "0.5e1", "1e6", "2.5e9", "1.5e-7", "1e01", "1e+0", "+1e3",
		"-1.5e-9", "1.5e+2", "100e-2", "1e-1", "1e-2", "1e1", "1e2", "1e15", "1e16", "1e17",
		"123e-5", "0.123e3", "5e-1", "1.e3", ".5e1", "1e000000000000000000003")

	// Canonical-string cache quirks (String() returns the input verbatim if it "looks canonical").
	add("+5", "0001", "1.500", "1.000", "1.5000", "001.5", "+1Ki", "000100m", "1.250k",
		"10.000", "1000m", "1000000u", "+100m", "0.100", "01e3")

	// Parses that might surprise: sign or suffix without digits.
	add(".", "+", "-", "+.", "-.", "Ki", "-Ki", "m", "e3", "E", "k", "1E", "1.G", "-.5",
		"+.5", "1..", "e",
		// inf.Dec path with no digits at all: ErrNumeric.
		".e-10", "-.e-10", "0.e-10", ".Ei", "Ei", "-Ei", "Pi", "+.Pi", "e-10", ".Ki", "Ti")

	// Invalid strings.
	add("", " ", " 1", "1 ", "\t1", "1\n", "abc", "1Kb", "1KiB", "1K", "1mi", "1e", "1ee3",
		"1e3.5", "1.2.3", "--1", "+-1", "-+1", "1-", "1+", "1e+", "1e-", "0x10", "1_000",
		"∞", "Inf", "+Inf", "NaN", "1Ki5", "1i", "1Mi2", "1e1e1", "1G1", "1m1", "1.5.Gi",
		"1Gi ", "1,5", "1/2", "1e9223372036854775808", "1Ei1", "1µ", "1 Mi", "1MB",
		"1Gb", "1g", "1t", "1p", "1KI", "1mm", "1kk", "1EE", "1e3e", "1e-+3", "1ki", "١",
		"1e3m", "1Kie3", "true", "50%", "1.5%", "2024-01-01T00:00:00Z", "1n1", "1u2")
	return in
}

func main() {
	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	seen := map[string]bool{}
	for _, s := range inputs() {
		if seen[s] {
			continue
		}
		seen[s] = true
		r := record{In: s}
		if _, err := resource.ParseQuantity(s); err != nil {
			r.Err = err.Error()
		} else {
			r.OK = true
			q1, q2, q3, q4, q5 := parse(s), parse(s), parse(s), parse(s), parse(s)
			v := q1.Value()
			m := q2.MilliValue()
			str := q5.String()
			r.Value, r.Milli = &v, &m
			r.Approx = g(q3.AsApproximateFloat64())
			r.KSMFloat = g(convertValueToFloat64(&q4))
			r.String = &str
		}
		if err := enc.Encode(r); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}
}
