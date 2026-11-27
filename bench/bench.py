#!/usr/bin/env python3
"""End-to-end benchmark: kube-state-metrics in Go vs kube-state-met.rs (ADR-008).

Both binaries run against the same fake API server serving a generated
cluster. For every scale and implementation it measures:
  - sync time: start until /metrics reports every pod (kube_pod_info lines)
  - scrape latency p50 / p90 / p99 and response size (N scrapes, text format)
  - gzip scrape latency p50
  - RSS after sync, peak RSS (VmHWM), CPU seconds used (start..end)
With --churn RATE, a second fake API server sends RATE pod MODIFIED events per
second to the open watches; after sync it measures the CPU (cores) and RSS
each binary needs to keep up, over --churn-seconds without scrapes.

Scrapes use curl, timing the whole body (gzip: the compressed body, not decompressed).

Usage: bench/bench.py [--scales 1000,10000,50000] [--scrapes 30] [--churn 1000] [--churn-seconds 20] [--out bench/RESULTS.md]
       [--go PATH] [--rust PATH] [--fake PATH]
Build first: cargo build --release -p kube-state-metrics -p fake-apiserver,
and the Go binary (bench/run.sh does all of it).
"""
import argparse, os, platform, signal, socket, statistics, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def proc_status(pid, key):
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith(key + ":"):
                return int(line.split()[1]) * 1024
    except OSError:
        return 0
    return 0


def cpu_seconds(pid):
    try:
        f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return (int(f[11]) + int(f[12])) / os.sysconf("SC_CLK_TCK")
    except OSError:
        return 0.0


def scrape(port, gzip=False, timeout=120, keep=False):
    """One scrape with curl (urllib adds ~0.4s per 400MB body). Returns (seconds, body or size)."""
    out = tempfile.NamedTemporaryFile(delete=False) if keep else None
    cmd = ["curl", "-sS", "--fail", "--max-time", str(timeout), "-o", out.name if out else "/dev/null",
           "-w", "%{time_total} %{size_download}", f"http://127.0.0.1:{port}/metrics"]
    if gzip:
        cmd[1:1] = ["-H", "Accept-Encoding: gzip"]
    r = subprocess.run(cmd, capture_output=True, text=True, check=True)
    t, size = r.stdout.split()
    if out:
        body = open(out.name, "rb").read()
        os.unlink(out.name)
        return float(t), body
    return float(t), int(size)


