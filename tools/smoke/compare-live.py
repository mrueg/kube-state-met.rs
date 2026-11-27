#!/usr/bin/env python3
"""Smoke test against a live cluster (read-only: list and watch only).
Runs Go and Rust kube-state-metrics out of cluster with the same kubeconfig and
compares their scrapes per family (ADR-003). A live cluster changes between
scrapes, so Go is scraped before and after Rust: a family counts as equal if
Rust matches either Go scrape, as noise if the two Go scrapes differ too.
Usage: tools/smoke/compare-live.py --kubeconfig PATH [--rounds 3] [extra ksm flags]"""
import argparse, importlib.util, os, socket, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
spec = importlib.util.spec_from_file_location("compare", os.path.join(ROOT, "tools/app-cases/compare.py"))
cmp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cmp)


def by_name(text):
    out = {}
    for h, t, lines in cmp.fams(text):
        out[(h.split(" ")[2] if h.startswith("# HELP") else h)] = (h, t, lines)
    return out, [k for k in out]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kubeconfig", required=True)
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--rust", default=os.path.join(ROOT, "target/debug/kube-state-metrics"))
    ap.add_argument("--go", default=os.path.join(ROOT, "target/ksm-go/kube-state-metrics"))
    a, extra = ap.parse_known_args()
    log = open(os.path.join(tempfile.mkdtemp(), "log"), "w")
    ports, procs = {}, []
    try:
        for name, b in (("go", a.go), ("rust", a.rust)):
            ports[name] = cmp.port()
            procs.append(subprocess.Popen([b, "--kubeconfig", a.kubeconfig, "--port", str(ports[name]),
                                           "--telemetry-port", str(cmp.port()), "--host", "127.0.0.1",
                                           "--telemetry-host", "127.0.0.1"] + extra, stdout=log, stderr=log))
        time.sleep(8)
        worst = 0
        for r in range(a.rounds):
            g1 = cmp.get(ports["go"], "/metrics")[2]
            rs = cmp.get(ports["rust"], "/metrics")[2]
            g2 = cmp.get(ports["go"], "/metrics")[2]
            (G1, order_g), (R, order_r), (G2, _) = by_name(g1), by_name(rs), by_name(g2)
            equal, noise, differ = 0, [], []
            for k in sorted(set(G1) | set(R)):
                if R.get(k) in (G1.get(k), G2.get(k)):
                    equal += 1
                elif G1.get(k) != G2.get(k):
                    noise.append(k)
                else:
                    differ.append(k)
            order_ok = order_g == order_r
            print(f"round {r + 1}: {equal} families equal, {len(noise)} changed between Go scrapes (noise), "
                  f"{len(differ)} differ; family order {'equal' if order_ok else 'DIFFERS'}; "
                  f"{sum(len(f[2]) for f in R.values())} lines")
            for k in differ[:5]:
                go_l, rs_l = (G1.get(k) or ("", "", []))[2], (R.get(k) or ("", "", []))[2]
                print(f"  differs: {k}\n    go only: {[l for l in go_l if l not in rs_l][:3]}\n    rs only: {[l for l in rs_l if l not in go_l][:3]}")
            for k in noise[:5]:
                print(f"  noise: {k}")
            worst = max(worst, len(differ) + (0 if order_ok else 1))
            time.sleep(5)
        return 1 if worst else 0
    finally:
        for p in procs:
            p.terminate()


if __name__ == "__main__":
    sys.exit(main())
