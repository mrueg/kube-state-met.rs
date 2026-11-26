#!/usr/bin/env bash
# Builds both implementations and the fake API server, runs the end-to-end
# benchmark (bench/bench.py) and the criterion micro-benchmarks.
# Usage: bench/run.sh [--scales 1000,10000] [--scrapes 30] [--no-micro]
set -euo pipefail
cd "$(dirname "$0")/.."
GO_SRC="${KSM_GO_SRC:-../kube-state-metrics}"
micro=1
args=()
for a in "$@"; do
  if [[ "$a" == "--no-micro" ]]; then micro=0; else args+=("$a"); fi
done
cargo build --release -q -p kube-state-metrics -p fake-apiserver
mkdir -p target/ksm-go
(cd "$GO_SRC" && go build -o "$OLDPWD/target/ksm-go/kube-state-metrics" .)
python3 bench/bench.py "${args[@]}"
if [[ $micro == 1 ]]; then
  cargo bench -q -p ksm-stores --bench generate -- --noplot 2>&1 | tee target/criterion-summary.txt | grep -E "time:|^[a-z]" || true
fi
