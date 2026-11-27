//! Port of `internal/store/node.go`.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{Node, NodeSpec, NodeStatus, NodeSystemInfo};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use ksm_core::labels::sanitize_label_name;
use ksm_core::{FamilyGenerator, Metric};

use crate::quantity;
use crate::util::*;

pub const NAME: &str = "nodes";
pub const GO_TYPE: &str = "*v1.Node";

/// `constant.UnitCore`.
const UNIT_CORE: &str = "core";
/// `constant.UnitByte`.
const UNIT_BYTE: &str = "byte";
/// `constant.UnitInteger`.
const UNIT_INTEGER: &str = "integer";

fn spec(n: &Node) -> Option<&NodeSpec> {
    n.spec.as_ref()
}

fn status(n: &Node) -> Option<&NodeStatus> {
    n.status.as_ref()
}

fn node_info(n: &Node) -> Option<&NodeSystemInfo> {
    status(n).and_then(|st| st.node_info.as_ref())
}

/// The `{resource, unit}` samples of a node's allocatable or capacity list.
fn resource_metrics(list: Option<&BTreeMap<String, Quantity>>) -> Vec<Metric> {
    let mut ms = Vec::new();
    for (resource_name, val) in list.into_iter().flatten() {
        let mut push = |unit: &str| {
            ms.push(mk(
                &["resource", "unit"],
                vec![
                    sanitize_label_name(resource_name).into_owned(),
                    unit.to_owned(),
                ],
                quantity::ksm_f64(val),
            ));
        };
        match resource_name.as_str() {
            "cpu" => push(UNIT_CORE),
            "storage" | "ephemeral-storage" | "memory" => push(UNIT_BYTE),
            "pods" => push(UNIT_INTEGER),
            _ => {
                if is_huge_page_resource_name(resource_name) {
                    push(UNIT_BYTE);
                }
                if is_attachable_volume_resource_name(resource_name) {
                    push(UNIT_BYTE);
                }
                if is_extended_resource_name(resource_name) {
                    push(UNIT_INTEGER);
                }
            }
        }
    }
    ms
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Node>> {
    let mut f = Families::new(&["node"], |n: &Node| vec![name(&n.metadata)]);
    f.annotations("kube_node_annotations", Alpha, allow_annotations, |n| {
        &n.metadata
    })
    .add(
        "kube_node_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |n| created_metric(&n.metadata),
    )
    .add(
        "kube_node_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |n| deletion_metric(&n.metadata),
    )
    .add(
        "kube_node_info",
        "Information about a cluster node.",
        Gauge,
        Stable,
        "",
        |n| {
            let info = node_info(n);
            let info_str =
                |f: fn(&NodeSystemInfo) -> &String| info.map(|i| f(i).clone()).unwrap_or_default();
            // TODO: remove internal_ip in v3, replaced by kube_node_status_addresses
            let mut internal_ip = String::new();
            for a in status(n)
                .and_then(|st| st.addresses.as_ref())
                .into_iter()
                .flatten()
            {
                if a.type_ == "InternalIP" {
                    internal_ip.clone_from(&a.address);
                }
            }
            vec![mk(
                &[
                    "kernel_version",
                    "os_image",
                    "container_runtime_version",
                    "kubelet_version",
                    "kubeproxy_version",
                    "provider_id",
                    "pod_cidr",
                    "system_uuid",
                    "internal_ip",
                ],
                vec![
                    info_str(|i| &i.kernel_version),
                    info_str(|i| &i.os_image),
                    info_str(|i| &i.container_runtime_version),
                    info_str(|i| &i.kubelet_version),
                    "deprecated".to_owned(),
                    spec(n)
                        .map(|sp| s(&sp.provider_id))
                        .unwrap_or("")
                        .to_owned(),
                    spec(n).map(|sp| s(&sp.pod_cidr)).unwrap_or("").to_owned(),
                    info_str(|i| &i.system_uuid),
                    internal_ip,
                ],
                1.0,
            )]
        },
    )
    .labels("kube_node_labels", Stable, allow_labels, |n| &n.metadata)
    .add(
        "kube_node_role",
        "The role of a cluster node.",
        Gauge,
        Alpha,
        "",
        |n| {
            const PREFIX: &str = "node-role.kubernetes.io/";
            n.metadata
                .labels
                .iter()
                .flatten()
                .filter_map(|(lbl, _)| lbl.strip_prefix(PREFIX))
                .map(|role| m1("role", role, 1.0))
                .collect()
        },
    )
    .add(
        "kube_node_spec_pod_cidrs",
        "The pod CIDRs of a cluster node.",
        Gauge,
        Alpha,
        "",
        |n| {
            spec(n)
                .and_then(|sp| sp.pod_cidrs.as_ref())
                .into_iter()
                .flatten()
                .map(|c| m1("pod_cidr", c.as_str(), 1.0))
                .collect()
        },
    )
    .add(
        "kube_node_spec_taint",
        "The taint of a cluster node.",
        Gauge,
        Stable,
        "",
        |n| {
            spec(n)
                .and_then(|sp| sp.taints.as_ref())
                .into_iter()
                .flatten()
                .map(|t| {
                    mk(
                        &["key", "value", "effect"],
                        vec![t.key.clone(), s(&t.value).to_owned(), t.effect.clone()],
                        1.0,
                    )
                })
                .collect()
        },
    )
    .add(
        "kube_node_spec_unschedulable",
        "Whether a node can schedule new pods.",
        Gauge,
        Stable,
        "",
        |n| {
            let unschedulable = spec(n).and_then(|sp| sp.unschedulable).unwrap_or(false);
            vec![Metric::value(bool_f64(unschedulable))]
        },
    )
    .add(
        "kube_node_status_allocatable",
        "The allocatable for different resources of a node that are available for scheduling.",
        Gauge,
        Stable,
        "",
        |n| resource_metrics(status(n).and_then(|st| st.allocatable.as_ref())),
    )
    .add(
        "kube_node_status_capacity",
        "The capacity for different resources of a node.",
        Gauge,
        Stable,
        "",
        |n| resource_metrics(status(n).and_then(|st| st.capacity.as_ref())),
    )
    .add(
        "kube_node_status_condition",
        "The condition of a cluster node.",
        Gauge,
        Stable,
        "",
        |n| {
            status(n)
                .and_then(|st| st.conditions.as_ref())
                .into_iter()
                .flatten()
                .flat_map(|c| conditions_with(&["condition"], &[&c.type_], &c.status))
                .collect()
        },
    )
    .add(
        "kube_node_status_addresses",
        "Node address information.",
        Gauge,
        Alpha,
        "",
        |n| {
            status(n)
                .and_then(|st| st.addresses.as_ref())
                .into_iter()
                .flatten()
                .map(|a| {
                    mk(
                        &["type", "address"],
                        vec![a.type_.clone(), a.address.clone()],
                        1.0,
                    )
                })
                .collect()
        },
    );
    f.build()
}
