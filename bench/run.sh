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
export PATH="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
cargo build --release -q -p kube-state-metrics -p fake-apiserver
mkdir -p target/ksm-go
(cd "$GO_SRC" && go build -o "$OLDPWD/target/ksm-go/kube-state-metrics" .)
python3 bench/bench.py "${args[@]}"
if [[ $micro == 1 ]]; then
  cargo bench -q -p ksm-stores --bench generate -- --noplot > target/criterion-summary.txt 2>&1 || true
  {
    echo
    echo "## Micro-benchmarks (criterion, \`cargo bench -p ksm-stores --bench generate\`)"
    echo
    echo "Per-object metric generation over the store-golden objects, and writing a store of 10 000 objects."
    echo
    echo '```'
    grep -E "time:|^[a-z]" target/criterion-summary.txt | paste -sd' ' | sed 's/ \([a-z_]*\/\)/\n\1/g; s/ \(labels\/\)/\n\1/g'
    echo '```'
  } >> bench/RESULTS.md
fi
