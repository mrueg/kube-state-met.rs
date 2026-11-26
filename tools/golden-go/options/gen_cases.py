#!/usr/bin/env python3
"""Records the Go kube-state-metrics command line behaviour as test tables.

Usage: gen_cases.py GO_BINARY HARNESS OUT_DIR
Writes OUT_DIR/cli_cases.json and OUT_DIR/options_go_cases.json.
"""
import json
import os
import re
import subprocess
import sys

GO, HARNESS, OUT = sys.argv[1], sys.argv[2], sys.argv[3]


def go_unquote(s):
    out = []
    i = 0
    b = s.encode("utf-8")
    raw = bytearray()
    while i < len(b):
        c = b[i]
        if c == 0x5C:  # backslash
            n = chr(b[i + 1])
            if n in "\"\\'":
                raw.append(ord(n)); i += 2
            elif n in "abfnrtv":
                raw += {"a": b"\a", "b": b"\b", "f": b"\f", "n": b"\n", "r": b"\r", "t": b"\t", "v": b"\v"}[n]; i += 2
            elif n == "x":
                raw.append(int(b[i + 2:i + 4], 16)); i += 4
            elif n == "u":
                raw += chr(int(b[i + 2:i + 6], 16)).encode(); i += 6
            elif n == "U":
                raw += chr(int(b[i + 2:i + 10], 16)).encode(); i += 10
            else:
                raise ValueError(s)
        else:
            raw.append(c); i += 1
    return raw.decode("utf-8", errors="replace")


ERR_RE = re.compile(r'"Running kube-state-metrics failed" err="((?:[^"\\]|\\.)*)"')

