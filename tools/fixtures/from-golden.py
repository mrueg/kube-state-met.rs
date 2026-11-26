#!/usr/bin/env python3
"""Turns the objects of testdata/golden/stores.jsonl into fake-apiserver fixtures
(testdata/fixtures/golden/objects.json): every store-test object becomes one
object of a cluster, with a unique UID, so a whole-app scrape covers what the
store tests cover."""
import json, os, sys

GO_TYPES = {
    "*v1.CertificateSigningRequest": ("certificates.k8s.io/v1", "CertificateSigningRequest"),
    "*v1.ClusterRole": ("rbac.authorization.k8s.io/v1", "ClusterRole"),
    "*v1.ClusterRoleBinding": ("rbac.authorization.k8s.io/v1", "ClusterRoleBinding"),
    "*v1.Role": ("rbac.authorization.k8s.io/v1", "Role"),
    "*v1.RoleBinding": ("rbac.authorization.k8s.io/v1", "RoleBinding"),
    "*v1.ConfigMap": ("v1", "ConfigMap"), "*v1.Endpoints": ("v1", "Endpoints"),
    "*v1.LimitRange": ("v1", "LimitRange"), "*v1.Namespace": ("v1", "Namespace"),
    "*v1.Node": ("v1", "Node"), "*v1.PersistentVolume": ("v1", "PersistentVolume"),
    "*v1.PersistentVolumeClaim": ("v1", "PersistentVolumeClaim"), "*v1.Pod": ("v1", "Pod"),
    "*v1.ReplicationController": ("v1", "ReplicationController"), "*v1.ResourceQuota": ("v1", "ResourceQuota"),
    "*v1.Secret": ("v1", "Secret"), "*v1.Service": ("v1", "Service"), "*v1.ServiceAccount": ("v1", "ServiceAccount"),
    "*v1.CronJob": ("batch/v1", "CronJob"), "*v1.Job": ("batch/v1", "Job"),
    "*v1.DaemonSet": ("apps/v1", "DaemonSet"), "*v1.Deployment": ("apps/v1", "Deployment"),
    "*v1.ReplicaSet": ("apps/v1", "ReplicaSet"), "*v1.StatefulSet": ("apps/v1", "StatefulSet"),
    "*v1.EndpointSlice": ("discovery.k8s.io/v1", "EndpointSlice"),
    "*v2.HorizontalPodAutoscaler": ("autoscaling/v2", "HorizontalPodAutoscaler"),
    "*v1.Ingress": ("networking.k8s.io/v1", "Ingress"), "*v1.IngressClass": ("networking.k8s.io/v1", "IngressClass"),
    "*v1.NetworkPolicy": ("networking.k8s.io/v1", "NetworkPolicy"),
    "*v1.Lease": ("coordination.k8s.io/v1", "Lease"),
    "*v1.PodDisruptionBudget": ("policy/v1", "PodDisruptionBudget"),
    "*v1.StorageClass": ("storage.k8s.io/v1", "StorageClass"), "*v1.VolumeAttachment": ("storage.k8s.io/v1", "VolumeAttachment"),
    "*v1.VolumeAttributesClass": ("storage.k8s.io/v1", "VolumeAttributesClass"),
    "*v1.MutatingWebhookConfiguration": ("admissionregistration.k8s.io/v1", "MutatingWebhookConfiguration"),
    "*v1.ValidatingWebhookConfiguration": ("admissionregistration.k8s.io/v1", "ValidatingWebhookConfiguration"),
    "*v1.MutatingAdmissionPolicy": ("admissionregistration.k8s.io/v1", "MutatingAdmissionPolicy"),
    "*v1.MutatingAdmissionPolicyBinding": ("admissionregistration.k8s.io/v1", "MutatingAdmissionPolicyBinding"),
    "*v1.ValidatingAdmissionPolicy": ("admissionregistration.k8s.io/v1", "ValidatingAdmissionPolicy"),
    "*v1.ValidatingAdmissionPolicyBinding": ("admissionregistration.k8s.io/v1", "ValidatingAdmissionPolicyBinding"),
}

root = os.path.join(os.path.dirname(__file__), "..", "..")
src = os.path.join(root, "testdata/golden/stores.jsonl")
out_dir = os.path.join(root, "testdata/fixtures/golden")
os.makedirs(out_dir, exist_ok=True)
items, seen, names = [], set(), set()
for n, line in enumerate(open(src), 1):
    r = json.loads(line)
    av, kind = GO_TYPES[r["type"]]
    o = r["object"]
    key = json.dumps(o, sort_keys=True)
    if key in seen:
        continue
    seen.add(key)
    o = dict(o)
    o["apiVersion"], o["kind"] = av, kind
    md = dict(o.get("metadata") or {})
    md.setdefault("name", f"golden-{n}")
    # Names are unique per kind and namespace in a real cluster (client-go keys
    # its WatchList buffer by namespace/name).
    nkey = (kind, md.get("namespace", ""), md["name"])
    if nkey in names:
        md["name"] = f"{md['name']}-g{n}"
    names.add((kind, md.get("namespace", ""), md["name"]))
    md["uid"] = f"golden-{n:04d}-{md.get('uid', '')}"
    o["metadata"] = md
    items.append(o)
json.dump({"apiVersion": "v1", "kind": "List", "items": items}, open(os.path.join(out_dir, "objects.json"), "w"), indent=1)
print(f"{len(items)} objects")
