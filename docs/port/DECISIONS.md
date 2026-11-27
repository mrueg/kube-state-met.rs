# Architecture decisions: kube-state-met.rs

Port of kube-state-metrics (Go, upstream commit `46cd52ff`, checkout in `../kube-state-metrics`)
to Rust. The Go checkout is reference only and is never modified.

## ADR-001: Project layout
Status: accepted (2026-11-26)
Context: The user wants the Rust project in its own folder, `kube-state-met.rs`, with the Go code kept elsewhere as reference.
Decision: A cargo workspace at the repository root. Port notes (`MAP.md`, `DECISIONS.md`, `GLOSSARY.md`, `HANDOFF.md`, `research/`) live in `docs/port/`. Files the rust-anything tool owns (`rust/app-cases/`, `rust/RULES.md`) live in `rust/`, because the tool writes there.
Consequences: Run `rust-anything app ...` with this repository as `REPO`, giving the Go binary as the command.

## ADR-002: Scope: everything, in waves
Status: accepted (2026-11-26)
Context: kube-state-metrics has ~40 resource stores, custom resource state (CRS) metrics, sharding (static and automatic), hot-reload of config and kubeconfig, TLS web config, an auth filter and gzip.
Decision: Port all of it, in waves (see MAP.md). `--auto-gomemlimit`, `--auto-gomemlimit-ratio` and `--auto-gomemlimit-refresh-interval` are Go-runtime only: they are accepted and validated as before, and otherwise do nothing.
Consequences: The klog flags (`-v`, `--vmodule`, `--log_dir`, ...) are accepted too. `-v` maps to the `tracing` level; the rest are no-ops.

## ADR-003: Equivalence bar
Status: accepted (2026-11-26)
Context: Go iterates `sync.Map` and Go maps in random order, so the original orders lines within a metric family differently from scrape to scrape.
Decision: On the metrics port (`/metrics`), the bar is: the same HELP/TYPE lines, the same family order, and inside each family the same multiset of sample lines, in any order. Label order inside a line must match exactly. Float formatting must match Go's `strconv.FormatFloat(f, 'g', -1, 64)` byte for byte (`1.501569018e+09`). Exit codes and the errors printed for bad flags and config must match. Log lines do not need to match.
Consequences: Replay sorts sample lines within each family before comparing.

## ADR-004: Compatibility surface
Status: accepted (2026-11-26)
Decision: Keep every CLI flag with its exact spelling and default (help text included), the `--config` YAML schema, the CRS config schema, the routes `/metrics`, `/healthz`, `/livez`, `/readyz` (telemetry) and `/`, the `resources=` / `exclude_resources=` query parameters, OpenMetrics content negotiation, gzip, and the `--tls-config` web-config file format (exporter-toolkit).

## ADR-005: Sharding stays hash-compatible
Status: accepted (2026-11-26)
Decision: Shard assignment is `jump(fnv64a(uid), total_shards) == shard`, exactly as in Go. Automatic sharding through the StatefulSet ordinal works as before.
Consequences: Go and Rust replicas can be mixed during a rollout. A test pins Go's hash values.

## ADR-006: Core libraries
Status: accepted (2026-11-26)
Decision: `kube` 5.x (client, config, `kube::runtime::watcher` in place of informers and reflectors), `k8s-openapi` 0.29 with feature `v1_37`, `tokio`, `axum`/`hyper` for HTTP, `rustls` for TLS, `clap` for flags, `fancy-regex` for the ECMAScript allow/deny lists (`regexp2` in Go), `regex` for opt-in lists and label wildcards (Go `regexp` is RE2), and a hand-written text exposition writer, as in Go. Small Go dependencies are ported in-tree: jump hash, FNV-64a, `resource.Quantity`, field selectors, label-name sanitizing.
Consequences: Types k8s-openapi lacks get their own serde structs.

