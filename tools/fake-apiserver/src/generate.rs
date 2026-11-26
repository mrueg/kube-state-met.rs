//! Deterministic synthetic clusters for benchmarks: `--generate pods=10000,nodes=100,...`.
//!
//! Objects look like a busy real cluster (labels, owner references, several
//! containers with requests and limits, conditions, statuses) so that metric
//! generation does realistic work. The same spec and seed always give the
//! same objects, UIDs included.

use serde_json::{Value, json};

/// Deterministic UUID-shaped id for a string.
pub fn uid_for(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x8422_2325_cbf2_9ce4;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        h2 = h2.rotate_left(5) ^ u64::from(b);
        h2 = h2.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        h >> 32,
        (h >> 16) & 0xffff,
        h & 0xffff,
        h2 >> 48,
        h2 & 0xffff_ffff_ffff
    )
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len() as u64) as usize]
    }
}

const BASE_TS: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z

fn ts(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since epoch (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn meta(r: &mut Rng, name: &str, ns: Option<&str>, app: &str) -> Value {
    let mut m = json!({
        "name": name,
        "creationTimestamp": ts(BASE_TS + r.below(30 * 86_400) as i64),
        "labels": {
            "app": app,
            "app.kubernetes.io/name": app,
            "app.kubernetes.io/part-of": "bench",
            "team": r.pick(&["payments", "search", "infra", "data", "web"]),
            "tier": r.pick(&["frontend", "backend", "cache", "db"]),
        },
        "annotations": {
            "deployment.kubernetes.io/revision": r.below(20).to_string(),
            "kubectl.kubernetes.io/last-applied-configuration": "{\"apiVersion\":\"v1\"}",
        },
    });
    if let Some(ns) = ns {
        m["namespace"] = json!(ns);
    }
    m
}

fn pod(r: &mut Rng, i: usize, namespaces: usize, nodes: usize) -> Value {
    let ns = format!("ns-{}", i % namespaces.max(1));
    let app = format!("app-{}", i % 97);
    let name = format!("{app}-{:05}-{:x}", i, r.below(0xfffff));
    let node = format!("node-{}", r.below(nodes.max(1) as u64));
    let containers: Vec<Value> = (0..1 + r.below(3))
        .map(|c| {
            json!({
                "name": format!("c{c}"),
                "image": format!("registry.example.com/{app}:v1.{}", r.below(30)),
                "resources": {
                    "requests": {"cpu": format!("{}m", 50 + r.below(950)), "memory": format!("{}Mi", 64 + r.below(2000))},
                    "limits": {"cpu": format!("{}", 1 + r.below(4)), "memory": format!("{}Gi", 1 + r.below(8))},
                },
                "ports": [{"containerPort": 8080, "protocol": "TCP", "name": "http"}],
            })
        })
        .collect();
    let started = BASE_TS + 40 * 86_400 + r.below(86_400) as i64;
    let statuses: Vec<Value> = containers
        .iter()
        .map(|c| {
            json!({
                "name": c["name"], "image": c["image"], "imageID": format!("sha256:{:064x}", r.next()),
                "containerID": format!("containerd://{:064x}", r.next()),
                "ready": true, "started": true, "restartCount": r.below(5),
                "state": {"running": {"startedAt": ts(started)}},
            })
        })
        .collect();
    let mut m = meta(r, &name, Some(&ns), &app);
    m["ownerReferences"] = json!([{"apiVersion": "apps/v1", "kind": "ReplicaSet", "name": format!("{app}-rs"),
        "uid": uid_for(&format!("rs-{ns}-{app}")), "controller": true, "blockOwnerDeletion": true}]);
    json!({
        "apiVersion": "v1", "kind": "Pod", "metadata": m,
        "spec": {"nodeName": node, "containers": containers, "restartPolicy": "Always",
            "schedulerName": "default-scheduler", "priorityClassName": "default", "serviceAccountName": "default",
            "tolerations": [{"key": "node.kubernetes.io/not-ready", "operator": "Exists", "effect": "NoExecute", "tolerationSeconds": 300}]},
        "status": {"phase": "Running", "qosClass": "Burstable", "hostIP": "10.0.0.1", "podIP": format!("10.1.{}.{}", i / 250 % 250, i % 250),
            "podIPs": [{"ip": format!("10.1.{}.{}", i / 250 % 250, i % 250)}],
            "startTime": ts(started),
            "conditions": [
                {"type": "Initialized", "status": "True", "lastTransitionTime": ts(started)},
                {"type": "Ready", "status": "True", "lastTransitionTime": ts(started + 5)},
                {"type": "ContainersReady", "status": "True", "lastTransitionTime": ts(started + 5)},
                {"type": "PodScheduled", "status": "True", "lastTransitionTime": ts(started - 1)},
            ],
            "containerStatuses": statuses},
    })
}

fn node(r: &mut Rng, i: usize) -> Value {
    let name = format!("node-{i}");
    let mut m = meta(r, &name, None, "node");
    m["labels"]["kubernetes.io/hostname"] = json!(name);
    m["labels"]["node.kubernetes.io/instance-type"] = json!(r.pick(&["m5.xlarge", "m5.2xlarge", "c6i.4xlarge"]));
    let cap = json!({"cpu": format!("{}", 4 * (1 + r.below(8))), "memory": format!("{}Ki", 16_000_000 + r.below(50_000_000)),
        "pods": "110", "ephemeral-storage": "100Gi", "hugepages-2Mi": "0"});
    json!({
        "apiVersion": "v1", "kind": "Node", "metadata": m,
        "spec": {"podCIDR": format!("10.1.{i}.0/24"), "providerID": format!("aws:///eu-west-1a/i-{:016x}", r.next())},
        "status": {"capacity": cap, "allocatable": cap,
            "conditions": [
                {"type": "Ready", "status": "True", "lastHeartbeatTime": ts(BASE_TS), "lastTransitionTime": ts(BASE_TS)},
                {"type": "MemoryPressure", "status": "False", "lastHeartbeatTime": ts(BASE_TS), "lastTransitionTime": ts(BASE_TS)},
                {"type": "DiskPressure", "status": "False", "lastHeartbeatTime": ts(BASE_TS), "lastTransitionTime": ts(BASE_TS)},
                {"type": "PIDPressure", "status": "False", "lastHeartbeatTime": ts(BASE_TS), "lastTransitionTime": ts(BASE_TS)},
            ],
            "addresses": [{"type": "InternalIP", "address": format!("10.0.{}.{}", i / 250, i % 250)}, {"type": "Hostname", "address": name}],
            "nodeInfo": {"machineID": "m", "systemUUID": "s", "bootID": "b", "kernelVersion": "6.1.0", "osImage": "Linux",
                "containerRuntimeVersion": "containerd://2.0.0", "kubeletVersion": "v1.37.0", "kubeProxyVersion": "v1.37.0",
                "operatingSystem": "linux", "architecture": "amd64"}},
    })
}

fn deployment(r: &mut Rng, i: usize, namespaces: usize) -> Value {
    let ns = format!("ns-{}", i % namespaces.max(1));
    let app = format!("app-{}", i % 97);
    let replicas = 1 + r.below(10);
    json!({
        "apiVersion": "apps/v1", "kind": "Deployment", "metadata": meta(r, &format!("{app}-{i}"), Some(&ns), &app),
        "spec": {"replicas": replicas, "selector": {"matchLabels": {"app": app}},
            "strategy": {"type": "RollingUpdate", "rollingUpdate": {"maxSurge": "25%", "maxUnavailable": 1}},
            "template": {"metadata": {"labels": {"app": app}}, "spec": {"containers": [{"name": "c", "image": "x"}]}}},
        "status": {"observedGeneration": 3, "replicas": replicas, "readyReplicas": replicas, "availableReplicas": replicas,
            "updatedReplicas": replicas,
            "conditions": [{"type": "Available", "status": "True"}, {"type": "Progressing", "status": "True"}]},
    })
}

fn replicaset(r: &mut Rng, i: usize, namespaces: usize) -> Value {
    let ns = format!("ns-{}", i % namespaces.max(1));
    let app = format!("app-{}", i % 97);
    let replicas = r.below(10);
    let suffix = r.below(0xffff);
    let mut m = meta(r, &format!("{app}-{i}-{suffix:x}"), Some(&ns), &app);
    m["ownerReferences"] = json!([{"apiVersion": "apps/v1", "kind": "Deployment", "name": format!("{app}-{i}"),
        "uid": uid_for(&format!("deploy-{i}")), "controller": true}]);
    json!({"apiVersion": "apps/v1", "kind": "ReplicaSet", "metadata": m,
        "spec": {"replicas": replicas, "selector": {"matchLabels": {"app": app}}},
        "status": {"replicas": replicas, "readyReplicas": replicas, "availableReplicas": replicas, "fullyLabeledReplicas": replicas, "observedGeneration": 2}})
}

fn service(r: &mut Rng, i: usize, namespaces: usize) -> Value {
    let ns = format!("ns-{}", i % namespaces.max(1));
    let app = format!("app-{}", i % 97);
    json!({"apiVersion": "v1", "kind": "Service", "metadata": meta(r, &format!("{app}-svc-{i}"), Some(&ns), &app),
        "spec": {"type": r.pick(&["ClusterIP", "ClusterIP", "NodePort", "LoadBalancer"]), "clusterIP": format!("10.96.{}.{}", i / 250 % 250, i % 250),
            "ports": [{"port": 80, "targetPort": 8080, "protocol": "TCP"}], "selector": {"app": app}},
        "status": {"loadBalancer": {}}})
}

fn configmap(r: &mut Rng, i: usize, namespaces: usize) -> Value {
    let ns = format!("ns-{}", i % namespaces.max(1));
    json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": meta(r, &format!("cm-{i}"), Some(&ns), "cm"), "data": {"k": "v"}})
}

fn secret(r: &mut Rng, i: usize, namespaces: usize) -> Value {
    let ns = format!("ns-{}", i % namespaces.max(1));
    json!({"apiVersion": "v1", "kind": "Secret", "metadata": meta(r, &format!("secret-{i}"), Some(&ns), "secret"), "type": "Opaque"})
}

fn namespace(r: &mut Rng, i: usize) -> Value {
    json!({"apiVersion": "v1", "kind": "Namespace", "metadata": meta(r, &format!("ns-{i}"), None, "ns"), "status": {"phase": "Active"}})
}

fn job(r: &mut Rng, i: usize, namespaces: usize) -> Value {
    let ns = format!("ns-{}", i % namespaces.max(1));
    let start = BASE_TS + r.below(86_400) as i64;
    json!({"apiVersion": "batch/v1", "kind": "Job", "metadata": meta(r, &format!("job-{i}"), Some(&ns), "job"),
        "spec": {"parallelism": 1, "completions": 1, "backoffLimit": 6},
        "status": {"succeeded": 1, "startTime": ts(start), "completionTime": ts(start + 60),
            "conditions": [{"type": "Complete", "status": "True", "lastTransitionTime": ts(start + 60)}]}})
}

/// Objects for a spec like `pods=10000,nodes=100`. Namespaces default to
/// one per 100 pods (at least 1) unless `namespaces=` is given.
pub fn cluster(spec: &str, seed: u64) -> Result<Vec<Value>, String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for part in spec.split(',').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').ok_or_else(|| format!("bad --generate part {part:?}"))?;
        let n: usize = v.parse().map_err(|_| format!("bad count in {part:?}"))?;
        counts.push((k.to_owned(), n));
    }
    let get = |k: &str| counts.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
    let pods = get("pods").unwrap_or(0);
    let namespaces = get("namespaces").unwrap_or((pods / 100).max(1));
    let nodes = get("nodes").unwrap_or((pods / 50).max(1));
    let mut r = Rng(seed);
    let mut out = Vec::new();
    for i in 0..namespaces {
        out.push(namespace(&mut r, i));
    }
    for i in 0..nodes {
        out.push(node(&mut r, i));
    }
    for (kind, n) in &counts {
        for i in 0..*n {
            let o = match kind.as_str() {
                "pods" => pod(&mut r, i, namespaces, nodes),
                "deployments" => deployment(&mut r, i, namespaces),
                "replicasets" => replicaset(&mut r, i, namespaces),
                "services" => service(&mut r, i, namespaces),
                "configmaps" => configmap(&mut r, i, namespaces),
                "secrets" => secret(&mut r, i, namespaces),
                "jobs" => job(&mut r, i, namespaces),
                "namespaces" | "nodes" => break,
                other => return Err(format!("cannot generate {other}")),
            };
            out.push(o);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let a = cluster("pods=50,deployments=5", 7).unwrap();
        let b = cluster("pods=50,deployments=5", 7).unwrap();
        assert_eq!(a, b);
        assert_eq!(ts(BASE_TS), "2026-01-01T00:00:00Z");
        assert_eq!(ts(0), "1970-01-01T00:00:00Z");
    }
}
