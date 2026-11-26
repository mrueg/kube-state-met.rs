#!/usr/bin/env python3
"""Scrapes Go and Rust kube-state-metrics side by side against the fake API
server and compares by ADR-003: same families in the same order with the same
HELP/TYPE, the same multiset of lines inside each family.
Usage: tools/app-cases/compare.py [--rust PATH] [--go PATH] [--fixtures DIR] [extra ksm args...]"""
import argparse, os, socket, subprocess, sys, tempfile, time, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ALL = ("certificatesigningrequests,clusterrolebindings,clusterroles,configmaps,cronjobs,daemonsets,deployments,"
       "endpoints,endpointslices,horizontalpodautoscalers,ingressclasses,ingresses,jobs,leases,limitranges,"
       "mutatingadmissionpolicies,mutatingadmissionpolicybindings,mutatingwebhookconfigurations,namespaces,"
       "networkpolicies,nodes,persistentvolumeclaims,persistentvolumes,poddisruptionbudgets,pods,replicasets,"
       "replicationcontrollers,resourcequotas,rolebindings,roles,secrets,serviceaccounts,services,statefulsets,"
       "storageclasses,validatingadmissionpolicies,validatingadmissionpolicybindings,validatingwebhookconfigurations,"
       "volumeattachments,volumeattributesclasses")


def port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def fams(text):
    out, cur = [], None
    for l in text.splitlines(keepends=True):
        if l.startswith("# HELP"):
            cur = [l, "", []]; out.append(cur)
        elif l.startswith("# TYPE") and cur is not None and not cur[1]:
            cur[1] = l
        elif l.startswith("# EOF"):
            out.append(["EOF", "", []])
        else:
            if cur is None:
                cur = ["", "", []]; out.append(cur)
            cur[2].append(l)
    return [(h, t, sorted(ls)) for h, t, ls in out]


def compare(name, a, b):
    fa, fb = fams(a), fams(b)
    for i, (x, y) in enumerate(zip(fa, fb)):
        if x != y:
            print(f"  {name}: family {i} differs\n    go: {x[0].strip()} | {x[1].strip()}\n    rs: {y[0].strip()} | {y[1].strip()}")
            sx, sy = x[2], y[2]
            print("    go only:", [l for l in sx if l not in sy][:4])
            print("    rs only:", [l for l in sy if l not in sx][:4])
            return False
    if len(fa) != len(fb):
        print(f"  {name}: {len(fa)} go families vs {len(fb)} rust")
        extra = (fa if len(fa) > len(fb) else fb)[min(len(fa), len(fb))]
        print("    first extra:", extra[0].strip(), extra[2][:2])
        return False
    print(f"  {name}: equal ({len(fa)} families, {sum(len(f[2]) for f in fa)} lines)")
    return True


def get(port, path, headers=None):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, {k.lower(): v for k, v in e.headers.items()}, e.read().decode()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rust", default=os.path.join(ROOT, "target/debug/kube-state-metrics"))
    ap.add_argument("--go", default=os.path.join(ROOT, "target/ksm-go/kube-state-metrics"))
    ap.add_argument("--fixtures", default=os.path.join(ROOT, "testdata/fixtures"))
    a, extra = ap.parse_known_args()
    tmp = tempfile.mkdtemp()
    kc = os.path.join(tmp, "kc.yaml")
    log = open(os.path.join(tmp, "log"), "w")
    env = dict(os.environ, TZ="UTC")
    procs = [subprocess.Popen([os.path.join(ROOT, "target/debug/fake-apiserver"), "--listen", f"127.0.0.1:{port()}",
                               "--fixtures", a.fixtures, "--kubeconfig-out", kc], stdout=log, stderr=log)]
    try:
        while not os.path.exists(kc):
            time.sleep(0.05)
        ports = {}
        for name, binary in (("go", a.go), ("rust", a.rust)):
            p = port()
            ports[name] = p
            args = [binary, "--kubeconfig", kc, "--port", str(p), "--telemetry-port", str(port()), "--host", "127.0.0.1",
                    "--telemetry-host", "127.0.0.1", "--resources=" + ALL, "--enable-gzip-encoding"] + extra
            procs.append(subprocess.Popen(args, stdout=log, stderr=log, env=env))
        time.sleep(4)
        ok = True
        for path, hdr in [("/metrics", None), ("/metrics", {"Accept": "application/openmetrics-text; version=1.0.0"}),
                          ("/metrics?resources=pods,nodes", None), ("/metrics?exclude_resources=pods", None)]:
            g, r = get(ports["go"], path, hdr), get(ports["rust"], path, hdr)
            label = path + (" (openmetrics)" if hdr else "")
            if g[0] != r[0] or g[1].get("content-type") != r[1].get("content-type"):
                print(f"  {label}: status/content-type go={g[0]} {g[1].get('content-type')} rust={r[0]} {r[1].get('content-type')}")
                ok = False
            ok &= compare(label, g[2], r[2])
        for path in ["/healthz", "/livez", "/", "/nope"]:
            g, r = get(ports["go"], path), get(ports["rust"], path)
            import re
            norm = lambda t: re.sub(r"Version: \(version=[^)]*\)", "Version: (VERSION)", t)
            same = g[0] == r[0] and norm(g[2]) == norm(r[2]) and g[1].get("content-type") == r[1].get("content-type")
            print(f"  {path}: {'equal' if same else f'differs: go {g[0]} {g[2][:60]!r} rust {r[0]} {r[2][:60]!r}'}")
            ok &= same
        return 0 if ok else 1
    finally:
        for p in procs:
            p.terminate()
        print("logs:", os.path.join(tmp, "log"))


if __name__ == "__main__":
    sys.exit(main())