A = "--kubeconfig"
CLI = [
    [], ["-h"], ["--help"], ["--help=true"], ["--help=false"], ["-h=x"], ["--help=x"], ["-h", "--port=abc"],
    ["--port=abc", "-h"], ["help"], ["help", "version"], ["help", "help"], ["help", "-h"], ["help", "completion"],
    ["help", "bogus"], ["help", "bogus", "extra"], ["help", "version", "extra"], ["help", "--port"], ["help", "-v"],
    ["help", "--", "--port", "version"], ["help", "comp"], ["help", "c"],
    ["version"], ["version", "extra"], ["version", "-h"], ["version", "--help"], ["version", "--help=false"],
    ["version", "--port", "1"], ["--port", "1", "version"], ["-v", "2", "version"], ["--port", "version"],
    ["--auth-filter", "version"], ["version", "--", "x"],
    ["completion"], ["completion", "bash"], ["completion", "zsh"], ["completion", "fish"], ["completion", "foo"],
    ["completion", "bash", "zsh"], ["comp", "bash"], ["c", "fish"], ["completion", "-h"], ["completion", "bash", "-h"],
    ["comp", "-h"], ["completion", "--no-descriptions", "bash"], ["completion", "--", "bash"],
    ["foo"], ["foo", "bar"], ["-", ], [""], ["--", "foo"], ["--port=1", "--", "foo"], ["--"], ["--port=1", "--"],
    ["--bogus"], ["--bogus=1"], ["-x"], ["-x=1"], ["-é"], ["-hé"], ["---x"], ["--=x"], ["--port abc"],
    ["--port"], ["--port", "abc"], ["--port=abc"], ["--port=0x1F"], ["--port=010"], ["--port=8_080"], ["--port=_8"],
    ["--port=99999999999999999999"], ["--port=-1"], ["--port="], ["--port", ""],
    ["--telemetry-port=x"], ["--total-shards=0"], ["--total-shards=-3"], ["--total-shards=x"], ["--total-shards=2", "--shard=1"],
    ["--total-shards=2", "--shard=2"], ["--shard=-1"], ["--shard=2147483648"], ["--shard=x"], ["--shard=5"],
    ["--object-limit=-1"], ["--object-limit=1.5"], ["--object-limit=100"],
    ["--auto-gomemlimit-ratio=0"], ["--auto-gomemlimit-ratio=1"], ["--auto-gomemlimit-ratio=1.5"], ["--auto-gomemlimit-ratio=-0.5"],
    ["--auto-gomemlimit-ratio=nan"], ["--auto-gomemlimit-ratio=inf"], ["--auto-gomemlimit-ratio=-Inf"], ["--auto-gomemlimit-ratio=abc"],
    ["--auto-gomemlimit-ratio=1e400"], ["--auto-gomemlimit-ratio=0x1p-1"], ["--auto-gomemlimit-ratio=.5"], ["--auto-gomemlimit-ratio=1e-7"],
    ["--auto-gomemlimit-ratio=0.0078125"], ["--auto-gomemlimit-ratio=2e10"],
    ["--auto-gomemlimit-refresh-interval=-1s"], ["--auto-gomemlimit-refresh-interval=-1.5ms"], ["--auto-gomemlimit-refresh-interval=1m"],
    ["--auto-gomemlimit-refresh-interval=1"], ["--auto-gomemlimit-refresh-interval=abc"], ["--auto-gomemlimit-refresh-interval=1x"],
    ["--auto-gomemlimit-refresh-interval=-0"], ["--auto-gomemlimit-refresh-interval=9999999999h"],
    ["--server-read-timeout=-5s"], ["--server-read-timeout=.s"], ["--server-read-timeout=1µs"], ["--shutdown-delay=1h1h"],
    ["--auth-filter"], ["--auth-filter=false"], ["--auth-filter=0"], ["--auth-filter=T"], ["--auth-filter=yes"], ["--auth-filter", "false"],
    ["--auto-gomemlimit"], ["--enable-gzip-encoding=x"],
    ["--node=n"], ["--node=n", "--resources=pods"], ["--node=n", "--resources=pods,secrets"], ["--node=n", "--resources=secrets", "--shard=3"],
    ["--resources=pods", "--resources=secrets"], ["--metric-labels-allowlist=pods=[a**]"], ["--metric-labels-allowlist=pods"],
    ["--metric-labels-allowlist=pods=[a],"], ["--metric-annotations-allowlist=pods=[a,,b]"], ["--metric-labels-allowlist="],
    ["-v", "2"], ["-v=2"], ["-v2"], ["--v=3", "-v=4"], ["-v"], ["-v", "x"], ["-vh"], ["-hv"], ["-hv", "2"], ["-v", "99999999999"],
    ["--v"], ["--v="], ["-test.v"], ["--test.v"],
    ["--stderrthreshold=foo"], ["--stderrthreshold=warning"], ["--stderrthreshold=7"], ["--alsologtostderrthreshold=x"],
    ["--log_file_max_size=x"], ["--log_file_max_size=-1"], ["--log_file_max_size=99999999999999999999"], ["--log_file_max_size=0x10"],
    ["--add_dir_header=x"], ["--add_dir_header=1"], ["--logtostderr=false"], ["--logtostderr"], ["--one_output=TRUE"],
    ["--skip_headers"], ["--skip_log_headers=f"], ["--legacy_stderr_threshold_behavior"],
    ["--vmodule=a=1,b=2"], ["--vmodule=a"], ["--vmodule=a=x"], ["--vmodule=a=-1"], ["--vmodule=,"], ["--vmodule=a=1=2"],
    ["--log_backtrace_at=foo.go:12"], ["--log_backtrace_at=foo:12"], ["--log_backtrace_at=foo.go:0"], ["--log_backtrace_at=foo.go:x"],
    ["--log_backtrace_at="], ["--log_dir=/tmp", "--log_file=/tmp/x"],
    ["--log-dir=/tmp"], ["--namespaces=a,b", "--namespaces-denylist=c"],
]

SHELL_CASES = [
    (["completion", "-h"], "/bin/zsh"), (["completion", "-h"], "/usr/bin/fish"), (["completion", "-h"], "bash"),
    (["completion", "-h"], "/bin/tcsh"), (["completion", "-h"], ""), (["completion", "-h"], None),
    (["help", "completion"], None),
]


