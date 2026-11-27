# Port map: kube-state-met.rs

Goal: a standalone Rust kube-state-metrics, equivalent per [DECISIONS.md](DECISIONS.md) (ADR-003, ADR-004),
with benchmarks (ADR-008). Original: `../kube-state-metrics` @ `46cd52ff` (reference only).

Read first: [DECISIONS.md](DECISIONS.md), [GLOSSARY.md](GLOSSARY.md), `rust-anything rules .`, [HANDOFF.md](HANDOFF.md) if present.
Generated plan: [analyze-whole-app.md](analyze-whole-app.md). Research: [research/](research/).

## ADR index
001 layout · 002 scope · 003 equivalence bar · 004 compatibility · 005 sharding hash · 006 libraries ·
007 ground truth · 008 benchmarks · 009 targets/checkpoints/cutover · 010 telemetry · 011 watch semantics · 012 no proxying front door ·
013 TLS web config on rustls

## Units

| Wave | Unit (Go) | Rust home | State |
| ---: | --- | --- | --- |
| 0 | ground truth: Go golden harness, fake apiserver, app record | `tools/`, `testdata/golden/`, `rust/app-cases/` | goldens + fake apiserver done; app record open |
| 0 | front door | `crates/ksm` | dropped (ADR-012) |
| 1 | `pkg/metric`, `pkg/metric_generator` | `ksm-core::metric`, `::generator` | done |
| 1 | `pkg/metrics_store` | `ksm-core::store` | done |
| 1 | `pkg/options` | `ksm::options` | done: 182 argv cases + 347 option cases from Go |
| 1 | `pkg/allowdenylist`, `pkg/optin` | `ksm-core::filter` | done |
| 1 | `pkg/sharding`, `pkg/watch` | `ksm-core::sharding`, `ksm::watch` | done: go-jump vectors; sharded scrapes equal to Go |
| 1 | `internal/store/utils.go`, quantity, field selectors | `ksm-core::util`, `::quantity`, `::fields` | done (goldens: labels 911, float 3203, quantity 813) |
| 2 | `pkg/metricshandler`, `internal/store/builder.go`, `pkg/app` (server, telemetry) | `crates/ksm` | done: compare.py equal on 11 configurations; 16/16 recorded HTTP cases |
| 3 | `internal/store/*` (40 stores) | `ksm-stores` | done: 267/267 store goldens match Go |
| 4 | `pkg/customresourcestate`, `internal/discovery` | `ksm-crs`, `ksm::{crs,discovery,watch_dynamic}` | done: CRD discovery, wildcard GVKs, CR watchers; `compare.py --custom-resource-state-config-file=testdata/crs-config.yaml` equal |
| 4 | TLS web config (exporter-toolkit `web`) | `ksm::web` | done: all 49 exporter-toolkit test configs equal to Go (startup errors, HTTP/TLS answers); rustls limits in ADR-013 |
| 4 | `--auth-filter` (controller-runtime `WithAuthenticationAndAuthorization`) | `ksm::auth` | done: 19 request cases equal to Go against the fake apiserver's TokenReview/SubjectAccessReview (`--auth-token`, `--auth-allow`); WebSocket-protocol tokens not ported |
| 4 | hot reload (`internal/wrapper.go`, `internal/watch.go`) | `ksm::{main,reload}` | done: options config, CRS config and kubeconfig watched (parent dir, symlink swaps, movable watches), restart with reload flag + 3 s port wait, `--continue-without-*`; edit/break/fix/atomic-save sequence serves the same as Go at every step |
| 2 | `--server-*-timeout` (Go `http.Server` deadlines) | `ksm::conn` | done: header/read/write/idle deadlines with Go's semantics, TLS handshake timeout; `tools/app-cases/timeouts.py` closes connections at the same moment as Go |
| 5 | benchmarks report | `bench/` | done: `bench/run.sh` → `bench/RESULTS.md` |
| 5 | container image, CI | `Dockerfile`, `.github/workflows/ci.yml` | done: amd64 image runs against the fake apiserver; arm64 image builds (static aarch64, not run: no emulation on the build host); CI not run yet |
| 5 | lift | — | not applicable (ADR-012: no front door or unit bridges to remove) |

## Open questions
- Cobra's hidden `__complete` command is not ported (zsh/fish dynamic completion).

## Out of scope
- `go_*` telemetry metrics (ADR-010); GOMEMLIMIT behavior (ADR-002).
