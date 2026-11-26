// Golden recorder for the label helpers of internal/store/utils.go
// (kube-state-met.rs, ADR-007). Copied into a scratch copy by run.sh.
package store

import (
	"encoding/json"
	"fmt"
	"math/rand"
	"os"
	"testing"
)

type unitRecord struct {
	Fn     string            `json:"fn"`
	In     map[string]string `json:"in,omitempty"`
	Arg    string            `json:"arg,omitempty"`
	Allow  []string          `json:"allow,omitempty"`
	Keys   []string          `json:"keys"`
	Values []string          `json:"values,omitempty"`
}

func TestGoldenUnits(t *testing.T) {
	out := os.Getenv("GOLDEN_UNITS_OUT")
	if out == "" {
		t.Skip("GOLDEN_UNITS_OUT not set")
	}
	fh, err := os.Create(out)
	if err != nil {
		t.Fatal(err)
	}
	defer fh.Close()
	emit := func(r unitRecord) {
		b, _ := json.Marshal(r)
		fh.Write(append(b, '\n'))
	}
	words := []string{"a", "A", "b", "B", "a-b", "a.b", "a_b", "a/b", "aB", "AB", "aBC", "1A", "_", "-", ".",
		"app", "App", "app.kubernetes.io/name", "app.kubernetes.io/Name", "ü", "über", "日本", "x\ny",
		"a_b_conflict1", "a_b_conflict2", "label_a", "team/a", "team/b", "teamA", "kubectl.kubernetes.io/last-applied-configuration",
		"fooBarBaz", "foo2Bar", "FOO", "foo--bar", "foo__bar", "😀", "a-b_conflict1"}
	for _, w := range words {
		emit(unitRecord{Fn: "SanitizeLabelName", Arg: w, Keys: []string{SanitizeLabelName(w)}})
		emit(unitRecord{Fn: "toSnakeCase", Arg: w, Keys: []string{toSnakeCase(w)}})
		emit(unitRecord{Fn: "labelName", Arg: w, Keys: []string{labelName("label", w)}})
	}
	rng := rand.New(rand.NewSource(42))
	allows := [][]string{
		{"*"}, {"app"}, {"team/*"}, {"*", "kubectl.kubernetes.io/last-applied-configuration"},
		{"app", "*"}, {"*team*"}, {"a*"}, {"*b"}, {"x*y"}, {"app.kubernetes.io/*", "a"}, {"*", "app"},
	}
	for i := 0; i < 400; i++ {
		n := 1 + rng.Intn(6)
		m := map[string]string{}
		for j := 0; j < n; j++ {
			m[words[rng.Intn(len(words))]] = fmt.Sprintf("v%d", j)
		}
		k, v := mapToPrometheusLabels(m, "label")
		emit(unitRecord{Fn: "mapToPrometheusLabels", In: m, Keys: k, Values: v})
		allow := allows[rng.Intn(len(allows))]
		k, v = createPrometheusLabelKeysValues("annotation", m, allow)
		emit(unitRecord{Fn: "createPrometheusLabelKeysValues", In: m, Allow: allow, Keys: k, Values: v})
	}
}
