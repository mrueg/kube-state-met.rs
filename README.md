# kube-state-met.rs

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

`bench/run.sh` builds both and writes [bench/RESULTS.md](bench/RESULTS.md): sync time, scrape latency,
memory and CPU at 1k/10k/50k pods against the same fake API server, plus criterion micro-benchmarks
(`cargo bench -p ksm-stores`).

## Known differences from the Go original

See the ADRs in [docs/port/DECISIONS.md](docs/port/DECISIONS.md). In short: no `go_*` telemetry metrics and no
pprof; `--auto-gomemlimit*` are accepted and ignored; JSON instead of protobuf to the API server; rustls has no
TLS 1.0/1.1, CBC suites or P-521; log lines keep klog's layout but not its exact text; cobra's hidden
`__complete` command (dynamic shell completion) is not implemented; the `--server-*-timeout` flags are accepted
but not yet applied to the HTTP servers.
