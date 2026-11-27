#!/usr/bin/env bash
# Benchmarks kube-state-metrics (Go) against kube-state-met.rs (Rust) on a
# real, highly available API server: a kind cluster with three control-plane
# nodes behind kind's load balancer, and kwok simulating the nodes, so
# thousands of pods exist without running containers.
#
# Both binaries run on the host against the cluster, one after the other,
# and are measured by bench/bench.py (sync time, scrape latency, gzip,
# throughput, CPU, memory). The cluster is created for the run and deleted
# afterwards (KEEP=1 keeps it); no other cluster is touched.
#
# Usage: tools/kind-kwok-bench/run.sh [--pods 5000] [bench.py options...]
# Env:   CLUSTER (default ksm-bench), KWOK_VERSION (default v0.7.0), KEEP=1
# Needs: kind, kubectl, docker, go, cargo, python3, curl.
set -euo pipefail
cd "$(dirname "$0")/../.."

CLUSTER="${CLUSTER:-ksm-bench}"
KWOK_VERSION="${KWOK_VERSION:-v0.7.0}"
PODS=5000
args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --pods) PODS="$2"; shift 2 ;;
    *) args+=("$1"); shift ;;
  esac
done
NODES=$(( (PODS + 99) / 100 ))
DEPLOYMENTS=$(( (PODS + 9) / 10 ))
SERVICES=$(( PODS / 20 ))
CONFIGMAPS=$(( PODS / 10 ))
SECRETS=$(( PODS / 10 ))
NAMESPACES=10

work="$(mktemp -d)"
KUBECONFIG="$work/kubeconfig"
export KUBECONFIG

if kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then
  echo "cluster $CLUSTER already exists; delete it or set CLUSTER" >&2
  exit 1
fi
cleanup() {
  if [[ "${KEEP:-}" != 1 ]]; then
    kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
  else
    echo "kept cluster $CLUSTER (kubeconfig: $KUBECONFIG)"
  fi
}
trap cleanup EXIT

echo "== kind cluster $CLUSTER: 3 control-plane nodes, 1 worker"
cat > "$work/kind.yaml" <<'EOF'
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
  - role: control-plane
  - role: control-plane
  - role: worker
EOF
kind create cluster --name "$CLUSTER" --config "$work/kind.yaml" --kubeconfig "$KUBECONFIG" --wait 5m

echo "== kwok $KWOK_VERSION"
kubectl apply -f "https://github.com/kubernetes-sigs/kwok/releases/download/${KWOK_VERSION}/kwok.yaml"
kubectl apply -f "https://github.com/kubernetes-sigs/kwok/releases/download/${KWOK_VERSION}/stage-fast.yaml"
kubectl -n kube-system rollout status deployment/kwok-controller --timeout=5m

echo "== $NODES fake nodes, $PODS pods in $DEPLOYMENTS deployments, $SERVICES services, $CONFIGMAPS configmaps, $SECRETS secrets"
python3 - "$NODES" "$DEPLOYMENTS" "$SERVICES" "$CONFIGMAPS" "$SECRETS" "$NAMESPACES" "$PODS" > "$work/objects.yaml" <<'EOF'
import sys
nodes, deps, svcs, cms, secrets, nss, pods = map(int, sys.argv[1:])
out = []
for i in range(nodes):
    out.append(f"""apiVersion: v1
kind: Node
metadata:
  name: kwok-node-{i}
  annotations: {{node.alpha.kubernetes.io/ttl: "0", kwok.x-k8s.io/node: fake}}
  labels: {{kubernetes.io/hostname: kwok-node-{i}, kubernetes.io/os: linux, kubernetes.io/arch: amd64, type: kwok}}
spec:
  taints: [{{effect: NoSchedule, key: kwok.x-k8s.io/node, value: fake}}]
status:
  allocatable: {{cpu: "64", memory: 256Gi, pods: "110"}}
  capacity: {{cpu: "64", memory: 256Gi, pods: "110"}}""")
for n in range(nss):
    out.append(f"apiVersion: v1\nkind: Namespace\nmetadata:\n  name: bench-{n}")
left = pods
for i in range(deps):
    replicas = min(10, left)
    left -= replicas
    out.append(f"""apiVersion: apps/v1
kind: Deployment
metadata: {{name: app-{i}, namespace: bench-{i % nss}, labels: {{app: app-{i}}}}}
spec:
  replicas: {replicas}
  selector: {{matchLabels: {{app: app-{i}}}}}
  template:
    metadata: {{labels: {{app: app-{i}, bench: kwok}}}}
    spec:
      nodeSelector: {{type: kwok}}
      tolerations: [{{key: kwok.x-k8s.io/node, operator: Exists, effect: NoSchedule}}]
      containers:
        - name: app
          image: registry.k8s.io/pause:3.10
          resources: {{requests: {{cpu: 10m, memory: 16Mi}}, limits: {{memory: 32Mi}}}}""")
for i in range(svcs):
    out.append(f"""apiVersion: v1
kind: Service
metadata: {{name: svc-{i}, namespace: bench-{i % nss}}}
spec:
  selector: {{app: app-{i}}}
  ports: [{{port: 80}}]""")
for i in range(cms):
    out.append(f"apiVersion: v1\nkind: ConfigMap\nmetadata: {{name: cm-{i}, namespace: bench-{i % nss}}}\ndata: {{key: value}}")
for i in range(secrets):
    out.append(f"apiVersion: v1\nkind: Secret\nmetadata: {{name: secret-{i}, namespace: bench-{i % nss}}}\nstringData: {{key: value}}")
print("\n---\n".join(out))
EOF
kubectl apply --server-side -f "$work/objects.yaml" >/dev/null

echo "== waiting for $PODS pods to run"
deadline=$(( SECONDS + 1200 ))
while :; do
  running=$(kubectl get pods -A -l bench=kwok --field-selector=status.phase=Running --no-headers 2>/dev/null | wc -l)
  [[ $running -ge $PODS ]] && break
  if [[ $SECONDS -gt $deadline ]]; then
    echo "only $running of $PODS pods running after 20 minutes" >&2
    exit 1
  fi
  sleep 5
done
total=$(kubectl get pods -A --no-headers | wc -l)
echo "== $total pods in the cluster; benchmarking"

source="a kind cluster with 3 control-plane nodes (HA, behind kind's load balancer) and kwok: $NODES fake nodes, $total pods ($PODS on kwok nodes in $DEPLOYMENTS deployments), $SERVICES services, $CONFIGMAPS configmaps, $SECRETS secrets"
bench/run.sh --no-micro --kubeconfig "$KUBECONFIG" --pods "$total" --source "$source" \
  --out bench/RESULTS-kind-kwok.md "${args[@]}"