def run_cli(args, shell="/bin/bash"):
    env = {k: v for k, v in os.environ.items() if not k.startswith("KUBERNETES_")}
    env["HOME"] = "/nonexistent"
    env.pop("SHELL", None)
    if shell is not None:
        env["SHELL"] = shell
    p = subprocess.run([GO] + args, capture_output=True, env=env, cwd="/", timeout=20)
    stdout = p.stdout.decode()
    stderr = p.stderr.decode()
    case = {"args": args, "shell": shell, "exit": p.returncode}
    m = ERR_RE.search(stderr)
    if m:
        case["err"] = go_unquote(m.group(1))
    elif "Starting kube-state-metrics" in stderr or (p.returncode != 0 and not stdout):
        # A raised -stderrthreshold hides the "Starting" line.
        case["run"] = True
    else:
        if stdout.startswith("kube-state-metrics, version"):
            stdout = "<version>\n"
        case["stdout"] = stdout
    return case


cli = [run_cli(a) for a in CLI] + [run_cli(a, s) for a, s in SHELL_CASES]
with open(os.path.join(OUT, "cli_cases.json"), "w", encoding="utf-8") as f:
    json.dump(cli, f, indent=1, ensure_ascii=False)
    f.write("\n")

# ---------------------------------------------------------------------------
# Harness cases

FIELDS = {
    "AnnotationsAllowList": "annotations_allow_list", "LabelsAllowList": "labels_allow_list",
    "MetricAllowlist": "metric_allowlist", "MetricDenylist": "metric_denylist", "MetricOptInList": "metric_opt_in_list",
    "Resources": "resources", "Apiserver": "apiserver", "CustomResourceConfig": "custom_resource_config",
    "CustomResourceConfigFile": "custom_resource_config_file",
    "CustomResourceConfigFileDeprecated": "custom_resource_config_file_deprecated",
    "ContinueWithoutCustomResourceConfigFile": "continue_without_custom_resource_config_file", "Host": "host",
    "Kubeconfig": "kubeconfig", "Namespace": "namespace", "Node": "node", "Pod": "pod", "TLSConfig": "tls_config",
    "TelemetryHost": "telemetry_host", "Config": "config", "ContinueWithoutConfig": "continue_without_config",
    "Namespaces": "namespaces", "NamespacesDenylist": "namespaces_denylist", "AutoGoMemlimitRatio": "auto_go_memlimit_ratio",
    "Port": "port", "TelemetryPort": "telemetry_port", "TotalShards": "total_shards",
    "ServerReadTimeout": "server_read_timeout", "ServerWriteTimeout": "server_write_timeout",
    "ServerIdleTimeout": "server_idle_timeout", "ServerReadHeaderTimeout": "server_read_header_timeout",
    "ShutdownDelay": "shutdown_delay", "AutoGoMemlimitRefreshInterval": "auto_go_memlimit_refresh_interval",
    "Shard": "shard", "AutoGoMemlimit": "auto_go_memlimit", "CustomResourcesOnly": "custom_resources_only",
    "EnableGZIPEncoding": "enable_gzip_encoding", "Help": "help", "TrackUnscheduledPods": "track_unscheduled_pods",
    "UseAPIServerCache": "use_api_server_cache", "ObjectLimit": "object_limit", "AuthFilter": "auth_filter",
}
CLAMPED = {"server_read_timeout", "server_write_timeout", "server_idle_timeout", "server_read_header_timeout", "shutdown_delay"}


def canon(opts, ratio):
    out = {}
    for go, rs in FIELDS.items():
        v = opts[go]
        if rs in ("metric_allowlist", "metric_denylist", "metric_opt_in_list", "resources"):
            v = sorted((v or {}).keys())
        elif rs in ("annotations_allow_list", "labels_allow_list"):
            v = {k: (l or []) for k, l in (v or {}).items()}
        elif rs in ("namespaces", "namespaces_denylist"):
            v = v or []
        elif rs in CLAMPED:
            v = max(v, 0)
        out[rs] = v
    if ratio is not None:
        out["auto_go_memlimit_ratio"] = ratio
    return out


