#!/usr/bin/env bash
# Records custom resource state goldens (ADR-007): runs every case of
# cases.yaml, plus every CustomResourceStateMetrics example of the docs,
# through Go's own decoder, compiler and generators, and records flect plurals.
# Usage: tools/golden-go/crs/run.sh [path-to-kube-state-metrics] [out-dir]
# Works on a scratch copy; the reference checkout is never modified.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
src="$(cd "${1:-$here/../../../../kube-state-metrics}" && pwd)"
outdir="$(realpath -m "${2:-$here/../../../testdata/golden}")"
work="$(mktemp -d "${TMPDIR:-/tmp}/ksm-golden-crs.XXXXXX")"
trap 'rm -rf "$work"' EXIT

rsync -a --exclude .git --exclude .claude "$src/" "$work/"
cp "$here/zz_golden_test.go" "$work/pkg/customresourcestate/"
cd "$work/pkg/customresourcestate"

# flect reads inflections.json / acronyms.json from the working directory or
# $INFLECT_PATH / $ACRONYMS_PATH; make sure none applies.
unset INFLECT_PATH ACRONYMS_PATH

mkdir -p "$outdir"
TZ=UTC GOLDEN_CASES="$here/cases.yaml" \
  GOLDEN_DOCS="$src/docs/metrics/extend/customresourcestate-metrics.md" \
  GOLDEN_OUT="$outdir/crs.jsonl" GOLDEN_FLECT_OUT="$outdir/flect.jsonl" \
  go test -count=1 -run 'TestZZGolden' . || {
    echo "go test failed in the scratch copy" >&2; exit 1; }
echo "wrote $(wc -l < "$outdir/crs.jsonl") records to $outdir/crs.jsonl"
echo "wrote $(wc -l < "$outdir/flect.jsonl") records to $outdir/flect.jsonl"
