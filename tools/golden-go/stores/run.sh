#!/usr/bin/env bash
# Records store goldens from kube-state-metrics' own store tests (ADR-007).
# Usage: tools/golden-go/stores/run.sh [path-to-kube-state-metrics] [out.jsonl]
# Works on a scratch copy; the reference checkout is never modified.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
src="$(cd "${1:-$here/../../../../kube-state-metrics}" && pwd)"
out="$(realpath -m "${2:-$here/../../../testdata/golden/stores.jsonl}")"
work="$(mktemp -d "${TMPDIR:-/tmp}/ksm-golden.XXXXXX")"
trap 'rm -rf "$work"' EXIT

rsync -a --exclude .git "$src/" "$work/"
cd "$work/internal/store"

cp "$here/zz_golden.go" "$here/zz_units_test.go" .

# Extra fields on the test case, and the recording hook at the start of run().
perl -0pi -e 's/(\tHeaders              \[\]string\n)/$1\tgoldenFn             string\n\tgoldenAnn            []string\n\tgoldenLab            []string\n\tgoldenHeaders        []string\n\tgoldenSet            bool\n/' testutils.go
perl -0pi -e 's/(func \(testCase \*generateMetricsTestCase\) run\(\) error \{\n)/$1\ttestCase.recordGolden()\n/' testutils.go

# Remember which family function and allow lists each case was run with.
for f in *_test.go; do
  perl -pi -e '
    s/c\.Func = generator\.ComposeMetricGenFuncs\((\w+)\(([^,()]*), ([^()]*)\)\)\s*$/c.Func = generator.ComposeMetricGenFuncs($1($2, $3)); c.goldenFn, c.goldenAnn, c.goldenLab, c.goldenSet = "$1", $2, $3, true; c.goldenHeaders = generator.ExtractMetricFamilyHeaders($1($2, $3))\n/;
    s/c\.Func = generator\.ComposeMetricGenFuncs\((\w+)\)\s*$/c.Func = generator.ComposeMetricGenFuncs($1); c.goldenFn, c.goldenSet = "$1", true; c.goldenHeaders = generator.ExtractMetricFamilyHeaders($1)\n/;
  ' "$f"
done

mkdir -p "$(dirname "$out")"; rm -f "$out.tmp"
TZ=UTC GOLDEN_UNITS_OUT="$(dirname "$out")/labels.jsonl" GOLDEN_OUT="$out.tmp" go test -count=1 -run 'Test' . >/dev/null || {
  echo "go test failed in the scratch copy" >&2; exit 1; }
sort -u "$out.tmp" > "$out"
rm -f "$out.tmp"
echo "wrote $(wc -l < "$out") cases to $out"
