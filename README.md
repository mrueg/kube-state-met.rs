# kube-state-met.rs

[![ci](https://github.com/mrueg/kube-state-met.rs/actions/workflows/ci.yml/badge.svg)](https://github.com/mrueg/kube-state-met.rs/actions/workflows/ci.yml)
[![bench](https://github.com/mrueg/kube-state-met.rs/actions/workflows/bench.yml/badge.svg)](https://github.com/mrueg/kube-state-met.rs/actions/workflows/bench.yml)

> [!WARNING]
> This is a prototype. It is not an official Kubernetes project and is not supported for production
> use. Behaviour is checked against upstream kube-state-metrics, but differences remain (see
> [Known differences](#known-differences-from-the-go-original)).

A Rust port of [kube-state-metrics](https://github.com/kubernetes/kube-state-metrics): it watches the
Kubernetes API server and serves Prometheus metrics about the state of the objects. It is a drop-in
replacement: the same flags, config files, routes, metric names, labels and values (see
[docs/port/DECISIONS.md](docs/port/DECISIONS.md), ADR-003 and ADR-004).

Ported from upstream commit `46cd52ff` (all 40 built-in resources, custom resource state metrics,
static and automatic sharding, allow/deny/opt-in lists, label and annotation allow lists, `--tls-config`,
`--auth-filter`, config/kubeconfig hot reload, gzip, OpenMetrics).

## Build and run

```sh
cargo build --release -p kube-state-metrics
target/release/kube-state-metrics --kubeconfig ~/.kube/config --port 8080 --telemetry-port 8081
```

Container image (static musl binary on distroless, amd64 and arm64):

```sh
docker buildx build --platform linux/amd64,linux/arm64 -t kube-state-metrics-rs .
```

The upstream manifests and RBAC in `../kube-state-metrics/examples/standard` work unchanged with this image.

## Workspace

| Crate | What it is |
| --- | --- |
| `crates/ksm-core` | metric text format, family generators, metric stores and writers, allow/deny and opt-in filters (with an ECMAScript-to-fancy-regex translator), sharding (jump hash), label helpers |
| `crates/ksm-stores` | the metric families of all 40 resources, `resource.Quantity`, cron schedules |
| `crates/ksm-crs` | custom resource state metrics (config, metric generation, flect pluralization) |
| `crates/ksm` | the `kube-state-metrics` binary: options and CLI, watchers, builder, HTTP servers, telemetry, TLS, auth filter, CRD discovery, hot reload |
| `tools/fake-apiserver` | a deterministic API server stand-in used for recording, comparisons and benchmarks |

## How equivalence with Go is proven

- **Goldens recorded from the Go code** (`testdata/golden/`, run by `cargo test`): every object of the
  upstream store tests with Go's real output (267 cases, all 40 stores), label helpers (911), float
  formatting (3203), quantities (813), custom resource state (240 families, 84 configs), flect (212), CLI and
  options (529). Regenerate with `tools/golden-go/*/run.sh`.
- **Side by side**: `tools/app-cases/compare.py [flags]` runs the Go and Rust binaries against the fake API
  server and compares every family (same HELP/TYPE and order, same lines in any order).
- **Recorded HTTP cases**: `tools/app-cases/record.sh` records the Go binary with
  [rust-anything](https://crates.io/crates/rust-anything); `tools/app-cases/record.sh replay` replays the Rust
  binary against them (`rust/app-cases/`).

## Benchmarks

`bench/run.sh` builds both and writes [bench/RESULTS.md](bench/RESULTS.md) for 1k/10k pods against the same
fake API server:
- sync time;
- scrape latency p50/p90/p99, plain and gzip;
- throughput with 4 concurrent clients;
- CPU per scrape and memory;
- CPU under a steady load of pod watch events (`--churn`);
- criterion micro-benchmarks (`cargo bench -p ksm-stores`).

At 10k pods, compared with Go:
- sync is 2x faster;
- scrapes are 3.3x faster;
- gzip scrapes are 2x faster;
- plain-text throughput is 2.5x higher;
- memory is 1.3x lower;
- CPU under 1000 pod events per second is about half.

`tools/kind-kwok-bench/run.sh` runs the same comparison against a real HA API server: a kind cluster with three
control-plane nodes, with kwok simulating the nodes and pods. It writes `bench/RESULTS-kind-kwok.md`. Both
benchmarks run in the [bench workflow](.github/workflows/bench.yml).

## Known differences from the Go original

See the ADRs in [docs/port/DECISIONS.md](docs/port/DECISIONS.md). In short: no `go_*` telemetry metrics and no
pprof; `--auto-gomemlimit*` are accepted and ignored; JSON instead of protobuf to the API server; rustls has no
TLS 1.0/1.1, CBC suites or P-521; log lines keep klog's layout but not its exact text; cobra's hidden
`__complete` command (dynamic shell completion) is not implemented; no client-side API rate limit (client-go: QPS 5,
burst 10); the telemetry port answers protobuf requests with text; label values with invalid UTF-8 arrive as U+FFFD
through JSON, where Go's protobuf client keeps the raw bytes.
