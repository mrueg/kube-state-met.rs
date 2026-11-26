# Telemetry port (`--telemetry-port`, default 8081): `/metrics`, `/readyz`, exposition format

Path prefixes used below:

- `CG` = `$(go env GOMODCACHE)/github.com/prometheus/client_golang@v1.25.0/prometheus`
- `PC` = `$(go env GOMODCACHE)/github.com/prometheus/common@v0.74.0`
- `PF` = `…/github.com/prometheus/procfs@v0.22.0`
- `KSM` = `/home/mrueg/repos/kube-state-metrics`
- `CR` = `~/.cargo/registry/src/index.crates.io-*/`

A real capture against kind (`--resources configmaps`, after a few scrapes of `:8080/metrics`), with the `go_*` lines stripped, is in `fixtures/telemetry-metrics-sample.txt`.

## 1. Registry contents

There is a single `prometheus.NewRegistry()` (`KSM pkg/app/server.go:106`). Pedantic checks are off, and **there is no `promhttp_metric_handler_*`**, because `HandlerFor` is used without `opts.Registry` (`server.go:554`; `CG promhttp/http.go:226,256`). Families are listed below by source. "Emitted" says when a series exists, which matters for byte-compatibility.

| Family | TYPE | Labels | HELP (verbatim) | Source | Emitted |
|---|---|---|---|---|---|
| `kube_state_metrics_build_info` | gauge | `branch, client_go_version, goarch, goos, goversion, revision, tags, version` (all const) | `A metric with a constant '1' value labeled by version, revision, branch, goversion from which kube_state_metrics was built, and the goos and goarch for the build.` | `server.go:107-109`, `CG collectors/version/version.go:39-67` | always, value 1 |
| `http_request_duration_seconds` | histogram | const `handler="metrics"`, `method` | `A histogram of requests for kube-state-metrics metrics handler.` | `server.go:111-118,670` | after the first request on **:8080 `/metrics`** |
| `kube_state_metrics_config_hash` | gauge | `type, filename` | `Hash of the currently loaded configuration.` | `server.go:119-123,182,227` | only with `--config` / a CR config file |
| `kube_state_metrics_last_config_reload_successful` | gauge | `type, filename` | `Whether the last configuration reload attempt was successful.` | `server.go:124-128,173,179,224` | same |
| `kube_state_metrics_last_config_reload_success_timestamp_seconds` | gauge | `type, filename` | `Timestamp of the last successful configuration reload.` | `server.go:129-133` | same |
| `kube_state_metrics_custom_resource_state_add_events_total` | counter | – | `Number of times that the CRD informer triggered the add event.` | `server.go:136-139`, `KSM internal/discovery/discovery.go:172` | always (0) |
| `kube_state_metrics_custom_resource_state_update_events_total` | counter | – | `Number of times that the CRD informer triggered the update event.` | `:140-143`, `discovery.go:185` | always |
| `kube_state_metrics_custom_resource_state_delete_events_total` | counter | – | `Number of times that the CRD informer triggered the remove event.` | `:144-147`, `discovery.go:196` | always |
| `kube_state_metrics_custom_resource_state_cache` | gauge | – | `Net amount of CRDs affecting the cache currently.` | `:148-151`, `discovery.go:173,197` | always |
| `kube_state_metrics_watch_total` | counter | `result` (`success`/`error`), `resource` | `Number of total resource watch calls in kube-state-metrics` | `KSM pkg/watch/watch.go:44-50,177,181` | per watch call |
| `kube_state_metrics_list_total` | counter | `result, resource` | `Number of total resource list calls in kube-state-metrics` | `watch.go:51-57,134,138` | per **List** call; **absent in the capture** (see trap 3) |
| `kube_state_metrics_list_objects` | gauge | `resource` | `Number of resources listed in kube-state-metrics` | `watch.go:58-64,166` | after a List |
| `kube_state_metrics_list_objects_limit` | gauge | `resource` | `Number of resource list limit in kube-state-metrics` | `watch.go:65-71,128` | List with `--object-limit` > 0 |
| `kube_state_metrics_object_limit_dropped_total` | counter | `resource` | `Number of add/update events for objects not stored because the object limit was reached` | `watch.go:72-78`, `KSM internal/store/builder.go:755-768` | first drop only |
| `kube_state_metrics_shard_ordinal` | gauge | `shard_ordinal` | `Current sharding ordinal/index of this instance` | `KSM pkg/sharding/metrics.go:26,40-45`, `builder.go:181-188` | always; value = shard, label = `strconv.Itoa(shard)`; `Reset()` on reshard |
| `kube_state_metrics_total_shards` | gauge | – | `Number of total shards this instance is aware of` | `sharding/metrics.go:46-51` | always |
| `process_*` (9 families) | see §2 | – | see §2 | `server.go:314-317` | always on Linux |
| `go_*` (29 families) | | | Go runtime | `CG collectors/go_collector_latest.go:161` | always |