Y = []
def y(yaml, args=(), op="yaml"):
    Y.append({"op": op, "args": list(args), "yaml": yaml})

y("")
y("# just a comment\n")
y("~")
y("foo")
y("[1, 2]")
y("port: 1234\nhost: 0.0.0.0\n")
y("port: '1234'")
y("port: 1.9")
y("port: 0x10\ntelemetry_port: 0o17\ntotal_shards: 3")
y("port: abc\nhost: kept-after-error")
y("port: ~\nhost:\n")
y("port: [1]")
y("shard: 2147483648")
y("shard: -5")
y("object_limit: 9223372036854775807")
y("object_limit: 9223372036854775808")
y("auto-gomemlimit-ratio: 0.5")
y("auto-gomemlimit-ratio: 1")
y("auto-gomemlimit-ratio: '0.5'")
y("auto-gomemlimit-ratio: .inf")
y("auto-gomemlimit-refresh-interval: 1m")
y("auto-gomemlimit-refresh-interval: -1m")
y("auto-gomemlimit-refresh-interval: 60")
y("server_read_timeout: 2m\nserver_write_timeout: 3s\nserver_idle_timeout: 1h\nserver_read_header_timeout: 1ms\nshutdown_delay: 10s")
y("server_read_timeout: abc")
y("server_read_timeout: 0")
y("server_read_timeout: -5s")
y("auth_filter: true\nauto-gomemlimit: yes\ncustom_resources_only: On\nenable_gzip_encoding: Y\nhelp: True\ntrack_unscheduled_pods: TRUE\nuse_api_server_cache: n")
y("auth_filter: 'true'")
y("auth_filter: 'yes'")
y("auth_filter: 1")
y("auth_filter: [true]")
y("continue_without_config: true\ncontinue_without_custom_resource_state_config_file: true")
y("apiserver: https://x\nkubeconfig: /k\nnamespace: ns\nnode: n1\npod: p\ntls_config: /t\ntelemetry_host: 127.0.0.1\nconfig: /c\ncustom_resource_config: 'a: b'")
y("apiserver: 0x10")
y("apiserver: 1.50")
y("apiserver: true")
y("apiserver: {a: b}")
y("custom_resource_state_config_file: /new\ncustom_resource_config_file: /old")
y("custom_resource_config_file: /old")
y("CustomResourceConfigFile: /ignored\nPort: 1")
y("unknown_key: 1\nport: 2")
y("port: 1\nport: 2")
y("resources: {pods: {}, secrets: {}}", ["--resources=configmaps"])
y("resources: {pods: ~, secrets: }", ["--resources=configmaps"])
y("resources: ~", ["--resources=configmaps"])
y("resources: [pods]", ["--resources=configmaps"])
y("resources: {pods: 1}", ["--resources=configmaps"])
y("resources: {pods: {x: 1}}")
y("metric_allowlist: {kube_pod_info: {}}\nmetric_denylist: {kube_pod_labels: {}}\nmetric_opt_in_list: {a: {}}", ["--metric-allowlist=x", "--metric-denylist=y"])
y("namespaces: [a, b]\nnamespaces_denylist: [c]", ["--namespaces=x", "--namespaces-denylist=y"])
y("namespaces: [a, ~, b]")
y("namespaces: ~", ["--namespaces=x"])
y("namespaces: a")
y("namespaces: [[a]]")
y("namespaces: [1, 0x10, true]")
y("labels_allow_list: {pods: [a, b]}", ["--metric-labels-allowlist=nodes=[c],pods=[d]"])
y("labels_allow_list: {pods: ~}", ["--metric-labels-allowlist=nodes=[c]"])
y("labels_allow_list: ~", ["--metric-labels-allowlist=nodes=[c]"])
y("labels_allow_list: {pods: ['a*b*']}\nport: 1")
y("port: 1\nlabels_allow_list: {pods: ['a*b*']}\nhost: h")
y("annotations_allow_list: {pods: ['*']}\nlabels_allow_list: {nodes: ['x*']}")
y("annotations_allow_list: {pods: a}\nport: 3")
y("annotations_allow_list: [a]")
y("annotations_allow_list: {pods: [a, ~, b]}")
y("port: 1\n---\nport: 2\n")
y("port: 1\n  bad: indentation\n")
y("a: &x 5\nport: *x\n")
y("port: 1\n", ["--port=2", "--host=h"])
# configureResourcesAndMetrics
y("", ["--resources=a"], op="configure")
y("resources: {pod: {}, service: {}}\nmetric_allowlist: {kube_pod_info: {}}\nmetric_denylist: {kube_pod_labels: {}}\nmetric_opt_in_list: {kube_pod_status_phase: {}}\nlabels_allow_list: {labelX: [foo, bar]}\nannotations_allow_list: {annotationY: [baz]}\n",
  ["--resources=oldresource", "--metric-allowlist=oldallow", "--metric-denylist=olddeny", "--metric-opt-in-list=oldoptin",
   "--metric-labels-allowlist=oldlabel=[oldvalue]", "--metric-annotations-allowlist=oldannotation=[oldvalue]"], op="configure")
