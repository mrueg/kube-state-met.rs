# Handoff

State (2026-11-27): every unit in [MAP.md](MAP.md) is done; the Rust binary is the whole app (ADR-012).

- Proof: `cargo test --workspace` (Go goldens), `python3 tools/app-cases/compare.py [flags]` (side by side with Go,
  needs `target/ksm-go/kube-state-metrics`: `cd ../kube-state-metrics && go build -o ../kube-state-met.rs/target/ksm-go/kube-state-metrics .`),
  `tools/app-cases/record.sh replay` (16/16 recorded HTTP cases, report in `rust/app-cases/replay-report.md`).
- Benchmarks: `bench/run.sh` → [bench/RESULTS.md](../../bench/RESULTS.md).
- Decisions: [DECISIONS.md](DECISIONS.md) ADR-001..013; rules for new work: `rust/RULES.md`.
- Follow-ups: the two open questions in MAP.md; after pulling upstream, re-run `tools/golden-go/*/run.sh` and
  `tools/app-cases/record.sh`, then port what changed.
- Upstream drift: `rust-anything app drift .` replays the original and the build against the recorded cases.