`resource` is Go's `reflect.TypeOf(expectedType).String()` (`builder.go:762,783`), for example `*v1.ConfigMap`, `*v1.Pod`, `*v1.Deployment`, and `*unstructured.Unstructured` for custom resources.

**build_info values.**

- `version`, `branch` and `revision` come from the `-ldflags -X` values (`KSM Makefile:72`).
- `revision` falls back to `vcs.revision` and then to `"unknown"`. `tags` is the `-tags` build setting, else `"unknown"` (`PC version/info.go:100-150`).
- `goversion` is `runtime.Version()`, for example `go1.27.4`. The Rust port has to decide what to put here, for example `rustc 1.x`; record that in DECISIONS.md.
- `client_go_version` comes from `-X …/pkg/app.ClientGoVersion`, default `"unknown"` (`server.go:73`). It is set to `go list -m k8s.io/client-go` (`Makefile:22`).
- Observed: `kube_state_metrics_build_info{branch="main",client_go_version="v0.37.2",goarch="amd64",goos="linux",goversion="go1.27.4",revision="abc123",tags="unknown",version="v2.99.0"} 1`.

**Histogram.**

- Buckets are `DefBuckets = {.005,.01,.025,.05,.1,.25,.5,1,2.5,5,10}` (`CG histogram.go:271`), rendered as `le="0.005"…le="10"`, then `le="+Inf"`.
- There is **no `code` label**: `InstrumentHandlerDuration` only adds `code` if the vec has it (`CG promhttp/instrument_server.go:96-125`).
- `method` is lowercased for the 10 standard methods. Anything else becomes `"unknown"` (`:448-480`).
- The histogram sits **inside** the auth filter (`server.go:670,685`), so rejected requests are not observed.
- It is observed for every path routed to the handler, which is `/metrics` only, and for **all methods** (observed `get`, `head`, `post` series).

## 2. Process collector (`CG process_collector.go:62-115`, `CG process_collector_procfsenabled.go:27-79`)

Errors are silently skipped, because `ReportErrors=false` (`process_collector.go:155-158`).

| Family | TYPE | HELP | Source |
|---|---|---|---|
| `process_cpu_seconds_total` | counter | `Total user and system CPU time spent in seconds.` | `/proc/self/stat` (utime+stime)/100 (`PF proc_stat.go:39,233-235`) |
| `process_open_fds` | gauge | `Number of open file descriptors.` | count of `/proc/self/fd`, with a fast path that uses `stat().size` (`PF proc.go:245-260`) |
| `process_max_fds` | gauge | `Maximum number of open file descriptors.` | `/proc/self/limits` "Max open files" soft (`PF proc_limits.go:125`) |
| `process_virtual_memory_bytes` | gauge | `Virtual memory size in bytes.` | stat vsize (`proc_stat.go:214`) |
| `process_virtual_memory_max_bytes` | gauge | `Maximum amount of virtual memory available in bytes.` | limits "Max address space"; `unlimited` = 2^64-1, which prints as `1.8446744073709552e+19` (`proc_limits.go:129,153`) |
| `process_resident_memory_bytes` | gauge | `Resident memory size in bytes.` | stat rss × `os.Getpagesize()` (`proc_stat.go:219-220`) |
| `process_start_time_seconds` | gauge | `Start time of the process since unix epoch in seconds.` | `/proc/stat` btime + starttime/100 (`proc_stat.go:224-229`) |
| `process_network_receive_bytes_total` | counter | `Number of bytes received by the process over the network.` | `/proc/self/net/netstat` `IpExt InOctets` (`PF proc_netstat.go:187,448`) |
| `process_network_transmit_bytes_total` | counter | `Number of bytes sent by the process over the network.` | same, `OutOctets` |

The two network counters are **per network namespace**, not per process. On the test host they read about 12.5 GB.

The `go_*` families observed are `go_gc_duration_seconds` (a summary with quantiles 0, 0.25, 0.5, 0.75, 1), `go_gc_gogc_percent`, `go_gc_gomemlimit_bytes`, `go_goroutines`, `go_info{version}`, 22 `go_memstats_*` families, `go_sched_gomaxprocs_threads` and `go_threads`. A Rust binary cannot produce these meaningfully. Drop them and document the drop, or keep only an equivalent `*_info`.

