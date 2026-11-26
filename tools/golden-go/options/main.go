package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"maps"
	"math"
	"os"
	"strconv"
	"time"

	"github.com/spf13/cobra"
	"k8s.io/kube-state-metrics/v2/pkg/options"
	yaml "sigs.k8s.io/yaml/goyaml.v3"
)

type req struct {
	Op   string   `json:"op"`
	Args []string `json:"args"`
	YAML string   `json:"yaml"`
	S    string   `json:"s"`
	Bits int      `json:"bits"`
	Sels []string `json:"sels"`
	NS   []string `json:"ns"`
	Node string   `json:"node"`
	F    float64  `json:"f"`
}

func newOpts(args []string) (*options.Options, error) {
	opts := options.NewOptions()
	cmd := &cobra.Command{Use: "kube-state-metrics"}
	opts.AddFlags(cmd)
	if err := cmd.Flags().Parse(args); err != nil {
		return nil, err
	}
	return opts, nil
}

func configure(opts *options.Options, configFile []byte) *options.Options {
	config := options.NewOptions()
	err := yaml.Unmarshal(configFile, &config)
	if err == nil {
		if len(config.Resources) > 0 {
			opts.Resources = options.ResourceSet{}
			for resource := range config.Resources {
				opts.Resources[resource] = struct{}{}
			}
		}
		if len(config.MetricAllowlist) > 0 {
			opts.MetricAllowlist = options.MetricSet{}
			for metric := range config.MetricAllowlist {
				opts.MetricAllowlist[metric] = struct{}{}
			}
		}
		if len(config.MetricDenylist) > 0 {
			opts.MetricDenylist = options.MetricSet{}
			for metric := range config.MetricDenylist {
				opts.MetricDenylist[metric] = struct{}{}
			}
		}
		if len(config.MetricOptInList) > 0 {
			opts.MetricOptInList = options.MetricSet{}
			for metric := range config.MetricOptInList {
				opts.MetricOptInList[metric] = struct{}{}
			}
		}
		if len(config.LabelsAllowList) > 0 {
			opts.LabelsAllowList = options.LabelsAllowList{}
			maps.Copy(opts.LabelsAllowList, config.LabelsAllowList)
		}
		if len(config.AnnotationsAllowList) > 0 {
			opts.AnnotationsAllowList = options.LabelsAllowList{}
			maps.Copy(opts.AnnotationsAllowList, config.AnnotationsAllowList)
		}
	}
	if opts.CustomResourceConfigFile == "" && opts.CustomResourceConfigFileDeprecated != "" {
		opts.CustomResourceConfigFile = opts.CustomResourceConfigFileDeprecated
	}
	return opts
}

func errStr(err error) any {
	if err == nil {
		return nil
	}
	return err.Error()
}

func main() {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<24)
	enc := json.NewEncoder(os.Stdout)
	for sc.Scan() {
		var r req
		if err := json.Unmarshal(sc.Bytes(), &r); err != nil {
			panic(err)
		}
		out := map[string]any{"req": json.RawMessage(append([]byte(nil), sc.Bytes()...))}
		switch r.Op {
		case "yaml", "configure":
			opts, err := newOpts(r.Args)
			if err != nil {
				out["err"] = err.Error()
				break
			}
			if r.Op == "yaml" {
				err = yaml.Unmarshal([]byte(r.YAML), opts)
				out["err"] = errStr(err)
			} else {
				opts = configure(opts, []byte(r.YAML))
			}
			if math.IsNaN(opts.AutoGoMemlimitRatio) || math.IsInf(opts.AutoGoMemlimitRatio, 0) {
				out["ratio"] = fmt.Sprint(opts.AutoGoMemlimitRatio)
				opts.AutoGoMemlimitRatio = 0
			}
			out["opts"] = opts
			out["nil"] = map[string]bool{
				"resources":   opts.Resources == nil,
				"allow":       opts.MetricAllowlist == nil,
				"labels":      opts.LabelsAllowList == nil,
				"annotations": opts.AnnotationsAllowList == nil,
				"namespaces":  opts.Namespaces == nil,
			}
		case "dur":
			d, err := time.ParseDuration(r.S)
			out["ns"] = int64(d)
			out["err"] = errStr(err)
			out["str"] = d.String()
		case "int":
			v, err := strconv.ParseInt(r.S, 0, r.Bits)
			out["v"] = strconv.FormatInt(v, 10)
			out["err"] = errStr(err)
		case "uint":
			v, err := strconv.ParseUint(r.S, 0, 64)
			out["v"] = strconv.FormatUint(v, 10)
			out["err"] = errStr(err)
		case "float":
			v, err := strconv.ParseFloat(r.S, 64)
			out["v"] = strconv.FormatFloat(v, 'g', -1, 64)
			out["err"] = errStr(err)
		case "bool":
			v, err := strconv.ParseBool(r.S)
			out["v"] = v
			out["err"] = errStr(err)
		case "quote":
			out["v"] = strconv.Quote(r.S)
		case "fmtf":
			out["v"] = fmt.Sprintf("%f", r.F)
		case "merge":
			v, err := options.MergeFieldSelectors(r.Sels)
			out["v"] = v
			out["err"] = errStr(err)
		case "selectors":
			var nl options.NamespaceList
			n := options.NodeType(r.Node)
			out["exclude"] = nl.GetExcludeNSFieldSelector(r.NS)
			out["node"] = n.GetNodeFieldSelector()
		case "labels":
			l := options.LabelsAllowList{}
			err := l.Set(r.S)
			out["v"] = l
			out["err"] = errStr(err)
		}
		if err := enc.Encode(out); err != nil {
			panic(err)
		}
	}
}
