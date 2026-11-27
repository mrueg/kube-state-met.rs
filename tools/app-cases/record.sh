#!/usr/bin/env bash
# Records the whole-app HTTP cases (ADR-007 layer 2): the Go kube-state-metrics
# against tools/fake-apiserver serving testdata/fixtures/{basic,golden} (not
# crs/: the recorded resourceVersions depend on the objects loaded), all
# resources enabled.
# Usage: tools/app-cases/record.sh            (record with the Go binary)
#        tools/app-cases/record.sh replay     (replay the Rust binary)
set -euo pipefail
cd "$(dirname "$0")/../.."
export PATH="$HOME/.cargo/bin:$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH" TZ=UTC
cargo build -q --release -p fake-apiserver -p kube-state-metrics
kc=target/app-cases-kubeconfig.yaml # relative, as recorded in rust/app-cases
target/release/fake-apiserver --listen 127.0.0.1:16444 --fixtures testdata/fixtures/basic --fixtures testdata/fixtures/golden --kubeconfig-out "$kc" &
fake=$!
trap 'kill $fake 2>/dev/null || true' EXIT
sleep 1
ALL=$(target/release/kube-state-metrics --list-resources 2>/dev/null || true)
ALL=${ALL:-certificatesigningrequests,clusterrolebindings,clusterroles,configmaps,cronjobs,daemonsets,deployments,endpoints,endpointslices,horizontalpodautoscalers,ingressclasses,ingresses,jobs,leases,limitranges,mutatingadmissionpolicies,mutatingadmissionpolicybindings,mutatingwebhookconfigurations,namespaces,networkpolicies,nodes,persistentvolumeclaims,persistentvolumes,poddisruptionbudgets,pods,replicasets,replicationcontrollers,resourcequotas,rolebindings,roles,secrets,serviceaccounts,services,statefulsets,storageclasses,validatingadmissionpolicies,validatingadmissionpolicybindings,validatingwebhookconfigurations,volumeattachments,volumeattributesclasses}
args=(--arg=--kubeconfig="$kc" --arg=--resources="$ALL" --arg=--telemetry-port=0 --arg=--host=127.0.0.1 --arg=--enable-gzip-encoding)
if [[ "${1:-}" == "replay" ]]; then
  rust-anything app replay . --build "$PWD/target/release/kube-state-metrics --port {port}"
else
  rust-anything app record . --http --pace 0 --ready-timeout 30 "${args[@]}" \
    --inputs tools/app-cases/http-inputs.jsonl --no-traps --normalize 'version=Version: \(version=[^)]*\)'  \
    -- target/ksm-go/kube-state-metrics --port '{port}'
fi