## 3. HTTP behaviour (`CG promhttp/http.go:282-412`)

**Content negotiation.**

- `EnableOpenMetrics` is false, so the accepted formats are `[protobuf-delimited, protobuf-text, protobuf-compact, text 0.0.4, text 1.0.0]` (`PC expfmt/encode.go:50-56`; `http.go:323-329`).
- `NegotiateAccept` walks the Accept entries in q order. If nothing matches, it falls back to the **first text/plain**, which is 0.0.4 (`encode.go:140-158`). `*/*` does not match protobuf, because the `proto` and `encoding` params are required (`encode.go:200-212`).
- **`application/openmetrics-text` is never served.** Observed: an OpenMetrics Accept header gets 0.0.4.
- `Content-Type` always gets `; escaping=underscores` appended. The default is `text/plain; version=0.0.4; charset=utf-8; escaping=underscores`. `Accept: text/plain;version=1.0.0` gets `version=1.0.0`, and the body is byte-identical for KSM's legacy-valid names (diffed).
- Prometheus's default scrape Accept lists OpenMetrics first, so Prometheus ends up with text 1.0.0.

**Headers and compression.**

- Two separate header lines: `Vary: Accept` and `Vary: Accept-Encoding` (`http.go:330-333`).
- gzip is chosen via Accept-Encoding and sets `Content-Encoding: gzip`. zstd is **not** offered, because `internal.NewZstdWriter` is nil unless the zstd sub-package is imported (`http.go:72-77,668-695`). Observed: `Accept-Encoding: zstd` gets an identity response.
- The body is chunked, with no Content-Length.

**Other request handling.**

- `?name[]=foo&name[]=bar` filters families by exact name (`http.go:385-406`). Unknown names give an empty 200 body.
- If `Gather()` fails: **500** with body `An error has occurred while serving metrics:\n\n<err>\n`, and `Content-Encoding` deleted (`http.go:297-316,655-662`). `HTTPErrorOnError` is the default (`:491`).
- Any method gets the same output: GET, POST and HEAD all get headers and the body is discarded for HEAD.
- With `--auth-filter`, see auth-filter.md.

**`/readyz`** (`server.go:605-614`; `KSM pkg/util/utils.go:145-156`):

- It runs a full `Gather()` and counts metric children.
- An error or 0 children gives **503 `Service Unavailable`**. Otherwise **200 `OK`**. There is no trailing newline, and Content-Type is the sniffed `text/plain; charset=utf-8`.
- `build_info`, the CRD counters and the shard gauges always exist, so the count is never 0. **`/readyz` is 503 only when Gather errors**, for example on duplicate or inconsistent metrics. It says nothing about informer sync.
- The other routes:
  - `/metrics` matches exactly. `/metrics/` returns 404 from the landing page.
  - `//metrics` and `/debug/pprof` return a 307 redirect from Go's ServeMux path cleaning (observed).
  - Any other path returns `404 page not found\n`.

## 4. Text format 0.0.4 rendering (`PC expfmt/text_create.go`)

- **Family order:** sorted by name with `sort.Strings`; empty families are dropped (`CG internal/metric.go:85-101`).
- **Label order within a series:** sorted by label name, with const and variable labels merged (`CG desc.go:187`, `value.go:234`).
- **Series order:** compared label value by label value in name order, byte-wise (`internal/metric.go:50-80`).
- `# HELP name text`: HELP escapes only `\` and `\n`. `# TYPE name type`. Label values escape `\`, `\n` and `"` (`text_create.go:462-470`). HELP is written only if set (`:107`). No `# UNIT`, no `# EOF`, no `_created`.
- **Counter** lines are `name value`. The family name already ends in `_total`; nothing is appended.
- **Histogram:** `_bucket{…,le="x"}` lines in bucket order, with `+Inf` added if missing, then `_sum`, then `_count`. The `le` label comes **last**, after the sorted labels (`:230-281`).
- **Summary:** `{quantile="q"}` lines, then `_sum`, then `_count`.
- **Floats** (`:476-497`):
  - `1` → `1`, `0` → `0`, `-1` → `-1`, NaN → `NaN`, ±Inf → `+Inf`/`-Inf`.
  - Everything else uses Go `strconv.FormatFloat(f,'g',-1,64)`: shortest round-trip digits, with exponent form when the decimal exponent is < -4 or ≥ 6 (`GOROOT/src/internal/strconv/ftoa.go:350-361`). Examples: `0.08`, `524288`, `5.4304768e+07`, `1.2520676383e+10`, `1.79572964268e+09`, `0.000212058`, `3.4506e-05`.
  - The exponent always has a sign and at least 2 digits.
  - The port already has this as `format_go_g` in `crates/ksm-core/src/metric.rs:157-236`.