def wait_port(port, deadline):
    while time.time() < deadline:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            return True
        except OSError:
            time.sleep(0.05)
    return False


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def run_one(binary, kubeconfig, pods, scrapes, log):
    port, tport = free_port(), free_port()
    start = time.time()
    p = subprocess.Popen([binary, "--kubeconfig", kubeconfig, "--port", str(port), "--telemetry-port", str(tport),
                          "--host", "127.0.0.1", "--telemetry-host", "127.0.0.1", "--enable-gzip-encoding"],
                         stdout=log, stderr=log)
    try:
        if not wait_port(port, start + 60):
            raise RuntimeError(f"{binary} did not listen")
        sync = None
        while time.time() < start + 600:
            try:
                _, body = scrape(port, keep=True)
                if body.count(b"\nkube_pod_info{") >= pods:
                    sync = time.time() - start
                    break
            except Exception:
                pass
            time.sleep(0.1)
        if sync is None:
            raise RuntimeError(f"{binary} did not sync {pods} pods")
        rss_sync = proc_status(p.pid, "VmRSS")
        times, size = [], 0
        for _ in range(scrapes):
            t, size = scrape(port)
            times.append(t)
        gz, gz_size = [], 0
        for _ in range(max(3, scrapes // 3)):
            t, gz_size = scrape(port, gzip=True)
            gz.append(t)
        if gz_size >= size:
            raise RuntimeError(f"{binary}: gzip scrape was not compressed")
        return {
            "sync_s": sync, "p50_ms": pct(times, 50) * 1e3, "p90_ms": pct(times, 90) * 1e3,
            "p99_ms": pct(times, 99) * 1e3, "gzip_p50_ms": pct(gz, 50) * 1e3, "bytes": size, "gz_bytes": gz_size,
            "rss_sync_mb": rss_sync / 2**20, "rss_peak_mb": proc_status(p.pid, "VmHWM") / 2**20,
            "rss_end_mb": proc_status(p.pid, "VmRSS") / 2**20, "cpu_s": cpu_seconds(p.pid),
        }
    finally:
        p.send_signal(signal.SIGTERM)
        try:
            p.wait(10)
        except subprocess.TimeoutExpired:
            p.kill()


def run_churn(binary, kubeconfig, pods, seconds, log):
    port, tport = free_port(), free_port()
    start = time.time()
    p = subprocess.Popen([binary, "--kubeconfig", kubeconfig, "--port", str(port), "--telemetry-port", str(tport),
                          "--host", "127.0.0.1", "--telemetry-host", "127.0.0.1"], stdout=log, stderr=log)
    try:
        if not wait_port(port, start + 60):
            raise RuntimeError(f"{binary} did not listen")
        while time.time() < start + 600:
            try:
                if scrape(port, keep=True)[1].count(b"\nkube_pod_info{") >= pods:
                    break
            except Exception:
                pass
            time.sleep(0.1)
        else:
            raise RuntimeError(f"{binary} did not sync {pods} pods")
        time.sleep(3)  # settle after the sync scrapes
        c0, t0 = cpu_seconds(p.pid), time.time()
        time.sleep(seconds)
        cores = (cpu_seconds(p.pid) - c0) / (time.time() - t0)
        return {"cores": cores, "rss_mb": proc_status(p.pid, "VmRSS") / 2**20,
                "rss_peak_mb": proc_status(p.pid, "VmHWM") / 2**20}
    finally:
        p.send_signal(signal.SIGTERM)
        try:
            p.wait(10)
        except subprocess.TimeoutExpired:
            p.kill()


def start_fake(fake, spec, kc, log, extra=()):
    proc = subprocess.Popen([fake, "--listen", f"127.0.0.1:{free_port()}", "--generate", spec,
                             "--kubeconfig-out", kc, *extra], stdout=log, stderr=log)
    deadline = time.time() + 120
    while not os.path.exists(kc) and time.time() < deadline:
        time.sleep(0.05)
    return proc


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scales", default="1000,10000,50000")
    ap.add_argument("--scrapes", type=int, default=30)
    ap.add_argument("--go", default=os.path.join(ROOT, "target/ksm-go/kube-state-metrics"))
    ap.add_argument("--rust", default=os.path.join(ROOT, "target/release/kube-state-metrics"))
    ap.add_argument("--fake", default=os.path.join(ROOT, "target/release/fake-apiserver"))
    ap.add_argument("--out", default=os.path.join(ROOT, "bench/RESULTS.md"))
    ap.add_argument("--churn", type=int, default=1000, help="pod events per second for the churn run (0: skip)")
    ap.add_argument("--churn-seconds", type=int, default=20)
    a = ap.parse_args()
    results, churn = [], []
    tmp = tempfile.mkdtemp(prefix="ksm-bench-")
    log = open(os.path.join(tmp, "bench.log"), "w")
    for pods in [int(s) for s in a.scales.split(",")]:
        spec = (f"pods={pods},deployments={pods // 10},replicasets={pods // 5},services={pods // 20},"
                f"configmaps={pods // 10},secrets={pods // 10},jobs={pods // 50}")
        kc = os.path.join(tmp, f"kubeconfig-{pods}")
        fake = start_fake(a.fake, spec, kc, log)
        try:
            for name, binary in (("go", a.go), ("rust", a.rust)):
                r = run_one(binary, kc, pods, a.scrapes, log)
                r.update(impl=name, pods=pods, spec=spec)
                results.append(r)
                print(f"{pods:>6} pods {name:>4}: sync {r['sync_s']:.2f}s  p50 {r['p50_ms']:.1f}ms  p99 {r['p99_ms']:.1f}ms  gzip {r['gzip_p50_ms']:.1f}ms  "
                      f"rss {r['rss_end_mb']:.0f}MB (peak {r['rss_peak_mb']:.0f})  cpu {r['cpu_s']:.1f}s", flush=True)
        finally:
            fake.terminate()
            fake.wait()
        if a.churn > 0:
            kc = os.path.join(tmp, f"kubeconfig-churn-{pods}")
            fake = start_fake(a.fake, spec, kc, log, ("--churn", str(a.churn)))
            try:
                for name, binary in (("go", a.go), ("rust", a.rust)):
                    r = run_churn(binary, kc, pods, a.churn_seconds, log)
                    r.update(impl=name, pods=pods, rate=a.churn)
                    churn.append(r)
                    print(f"{pods:>6} pods {name:>4}: churn {a.churn}/s  cpu {r['cores']:.2f} cores  "
                          f"rss {r['rss_mb']:.0f}MB (peak {r['rss_peak_mb']:.0f})", flush=True)
            finally:
                fake.terminate()
                fake.wait()
    write_report(a.out, results, a.scrapes, churn, a.churn_seconds)


def write_report(path, results, scrapes, churn=(), churn_seconds=0):
    cpu = "unknown"
    try:
        cpu = next(l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name"))
    except (OSError, StopIteration):
        pass
    lines = [
        "# Benchmarks: kube-state-metrics (Go) vs kube-state-met.rs (Rust)", "",
        f"Generated by `bench/run.sh` on {time.strftime('%Y-%m-%d')}, {platform.system()} {platform.machine()}, "
        f"{cpu}, {os.cpu_count()} CPUs. Both binaries ran against the same fake API server "
        f"(tools/fake-apiserver, generated cluster: pods plus deployments, replicasets, services, configmaps, "
        f"secrets, jobs, namespaces and nodes in proportion), default resources, {scrapes} scrapes each.", "",
        "| Pods | Impl | Sync (s) | Scrape p50 (ms) | p90 (ms) | p99 (ms) | gzip p50 (ms) | Body (MB) | gzip body (MB) | RSS after sync (MB) | Peak RSS (MB) | RSS end (MB) | CPU (s) |",
        "| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for r in results:
        lines.append(f"| {r['pods']} | {r['impl']} | {r['sync_s']:.2f} | {r['p50_ms']:.1f} | {r['p90_ms']:.1f} | "
                     f"{r['p99_ms']:.1f} | {r['gzip_p50_ms']:.1f} | {r['bytes'] / 2**20:.1f} | {r['gz_bytes'] / 2**20:.1f} | {r['rss_sync_mb']:.0f} | "
                     f"{r['rss_peak_mb']:.0f} | {r['rss_end_mb']:.0f} | {r['cpu_s']:.1f} |")
    lines += ["", "Ratios (Go / Rust; above 1 means Rust is better):", "",
              "| Pods | Sync | Scrape p50 | gzip p50 | Peak RSS | RSS end | CPU |", "| ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    by = {(r["pods"], r["impl"]): r for r in results}
    for pods in sorted({r["pods"] for r in results}):
        g, rs = by.get((pods, "go")), by.get((pods, "rust"))
        if g and rs:
            f = lambda k: f"{g[k] / rs[k]:.2f}x" if rs[k] else "-"
            lines.append(f"| {pods} | {f('sync_s')} | {f('p50_ms')} | {f('gzip_p50_ms')} | {f('rss_peak_mb')} | {f('rss_end_mb')} | {f('cpu_s')} |")
    if churn:
        lines += ["", f"Event load: the fake API server modifies {churn[0]['rate']} pods per second (MODIFIED watch events); "
                  f"CPU averaged over {churn_seconds}s after sync, no scrapes.", "",
                  "| Pods | Impl | Events/s | CPU (cores) | RSS (MB) | Peak RSS (MB) |",
                  "| ---: | --- | ---: | ---: | ---: | ---: |"]
        for r in churn:
            lines.append(f"| {r['pods']} | {r['impl']} | {r['rate']} | {r['cores']:.2f} | {r['rss_mb']:.0f} | {r['rss_peak_mb']:.0f} |")
    lines += ["", "Notes: the sync time includes process start and the first full list of every resource; "
              "the fake API server answers instantly, so it measures kube-state-metrics' own processing. "
              "The Go client requests protobuf but the fake server answers JSON, as it does for the Rust client.", ""]
    open(path, "w").write("\n".join(lines))
    print(f"wrote {path}")


if __name__ == "__main__":
    sys.exit(main())