## ADR-007: Ground truth
Status: accepted (2026-11-26)
Context: kube-state-metrics needs an API server, and a real cluster's state changes all the time (controllers, leases, kubelet status), so it can't be recorded deterministically.
Decision: Three layers.
1. **Store goldens**: a patched scratch copy of the Go store tests dumps every test object (JSON round-tripped, as the API server would deliver it), its allow lists, the headers and Go's own output to `testdata/golden/stores/*.jsonl`. Rust tests regenerate the output and compare by ADR-003. The same kind of Go harness records leaf functions (float formatting, label sanitizing, quantities, regex matching, field selectors, jump hash).
2. **App cases**: `tools/fake-apiserver` (Rust) serves fixed fixtures (list, watch, WatchList initial events, `/version`, discovery). `rust-anything app record --http` runs the Go binary against it and stores the HTTP cases in `rust/app-cases/http/`; `app replay` holds every build to them.
3. **Smoke**: the upstream e2e manifests on a kind cluster, comparing scrapes of Go and Rust side by side (not byte goldens).
Consequences: rust-anything's per-unit `bridge`/`record` (cgo) isn't used: the units take Kubernetes objects, not plain values. The Go harness covers that role, and the HTTP replay covers the whole app.

## ADR-008: Benchmarks are a deliverable
Status: accepted (2026-11-26)
Context: The user asked for benchmarks to be available.
Decision: (a) `criterion` benches in the workspace for exposition writing, per-store metric generation (pods, nodes, deployments...) and label sanitizing. (b) `bench/` harness: a fake API server generating N objects (scales 1k / 10k / 50k pods plus a mix of resources), Go and Rust binaries run against the same fixtures, measuring time to first full scrape (sync), scrape latency p50/p99 and scrape throughput, peak and steady RSS, and CPU time. The results go to `bench/RESULTS.md`, regenerated by `bench/run.sh`.

## ADR-009: Targets, checkpoints, cutover
Status: accepted (2026-11-26)
Decision: linux amd64 and arm64; distroless static container image; MSRV = current stable (1.100). Stop for the user's go-ahead after each wave. Cutover runs Rust beside Go (`app shadow` or comparing scrapes); the Go original stays as reference until the user says otherwise.

## ADR-010: Telemetry port content
Status: accepted (2026-11-26)
Context: The Go telemetry registry includes `go_*` runtime metrics (GC, goroutines, memstats) that can't exist in Rust.
Decision: Every `kube_state_metrics_*` and `http_request_duration_seconds` family matches. `process_*` metrics are kept (procfs). `go_*` metrics are dropped. `kube_state_metrics_build_info` keeps its labels (`client_go_version` reports the kube-rs version).
Consequences: Dashboards that read `go_*` from kube-state-metrics lose those series. The telemetry `/metrics` negotiates like `promhttp.HandlerFor` (text 0.0.4 or 1.0.0 with the requested escaping, gzip, `Vary`, `name[]`), but answers a protobuf request with text. `process_network_*_bytes_total` sum `/proc/self/net/dev`, where client_golang reads `IpExt` octets from `/proc/self/net/netstat`.

## ADR-011: Watch semantics on kube-rs
Status: accepted (2026-11-26)
Decision: One `kube::runtime::watcher` per store (per namespace, as in Go). `Init`/`InitApply`/`InitDone` map to `MetricsStore::replace` (applied once on `InitDone`, keeping the same "delete the missing, then upsert" order), `Apply` to `add`, and `Delete` to `delete`. Sharding filters objects before they reach the store. `--use-apiserver-cache` sets `resourceVersion=0` on lists. `--object-limit` sets the list page size and the store's object limit. Requests are JSON, not protobuf.
With `--object-limit`, kube-rs' watcher would follow every page, so `crate::limited` lists one page of at most the limit (dropping the continue token, with `resourceVersion=0` when asked, as Go does) and watches from its resourceVersion. Cluster-scoped built-in resources get no field selector (Go's `create*ListWatch` drop it). Failed lists and watches back off as client-go's reflector does (800ms doubling to 30s with jitter, reset after 2 minutes; `crate::backoff`), since kube-rs' own backoff resets on every event.
Consequences: List and watch counters on the telemetry port count kube-rs list/watch calls; their numbers may differ from client-go's after errors, and without an object limit kube-rs' periodic re-watches are not counted (a telemetry-port difference, not a metrics-port one). There is no client-side rate limit (client-go: QPS 5, burst 10 for non-watch requests), and kube-rs retries 429/503/504 responses.

