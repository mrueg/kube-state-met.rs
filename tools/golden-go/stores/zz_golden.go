// Golden recorder for kube-state-met.rs (ADR-007). Copied into a scratch copy
// of kube-state-metrics' internal/store package by run.sh; never into the
// reference checkout.
package store

import (
	"encoding/json"
	"os"
	"reflect"
	"runtime"
	"strings"
	"sync"
)

var goldenMu sync.Mutex

type goldenRecord struct {
	Test        string    `json:"test"`
	Families    string    `json:"families_fn"`
	Type        string    `json:"type"`
	Object      any       `json:"object"`
	AllowAnn    *[]string `json:"allow_annotations"`
	AllowLab    *[]string `json:"allow_labels"`
	Headers     []string  `json:"headers"`
	Output      []string  `json:"output"`
	RoundTripOK bool      `json:"roundtrip_ok"`
}

func goldenTestName() string {
	pc := make([]uintptr, 32)
	n := runtime.Callers(2, pc)
	frames := runtime.CallersFrames(pc[:n])
	for {
		f, more := frames.Next()
		name := f.Function[strings.LastIndex(f.Function, ".")+1:]
		if strings.HasPrefix(name, "Test") {
			return name
		}
		if !more {
			return "unknown"
		}
	}
}

func (testCase *generateMetricsTestCase) recordGolden() {
	out := os.Getenv("GOLDEN_OUT")
	if out == "" || testCase.Obj == nil || !testCase.goldenSet {
		return
	}
	rec := goldenRecord{
		Test:     goldenTestName(),
		Families: testCase.goldenFn,
		Type:     reflect.TypeOf(testCase.Obj).String(),
		Headers:  testCase.goldenHeaders,
	}
	if testCase.goldenAnn != nil {
		a := testCase.goldenAnn
		rec.AllowAnn = &a
	}
	if testCase.goldenLab != nil {
		l := testCase.goldenLab
		rec.AllowLab = &l
	}
	// Round-trip the object through JSON, as the API server delivers it.
	raw, err := json.Marshal(testCase.Obj)
	if err != nil {
		return
	}
	t := reflect.TypeOf(testCase.Obj)
	obj := testCase.Obj
	if t.Kind() == reflect.Pointer {
		fresh := reflect.New(t.Elem()).Interface()
		if err := json.Unmarshal(raw, fresh); err == nil {
			obj = fresh
			rec.RoundTripOK = true
		}
	}
	var generic any
	_ = json.Unmarshal(raw, &generic)
	rec.Object = generic
	for _, f := range testCase.Func(obj) {
		rec.Output = append(rec.Output, string(f.ByteSlice()))
	}
	line, err := json.Marshal(rec)
	if err != nil {
		return
	}
	goldenMu.Lock()
	defer goldenMu.Unlock()
	fh, err := os.OpenFile(out, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer fh.Close()
	_, _ = fh.Write(append(line, '\n'))
}