## 5. Recommendation: a hand-written registry and encoder

These were compiled and run in the scratchpad (`web-research/enc`) with the same inputs:

| | `prometheus-client` 0.26.0 `encoding::prometheus_text` | `prometheus-client` `encoding::text` (OpenMetrics) | `prometheus` 0.14.0 `TextEncoder` |
|---|---|---|---|
| Family order | registration order | registration order | **sorted** (`CR prometheus-0.14.0 src/registry.rs:151-180`) |
| Label order | insertion order | insertion order, with `le` **first** | sorted, `le` last |
| HELP | **appends `.`** (`CR prometheus-client-0.26.0 src/registry.rs:354`): `seconds..` | same | verbatim |
| Counter names | must not end in `_total`; the encoder appends it (`registry.rs:177-178`, `prometheus_text.rs:305-321`) | TYPE line without `_total` | verbatim |
| Values | `dtoa`: `54304768.0`, `18446744073709553000.0` (`prometheus_text.rs:401-407`) | `dtoa`, `le="1.0"` | Rust `to_string()`: `54304768`, `18446744073709552000` (`src/encoder/text.rs:194`) |
| `le` | Go-style (`prometheus_text.rs:410+`) | `1.0` | Rust `to_string()` |
| Content-Type | `text/plain; version=0.0.4; charset=utf-8` | OpenMetrics, with `# EOF` (`text.rs:273`) | same without `escaping=` |

**Neither crate is byte-compatible.** The `prometheus` crate comes closest, differing only in float formatting. Recommendation: about 300 lines of hand-written code in `ksm-core`:

- An `Arc`-shared registry of typed families (counter/gauge `f64` atomics, a `HistogramVec` with fixed buckets, and const-label families).
- At scrape time, collect into `BTreeMap<name, family>` and sort series by label values.
- Reuse `format_go_g` and the label escaper that the main `/metrics` encoder already needs.
- Add a `/proc` reader for the `process_*` families.

This also lets `/readyz` count series without a second encode. For HTTP, send `Vary` twice, add `escaping=underscores`, support gzip only, and ignore OpenMetrics and protobuf Accepts by returning 0.0.4 (or 1.0.0 when explicitly asked for). A protobuf response is optional; recommend not implementing it, and record that in DECISIONS.md.

## Semantic traps

1. **The float format is Go `%g`-shortest:** `5.4304768e+07`, not `54304768` or `54304768.0`. Both Rust crates get this wrong.
2. **Families are sorted by name and labels by name, with `le` last.** prometheus-client keeps insertion order and also appends `.` to HELP and `_total` to counter names.
3. **`kube_state_metrics_list_total` and `list_objects*` are absent** with client-go v0.37. `WatchListClient` defaults to true from 1.35 (`$(go env GOMODCACHE)/k8s.io/client-go@v0.37.2/features/known_features.go:141-144`), so reflectors stream the initial list through Watch and `InstrumentedListerWatcher.List` is never called (observed). A kube-rs list+watch would emit series Go does not.
4. **`resource` labels are Go reflect type strings** (`*v1.Pod`, `*unstructured.Unstructured`) and must be reproduced verbatim.
5. **Lazily created series:** `http_request_duration_seconds` (first `:8080/metrics` hit), the config families (config files only), `object_limit_dropped_total` (first drop) and `list_objects_limit` (limit > 0) are missing until their trigger. Unlabelled counters and gauges (CRD events and cache, `total_shards`) are always present at 0.
6. **The histogram has no `code` label.** Unknown HTTP methods become `method="unknown"`. Requests rejected by `--auth-filter` are not observed.
7. **No OpenMetrics on telemetry, ever**, and `Content-Type` carries `; escaping=underscores`. The main metrics port, which uses KSM's own handler, does not add `escaping=`. Do not share one header constant between them.
8. **`/readyz` is effectively always 200.** It is not a readiness check for informers. The body is `OK` or `Service Unavailable` with no newline.
9. **`process_network_*` are namespace-wide counters.** `process_virtual_memory_max_bytes` is `1.8446744073709552e+19` when unlimited.
10. **`name[]` query filtering is supported** by promhttp 1.25. Scrapers rarely use it, but it is observable.