## ADR-012: No proxying front door
Status: accepted (2026-11-26)
Context: The whole-app recipe puts a front door in front of the original that proxies whatever isn't ported yet. Here, every store was ported and golden-checked (267/267) before the binary first ran, and `/metrics` is one route covering every resource.
Decision: The Rust binary owns every route from the start. Features not ported yet (custom resource state metrics, `--tls-config`, `--auth-filter`, config hot reload) fail clearly at startup instead of falling back to Go. The fallback rate is the share of recorded app cases that use one of those features, and it drops to zero with wave 4.
Consequences: No Go process runs beside the Rust binary, and `app front-door`/`app wire` are not used. `app record`/`app replay` still hold the binary to the recorded cases. Wave 4 (2026-11-27) ported all four features, so nothing fails at startup any more.

## ADR-013: TLS web config on rustls
Status: accepted (2026-11-27)
Context: exporter-toolkit runs on Go crypto/tls; the port uses rustls (ADR-006), which has no TLS 1.0/1.1, no CBC or RSA-key-exchange suites and no P-521.
Decision: The `--tls-config` schema, startup validation and error texts follow exporter-toolkit v0.21.0 (Go `tls.CipherSuites()` names are accepted). Settings rustls cannot honor are accepted and narrowed: `min_version` TLS10/TLS11 means TLS 1.2, `max_version` below TLS12 fails every handshake, CBC suites in `cipher_suites` are dropped (TLS 1.3 suites stay, as in Go), and a `curve_preferences` list with only CurveP521 keeps the default groups. A client certificate that is itself one of the client CAs is accepted, as Go does. YAML type errors other than unknown fields and the custom value errors are mapped to yaml.v2 text on a best-effort basis.
Consequences: Clients that only speak TLS 1.0/1.1, CBC suites or P-521 cannot connect to the Rust binary. All 49 exporter-toolkit test configs give the same startup errors and HTTP answers as Go (scratch harness against both binaries). HTTP/2 connections close after the idle timeout (`IdleTimeout`, else `ReadTimeout`) without an open stream, as in Go; Go's per-stream read and write timeouts are not applied.

## ADR-014: Parallel gzip on /metrics
Status: accepted (2026-11-27)
Context: With `--enable-gzip-encoding`, compressing a 400 MB body on one core took several seconds. flate2's default backend (miniz_oxide) was also 3–8x slower than Go's compress/flate.
Decision: flate2 runs on the `zlib-rs` backend. The body is compressed pigz-style (`crates/ksm/src/pgzip.rs`). Each 1 MiB chunk is deflated on a rayon thread, using the previous 32 KiB as a preset dictionary, and ends with a sync flush. The chunks join into one standard gzip member, with the CRC32 values combined. The level is zlib's 5, not Go's default 6: on a 10k-pod body (83 MB) Go's compress/flate at 6 takes 0.53s of CPU, zlib-rs at 6 takes 0.65s and at 5 takes 0.47s, with output 1.8% larger than Go's. A response compresses at most half the rayon pool's threads at once (the physical cores with SMT), divided by the gzip responses in flight; a single chunk is compressed on the render thread.
Consequences: A lone gzip scrape uses a few cores, and its latency drops about 2x at 10k pods. Parallel compression costs CPU (eight chunks at once took 1.7x the CPU time of one after another on a 4-core SMT laptop), so concurrent scrapes compress serially and gzip CPU per scrape stays below Go's. The compressed size is within 2% of Go's. The bytes differ from Go's gzip output, but they decode to the same body; the tests check the result with a single-member decoder.
