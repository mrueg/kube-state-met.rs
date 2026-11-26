// Golden recorder for kube-state-met.rs (ADR-007). Copied into a scratch copy
// of kube-state-metrics' pkg/customresourcestate package by run.sh; never into
// the reference checkout.
package customresourcestate

import (
	"encoding/json"
	"os"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/gobuffalo/flect"
	"k8s.io/apimachinery/pkg/apis/meta/v1/unstructured"
	utiljson "k8s.io/apimachinery/pkg/util/json"
	"sigs.k8s.io/yaml"
	yamlv3 "sigs.k8s.io/yaml/goyaml.v3"

	generator "k8s.io/kube-state-metrics/v2/pkg/metric_generator"
)

type goldenCases struct {
	Objects map[string]string `yaml:"objects"`
	Cases   []struct {
		Name    string   `yaml:"name"`
		Config  string   `yaml:"config"`
		Objects []string `yaml:"objects"`
	} `yaml:"cases"`
	ToFloat []string `yaml:"tofloat"`
	Flect   []string `yaml:"flect"`
}

type goldenGVK struct {
	Group   string `json:"group"`
	Version string `json:"version"`
	Kind    string `json:"kind"`
}

type goldenResource struct {
	GVK          goldenGVK  `json:"gvk"`
	ResourceName string     `json:"resource_name"`
	Prefix       string     `json:"prefix"`
	Error        string     `json:"error,omitempty"`
	Families     []string   `json:"families"`
	Headers      []string   `json:"headers"`
	Outputs      [][]string `json:"outputs"`
}

type goldenMetrics struct {
	Kind      string            `json:"kind"`
	Name      string            `json:"name"`
	Config    string            `json:"config"`
	Objects   []json.RawMessage `json:"objects"`
	Error     string            `json:"error,omitempty"`
	Resources []goldenResource  `json:"resources"`
}

type goldenToFloat struct {
	Kind      string          `json:"kind"`
	Input     json.RawMessage `json:"input"`
	NilIsZero bool            `json:"nil_is_zero"`
	Value     string          `json:"value,omitempty"`
	Error     string          `json:"error,omitempty"`
}

type goldenFlect struct {
	In  string `json:"in"`
	Out string `json:"out"`
}

func writeJSONL(t *testing.T, path string, recs []any) {
	fh, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer fh.Close()
	for _, r := range recs {
		line, err := json.Marshal(r)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := fh.Write(append(line, '\n')); err != nil {
			t.Fatal(err)
		}
	}
}

var fence = regexp.MustCompile("(?s)```yaml\n(.*?)```")

func TestZZGolden(t *testing.T) {
	out := os.Getenv("GOLDEN_OUT")
	if out == "" {
		t.Skip("GOLDEN_OUT not set")
	}
	raw, err := os.ReadFile(os.Getenv("GOLDEN_CASES"))
	if err != nil {
		t.Fatal(err)
	}
	var gc goldenCases
	if err := yamlv3.Unmarshal(raw, &gc); err != nil {
		t.Fatal(err)
	}

	crJSON, err := json.Marshal(cr)
	if err != nil {
		t.Fatal(err)
	}
	gc.Objects["cr"] = string(crJSON)

	// The docs: the example object and every CustomResourceStateMetrics config.
	docs, err := os.ReadFile(os.Getenv("GOLDEN_DOCS"))
	if err != nil {
		t.Fatal(err)
	}
	blocks := fence.FindAllStringSubmatch(string(docs), -1)
	fooJSON, err := yaml.YAMLToJSON([]byte(blocks[0][1]))
	if err != nil {
		t.Fatal(err)
	}
	gc.Objects["docs-foo"] = string(fooJSON)
	n := 0
	for _, b := range blocks {
		if !strings.Contains(b[1], "CustomResourceStateMetrics") {
			continue
		}
		n++
		gc.Cases = append(gc.Cases, struct {
			Name    string   `yaml:"name"`
			Config  string   `yaml:"config"`
			Objects []string `yaml:"objects"`
		}{Name: "docs/" + strconv.Itoa(n), Config: b[1], Objects: []string{"docs-foo", "vpa", "vpa-empty"}})
	}

	var recs []any
	for _, c := range gc.Cases {
		rec := goldenMetrics{Kind: "metrics", Name: c.Name, Config: c.Config, Resources: []goldenResource{}}
		var objs []*unstructured.Unstructured
		for _, name := range c.Objects {
			src, ok := gc.Objects[name]
			if !ok {
				t.Fatalf("%s: no object %q", c.Name, name)
			}
			// Decode as client-go decodes unstructured JSON (int64 / float64).
			var m map[string]any
			if err := utiljson.Unmarshal([]byte(src), &m); err != nil {
				t.Fatalf("%s: object %s: %v", c.Name, name, err)
			}
			rec.Objects = append(rec.Objects, json.RawMessage(strings.TrimSpace(src)))
			objs = append(objs, &unstructured.Unstructured{Object: m})
		}

		// As FromConfig / resolveCustomResourceConfig: one Decode of a yaml.v3 decoder.
		var m Metrics
		if err := yamlv3.NewDecoder(strings.NewReader(c.Config)).Decode(&m); err != nil {
			rec.Error = "failed to parse Custom Resource State metrics: " + err.Error()
			recs = append(recs, rec)
			continue
		}
		configOverrides(&m)
		for _, r := range m.Spec.Resources {
			gr := goldenResource{
				GVK:          goldenGVK(r.GroupVersionKind),
				ResourceName: r.GetResourceName(),
				Prefix:       r.GetMetricNamePrefix(),
				Families:     []string{},
				Headers:      []string{},
				Outputs:      [][]string{},
			}
			f, err := NewCustomResourceMetrics(r)
			if err != nil {
				gr.Error = err.Error()
				rec.Resources = append(rec.Resources, gr)
				continue
			}
			gens := f.MetricFamilyGenerators()
			for _, g := range gens {
				gr.Families = append(gr.Families, g.Name)
			}
			gr.Headers = append(gr.Headers, generator.ExtractMetricFamilyHeaders(gens)...)
			compose := generator.ComposeMetricGenFuncs(gens)
			for _, o := range objs {
				outputs := []string{}
				for _, fam := range compose(o) {
					outputs = append(outputs, string(fam.ByteSlice()))
				}
				gr.Outputs = append(gr.Outputs, outputs)
			}
			rec.Resources = append(rec.Resources, gr)
		}
		recs = append(recs, rec)
	}

	for _, in := range gc.ToFloat {
		var v any
		if err := utiljson.Unmarshal([]byte(in), &v); err != nil {
			t.Fatalf("tofloat %s: %v", in, err)
		}
		for _, nz := range []bool{false, true} {
			rec := goldenToFloat{Kind: "tofloat", Input: json.RawMessage(in), NilIsZero: nz}
			f, err := toFloat64(v, nz)
			if err != nil {
				rec.Error = err.Error()
			} else {
				rec.Value = strconv.FormatFloat(f, 'g', -1, 64)
			}
			recs = append(recs, rec)
		}
	}
	writeJSONL(t, out, recs)

	var frecs []any
	for _, in := range gc.Flect {
		frecs = append(frecs, goldenFlect{In: in, Out: flect.Pluralize(in)})
	}
	writeJSONL(t, os.Getenv("GOLDEN_FLECT_OUT"), frecs)
}
