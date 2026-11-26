# Port map: kube-state-met.rs

Goal: a standalone Rust kube-state-metrics, equivalent per [DECISIONS.md](DECISIONS.md) (ADR-003, ADR-004),
with benchmarks (ADR-008). Original: `../kube-state-metrics` @ `46cd52ff` (reference only).

Read first: [DECISIONS.md](DECISIONS.md), [GLOSSARY.md](GLOSSARY.md), `rust-anything rules .`, [HANDOFF.md](HANDOFF.md) if present.
Generated plan: [analyze-whole-app.md](analyze-whole-app.md). Research: [research/](research/).

## ADR index
001 layout · 002 scope · 003 equivalence bar · 004 compatibility · 005 sharding hash · 006 libraries ·
007 ground truth · 008 benchmarks · 009 targets/checkpoints/cutover · 010 telemetry · 011 watch semantics

## Units

| Wave | Unit (Go) | Rust home | State |
| ---: | --- | --- | --- |
| 0 | ground truth: Go golden harness, fake apiserver, app record | `tools/`, `testdata/golden/`, `rust/app-cases/` | open |
| 0 | front door (Rust binary, proxies what isn't ported to Go) | `crates/ksm` | open |
| 1 | `pkg/metric`, `pkg/metric_generator` | `ksm-core::metric`, `::generator` | open |
| 1 | `pkg/metrics_store` | `ksm-core::store` | open |
| 1 | `pkg/options` | `ksm-core::options` | open |
| 1 | `pkg/allowdenylist`, `pkg/optin` | `ksm-core::filter` | open |
| 1 | `pkg/sharding`, `pkg/watch` | `ksm-core::sharding`, `ksm::watch` | open |
| 1 | `internal/store/utils.go`, quantity, field selectors | `ksm-core::util`, `::quantity`, `::fields` | open |
| 2 | `pkg/metricshandler`, `internal/store/builder.go`, `pkg/app` (server, telemetry) | `crates/ksm` | open |
| 3 | `internal/store/*` (40 stores) | `ksm-stores` | open |
| 4 | `pkg/customresourcestate`, `internal/discovery` | `ksm-crs` | open |
| 4 | auth filter, hot reload (`internal/wrapper.go`), TLS web config | `crates/ksm` | open |
| 5 | lift, benchmarks report, container image | | open |

## Open questions
- (none)

## Out of scope
- `go_*` telemetry metrics (ADR-010); GOMEMLIMIT behavior (ADR-002).
