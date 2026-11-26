#!/usr/bin/env bash
# Records the Go command line / options behaviour for crates/ksm's unit tests:
# crates/ksm/tests/cli_cases.json (runs the Go binary) and
# crates/ksm/tests/options_go_cases.json (runs a harness built against
# pkg/options). The help and completion texts in crates/ksm/src/options/ are
# captured from the same binary.
# Usage: tools/golden-go/options/run.sh [go-binary] [path-to-kube-state-metrics]
# Builds offline from the module cache; the reference checkout is not modified.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
bin="$(realpath "${1:-$root/target/ksm-go/kube-state-metrics}")"
src="$(cd "${2:-$root/../kube-state-metrics}" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/ksm-options.XXXXXX")"
trap 'rm -rf "$work"' EXIT

sed 's#^module k8s.io/kube-state-metrics/v2#module harness#' "$src/go.mod" > "$work/go.mod"
printf '\nrequire k8s.io/kube-state-metrics/v2 v2.0.0\n\nreplace k8s.io/kube-state-metrics/v2 => %s\n' "$src" >> "$work/go.mod"
cp "$src/go.sum" "$work/go.sum"
cp "$here/main.go" "$work/main.go"
(cd "$work" && GOFLAGS=-mod=mod GOPROXY=off go build -o harness .)

texts="$root/crates/ksm/src/options"
(cd / && "$bin" -h > "$texts/help.txt")
(cd / && "$bin" version -h > "$texts/help_version.txt")
(cd / && "$bin" help -h > "$texts/help_help.txt")
for shell in bash zsh fish; do
  (cd / && "$bin" completion "$shell" > "$texts/completion.$shell")
done

python3 -I "$here/gen_cases.py" "$bin" "$work/harness" "$root/crates/ksm/tests"