y("resources: {}\nport: 5", ["--resources=a"], op="configure")
y("resources: {b: {}}\nport: abc", ["--resources=a"], op="configure")
y("port: 1", ["--custom-resource-state-config-file=/x"], op="configure")

# Options after flag parsing only.
for args in (
    [], ["--port=0x1F", "--telemetry-port=010", "--total-shards=8_0", "--shard=-0", "--object-limit=+5"],
    ["--resources=pods, secrets,,", "--resources=nodes"], ["--namespaces=a, b", "--namespaces=a"],
    ["--metric-allowlist=a", "--metric-allowlist=b,a", "--metric-denylist= c ", "--metric-opt-in-list=d"],
    ["--metric-labels-allowlist=pods=[a,b],nodes=[]", "--metric-labels-allowlist=deployments=[ x , y ]"],
    ["--metric-annotations-allowlist= pods =[ a ],pods=[b]"], ["--metric-labels-allowlist=pods=[ ]"],
    ["--auto-gomemlimit-ratio=1_0.5e-1", "--auto-gomemlimit-refresh-interval=1.5h", "--server-read-timeout=1h2m3.004005006s",
     "--server-write-timeout=-1s", "--server-idle-timeout=+2m", "--server-read-header-timeout=.5s", "--shutdown-delay=1us"],
    ["--auto-gomemlimit-refresh-interval=-90s"],
    ["--apiserver=a", "--kubeconfig=b", "--host=c", "--telemetry-host=d", "--pod=e", "--pod-namespace=f", "--node=g",
     "--tls-config=h", "--config=i", "--custom-resource-state-config=j", "--custom-resource-state-config-file=k"],
    ["--auth-filter", "--auto-gomemlimit", "--continue-without-config", "--continue-without-custom-resource-state-config-file",
     "--custom-resource-state-only", "--enable-gzip-encoding", "--track-unscheduled-pods", "--use-apiserver-cache"],
):
    y("", args)

H = list(Y)
for s in ["", "0", "-0", "+0", "1", "1s", "1.5h", "-1.5h", "1h2m3s4ms5us6ns", "1µs", "1μs", "1us", ".5s", "5.s", ".s",
          "1.0000000000000000001s", "0.1h", "1e3s", "1x", "1s2", "s", "-", "+", "2562047h47m16.854775807s",
          "2562047h47m16.854775808s", "-2562047h47m16.854775808s", "9223372036854775807ns", "9223372036854775808ns",
          "99999999999999999999ns", "1é", "é", "1h\t", " 1h", "1H", "3.000000001s", "0.000000001s",
          "1.9999999999999999999999h"]:
    H.append({"op": "dur", "s": s})
for s in ["", "0", "-0", "+1", "0x", "0x1F", "0X1f", "0o17", "0O17", "017", "08", "0b101", "0b2", "1_000", "_1", "1_",
          "0x_1F", "0_7", "1__0", "abc", "9223372036854775807", "9223372036854775808", "-9223372036854775808",
          "-9223372036854775809", "2147483647", "2147483648", "99999999999999999999x", "1e3", " 1", "+-1", "١"]:
    H.append({"op": "int", "s": s, "bits": 64})
    H.append({"op": "int", "s": s, "bits": 32})
    H.append({"op": "uint", "s": s})
