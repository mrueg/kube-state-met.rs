#!/usr/bin/env python3
"""Compares Go and Rust server timeouts (--server-*-timeout) side by side:
how long each keeps a connection open for a stalled request and for an idle
keep-alive connection. Usage: tools/app-cases/timeouts.py"""
import os, socket, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def closed_after(p, send, first_response=False):
    s = socket.create_connection(("127.0.0.1", p))
    s.settimeout(10)
    s.sendall(send)
    t = time.time()
    if first_response:
        data = b""
        while b"\r\n\r\n" not in data:
            data += s.recv(65536)
        t = time.time()
    try:
        while s.recv(65536):
            pass
    except socket.timeout:
        return None
    return time.time() - t


def main():
    tmp = tempfile.mkdtemp()
    kc = os.path.join(tmp, "kc.yaml")
    log = open(os.path.join(tmp, "log"), "w")
    procs = [subprocess.Popen([os.path.join(ROOT, "target/debug/fake-apiserver"), "--listen", f"127.0.0.1:{port()}",
                               "--fixtures", os.path.join(ROOT, "testdata/fixtures"), "--kubeconfig-out", kc], stdout=log, stderr=log)]
    try:
        while not os.path.exists(kc):
            time.sleep(0.05)
        ports = {}
        for name, b in (("go", "target/ksm-go/kube-state-metrics"), ("rust", "target/debug/kube-state-metrics")):
            ports[name] = port()
            procs.append(subprocess.Popen([os.path.join(ROOT, b), "--kubeconfig", kc, "--port", str(ports[name]),
                                           "--telemetry-port", str(port()), "--host", "127.0.0.1", "--telemetry-host", "127.0.0.1",
                                           "--server-read-header-timeout=1s", "--server-read-timeout=3s",
                                           "--server-idle-timeout=2s"], stdout=log, stderr=log))
        time.sleep(3)
        ok = True
        cases = [
            ("stalled headers (read-header 1s)", b"GET /healthz HTTP/1.1\r\nHost: x\r\n", False, 1.0),
            ("idle keep-alive (idle 2s)", b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n", True, 2.0),
        ]
        for label, send, first, want in cases:
            got = {n: closed_after(p, send, first) for n, p in ports.items()}
            same = all(g is not None and abs(g - want) < 0.5 for g in got.values())
            ok &= same
            print(f"  {label}: go {got['go']:.2f}s rust {got['rust']:.2f}s {'ok' if same else 'DIFFERS'}")
        return 0 if ok else 1
    finally:
        for p in procs:
            p.terminate()


if __name__ == "__main__":
    sys.exit(main())