for s in ["", "0", "1", "-1", "+1.5", "1.", ".5", ".", "1e", "1e+", "1e-5", "1E5", "1e400", "-1e400", "1e-400", "inf", "+Inf",
          "-INFINITY", "infinity", "infin", "nan", "NaN", "+nan", "-nan", "0x1p-2", "0x1.8p1", "0x1", "0x.p1", "0X1P+3",
          "1_000.5", "1__0", "_1", "1_e5", "0x_1p0", "abc", "1.2.3", "0.9", "1e308", "1.7976931348623157e308",
          "1.7976931348623159e308", "4.9e-324", "2.4e-324", "0.1e1_0", "1e_1", "0x1p1_0"]:
    H.append({"op": "float", "s": s})
for s in ["1", "t", "T", "TRUE", "true", "True", "0", "f", "F", "FALSE", "false", "False", "yes", "", "tRUE"]:
    H.append({"op": "bool", "s": s})
for s in ["abc", "a\"b", "a\\b", "\n\t\r\a\b\f\v", "\x00\x01\x7f", "é", " ", "​", "\U0001F600", "﻿", "\U000e0001"]:
    H.append({"op": "quote", "s": s})
for f in [0.0, 0.9, 1.5, -0.5, 0.0078125, 1e-7, 2e10, 123456.1234565, 0.0000005, 2.5e-6, 1e21]:
    H.append({"op": "fmtf", "f": f})
for sels in [[], [""], ["", ""], ["a=b"], ["b=1,a=2"], ["", "spec.nodeName=n"], ["metadata.namespace!=a,metadata.namespace!=b", "spec.nodeName=n"],
             ["metadata.namespace!=z,metadata.namespace!=a", ""], ["a==b"], ["a"], ["a=b=c"], ["a=\\x"], ["a=b\\"], ["a=b\\,c"],
             ["=", "a=b"], ["c=1", "b=2", "a=3"], ["x=1,,y=2"], [",", "a=1"], ["a=1", "bad"], ["é=ü,b!=c"], ["a!=", "=b"]]:
    H.append({"op": "merge", "sels": sels})
for ns, node in [([], ""), (["a"], ""), (["b", "a"], "n"), (["a,b", "c=d", "e\\f"], "x,y=z")]:
    H.append({"op": "selectors", "ns": ns, "node": node})
for s in ["", "pods=[a,b]", "pods=[a,b],nodes=[c]", "pods=[]", "pods=[ ]", " pods =[ a , b ]", "pods=[a,,b]", "pods=[a,]",
          "pods", "pods=[a", "pods=[a],", "pods=[a],,nodes=[b]", "pods=[a]nodes=[b]", "=[a]", "po,ds=[a]", "pods=[a=b]",
          "pods=[a[b]", "pods=[a]b]", "pods=[*]", "pods=[a*b*]", "*=[*]", "pods=[a],pods=[b]", "pods=[a] ,nodes=[b]",
          "pods=[a], nodes=[b]"]:
    H.append({"op": "labels", "s": s})

p = subprocess.run([HARNESS], input="\n".join(json.dumps(h) for h in H) + "\n", capture_output=True, text=True, check=True)
results = []
for line in p.stdout.splitlines():
    r = json.loads(line)
    req = r.pop("req")
    if "opts" in r:
        r["opts"] = canon(r["opts"], r.pop("ratio", None))
        r.pop("nil")
    if req["op"] == "labels" and r["v"] is not None:
        r["v"] = {k: (l or []) for k, l in r["v"].items()}
    results.append({"req": req, "res": r})
with open(os.path.join(OUT, "options_go_cases.json"), "w", encoding="utf-8") as f:
    json.dump(results, f, indent=1, ensure_ascii=False)
    f.write("\n")
print(len(cli), "cli cases,", len(results), "harness cases")
