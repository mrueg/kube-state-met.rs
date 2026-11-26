//! Port of `internal/store/pod.go`.

use std::collections::{BTreeMap, HashMap, HashSet};

use k8s_openapi::api::core::v1::{
    Container, ContainerState, ContainerStatus, Pod, PodCondition, PodSpec, PodStatus,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use ksm_core::labels::{condition_metrics_keyed, sanitize_label_name};
use ksm_core::{FamilyGenerator, Metric};

use crate::quantity::{ksm_f64, value_f64};
use crate::util::*;

pub const NAME: &str = "pods";
pub const GO_TYPE: &str = "*v1.Pod";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

/// `podStatusReasons`.
const POD_STATUS_REASONS: &[&str] = &[
    "Evicted",
    "NodeAffinity",
    "NodeLost",
    "OutOfcpu",
    "OutOfmemory",
    "PreemptionByScheduler",
    "SchedulingGated",
    "Shutdown",
    "TerminationByKubelet",
    "TopologyAffinityError",
    "UnexpectedAdmissionError",
];

/// `podDisruptionConditionReasons`.
const POD_DISRUPTION_CONDITION_REASONS: &[&str] = &[
    "PreemptionByScheduler",
    "DeletionByTaintManager",
    "EvictionByEvictionAPI",
    "DeletionByPodGC",
    "TerminationByKubelet",
];

/// `constant.UnitCore`, `UnitByte`, `UnitInteger`.
const UNIT_CORE: &str = "core";
const UNIT_BYTE: &str = "byte";
const UNIT_INTEGER: &str = "integer";

static EMPTY_SPEC: std::sync::LazyLock<PodSpec> = std::sync::LazyLock::new(PodSpec::default);
static EMPTY_STATUS: std::sync::LazyLock<PodStatus> = std::sync::LazyLock::new(PodStatus::default);
static EMPTY_STATE: std::sync::LazyLock<ContainerState> =
    std::sync::LazyLock::new(ContainerState::default);

/// `p.Spec` (a Go value: unset means the zero spec).
fn spec(p: &Pod) -> &PodSpec {
    p.spec.as_ref().unwrap_or(&EMPTY_SPEC)
}

/// `p.Status` (a Go value: unset means the zero status).
fn status(p: &Pod) -> &PodStatus {
    p.status.as_ref().unwrap_or(&EMPTY_STATUS)
}

/// `cs.State` (a Go value).
fn state(cs: &ContainerStatus) -> &ContainerState {
    cs.state.as_ref().unwrap_or(&EMPTY_STATE)
}

/// `cs.LastTerminationState` (a Go value).
fn last_state(cs: &ContainerStatus) -> &ContainerState {
    cs.last_state.as_ref().unwrap_or(&EMPTY_STATE)
}

fn container_statuses(p: &Pod) -> &[ContainerStatus] {
    status(p).container_statuses.as_deref().unwrap_or(&[])
}

fn init_container_statuses(p: &Pod) -> &[ContainerStatus] {
    status(p).init_container_statuses.as_deref().unwrap_or(&[])
}

fn init_containers(p: &Pod) -> &[Container] {
    spec(p).init_containers.as_deref().unwrap_or(&[])
}

fn conditions(p: &Pod) -> &[PodCondition] {
    status(p).conditions.as_deref().unwrap_or(&[])
}

/// Which statuses a per-container family reads.
type Statuses = fn(&Pod) -> &[ContainerStatus];

/// `float64(t.Unix())` of the conditions of `type_` with status `True`.
fn condition_times(p: &Pod, type_: &str, status: &str) -> Vec<Metric> {
    conditions(p)
        .iter()
        .filter(|c| c.type_ == type_ && c.status == status)
        .map(|c| Metric::value(time_or_zero_f64(c.last_transition_time.as_ref())))
        .collect()
}

/// The `condition` samples of the conditions of `type_`.
fn condition_status(p: &Pod, type_: &str) -> Vec<Metric> {
    conditions(p)
        .iter()
        .filter(|c| c.type_ == type_)
        .flat_map(|c| condition_metrics_keyed("condition", &c.status))
        .collect()
}

/// One `container` sample per status.
fn per_status(p: &Pod, statuses: Statuses, v: fn(&ContainerStatus) -> f64) -> Vec<Metric> {
    statuses(p)
        .iter()
        .map(|cs| m1("container", cs.name.as_str(), v(cs)))
        .collect()
}

/// `kube_pod_[init_]container_state_started`.
fn state_started(p: &Pod, statuses: Statuses) -> Vec<Metric> {
    let mut ms = Vec::new();
    for cs in statuses(p) {
        let st = state(cs);
        if let Some(r) = &st.running {
            ms.push(m1(
                "container",
                cs.name.as_str(),
                time_or_zero_f64(r.started_at.as_ref()),
            ));
        } else if let Some(t) = &st.terminated
            && !time_is_zero(t.started_at.as_ref())
        {
            ms.push(m1(
                "container",
                cs.name.as_str(),
                time_or_zero_f64(t.started_at.as_ref()),
            ));
        }
    }
    ms
}

/// `..._status_last_terminated_reason`.
fn last_terminated_reason(p: &Pod, statuses: Statuses) -> Vec<Metric> {
    statuses(p)
        .iter()
        .filter_map(|cs| {
            last_state(cs).terminated.as_ref().map(|t| {
                mk(
                    &["container", "reason"],
                    vec![cs.name.clone(), s(&t.reason).to_owned()],
                    1.0,
                )
            })
        })
        .collect()
}

/// `..._status_last_terminated_exitcode`.
fn last_terminated_exit_code(p: &Pod, statuses: Statuses) -> Vec<Metric> {
    statuses(p)
        .iter()
        .filter_map(|cs| {
            last_state(cs)
                .terminated
                .as_ref()
                .map(|t| m1("container", cs.name.as_str(), f64::from(t.exit_code)))
        })
        .collect()
}

/// `..._status_last_terminated_timestamp`.
fn last_terminated_timestamp(p: &Pod, statuses: Statuses) -> Vec<Metric> {
    statuses(p)
        .iter()
        .filter_map(|cs| {
            last_state(cs)
                .terminated
                .as_ref()
                .filter(|t| !time_is_zero(t.finished_at.as_ref()))
                .map(|t| {
                    m1(
                        "container",
                        cs.name.as_str(),
                        time_or_zero_f64(t.finished_at.as_ref()),
                    )
                })
        })
        .collect()
}

/// `..._status_terminated_reason`.
fn terminated_reason(p: &Pod, statuses: Statuses) -> Vec<Metric> {
    statuses(p)
        .iter()
        .filter_map(|cs| {
            state(cs).terminated.as_ref().map(|t| {
                mk(
                    &["container", "reason"],
                    vec![cs.name.clone(), s(&t.reason).to_owned()],
                    1.0,
                )
            })
        })
        .collect()
}

/// `..._status_waiting_reason`.
fn waiting_reason(p: &Pod, statuses: Statuses) -> Vec<Metric> {
    statuses(p)
        .iter()
        .filter_map(|cs| {
            state(cs).waiting.as_ref().map(|w| {
                mk(
                    &["container", "reason"],
                    vec![cs.name.clone(), s(&w.reason).to_owned()],
                    1.0,
                )
            })
        })
        .collect()
}

/// The `(resource, unit, value)` samples of one container's resource list,
/// as the `switch resourceName` of the resource families.
fn resource_samples(list: &BTreeMap<String, Quantity>) -> Vec<(String, &'static str, f64)> {
    let mut out = Vec::new();
    for (name, val) in list {
        let res = || sanitize_label_name(name).into_owned();
        match name.as_str() {
            "cpu" => out.push((res(), UNIT_CORE, ksm_f64(val))),
            "storage" | "ephemeral-storage" | "memory" => {
                out.push((res(), UNIT_BYTE, value_f64(val)));
            }
            _ => {
                if is_huge_page_resource_name(name) {
                    out.push((res(), UNIT_BYTE, value_f64(val)));
                }
                if is_attachable_volume_resource_name(name) {
                    out.push((res(), UNIT_BYTE, value_f64(val)));
                }
                if is_extended_resource_name(name) {
                    out.push((res(), UNIT_INTEGER, value_f64(val)));
                }
            }
        }
    }
    out
}

/// Which list of a container's resources a family reads.
type ResourceList = fn(&Container) -> Option<&BTreeMap<String, Quantity>>;

fn limits(c: &Container) -> Option<&BTreeMap<String, Quantity>> {
    c.resources.as_ref().and_then(|r| r.limits.as_ref())
}

fn requests(c: &Container) -> Option<&BTreeMap<String, Quantity>> {
    c.resources.as_ref().and_then(|r| r.requests.as_ref())
}

/// `kube_pod_container_resource_{limits,requests}`.
fn container_resources(p: &Pod, list: ResourceList) -> Vec<Metric> {
    let node = s(&spec(p).node_name);
    let mut ms = Vec::new();
    for c in &spec(p).containers {
        if let Some(l) = list(c) {
            for (res, unit, v) in resource_samples(l) {
                ms.push(mk(
                    &["container", "node", "resource", "unit"],
                    vec![c.name.clone(), node.to_owned(), res, unit.to_owned()],
                    v,
                ));
            }
        }
    }
    ms
}

/// `kube_pod_init_container_resource_{limits,requests}`.
fn init_container_resources(p: &Pod, list: ResourceList) -> Vec<Metric> {
    let mut ms = Vec::new();
    for c in init_containers(p) {
        if let Some(l) = list(c) {
            for (res, unit, v) in resource_samples(l) {
                ms.push(mk(
                    &["container", "resource", "unit"],
                    vec![c.name.clone(), res, unit.to_owned()],
                    v,
                ));
            }
        }
    }
    ms
}

/// `hasPodStatusReason`.
fn has_pod_status_reason(p: &Pod, reason: &str) -> bool {
    if s(&status(p).reason) == reason {
        return true;
    }
    if conditions(p).iter().any(|c| s(&c.reason) == reason) {
        return true;
    }
    container_statuses(p).iter().any(|cs| {
        state(cs)
            .terminated
            .as_ref()
            .is_some_and(|t| s(&t.reason) == reason)
    })
}

/// The overhead samples of one resource, converted with `conv`.
fn overhead(p: &Pod, resource: &str, conv: fn(&Quantity) -> f64) -> Vec<Metric> {
    spec(p)
        .overhead
        .iter()
        .flatten()
        .filter(|(n, _)| n.as_str() == resource)
        .map(|(_, v)| Metric::value(conv(v)))
        .collect()
}

/// The `volume`, `persistentvolumeclaim`, `ephemeral` samples; `readonly`
/// picks the value of the `_readonly` family instead of 1.
fn pvc_volumes(p: &Pod, readonly: bool) -> Vec<Metric> {
    const KEYS: &[&str] = &["volume", "persistentvolumeclaim", "ephemeral"];
    let mut ms = Vec::new();
    for v in spec(p).volumes.iter().flatten() {
        if let Some(pvc) = &v.persistent_volume_claim {
            let value = if readonly {
                bool_f64(pvc.read_only.unwrap_or(false))
            } else {
                1.0
            };
            ms.push(mk(
                KEYS,
                vec![v.name.clone(), pvc.claim_name.clone(), "false".to_owned()],
                value,
            ));
        } else if v.ephemeral.is_some() {
            // ephemeral.VolumeClaimName: pod name + "-" + volume name.
            let claim = format!("{}-{}", name(&p.metadata), v.name);
            ms.push(mk(
                KEYS,
                vec![v.name.clone(), claim, "true".to_owned()],
                if readonly { 0.0 } else { 1.0 },
            ));
        }
    }
    ms
}

/// One sample per value of `values`, 1 for the one equal to `current`; none
/// when `current` is empty.
fn one_of(key: &'static str, values: &[&str], current: &str) -> Vec<Metric> {
    if current.is_empty() {
        return Vec::new();
    }
    values
        .iter()
        .map(|v| m1(key, *v, bool_f64(*v == current)))
        .collect()
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Pod>> {
    let mut f = Families::new(&["namespace", "pod", "uid"], |p: &Pod| {
        vec![ns(&p.metadata), name(&p.metadata), s(&p.metadata.uid)]
    });
    f.add(
        "kube_pod_completion_time",
        "Completion time in unix timestamp for a pod.",
        Gauge,
        Stable,
        "",
        |p| {
            let mut last_finish_time = 0.0_f64;
            for cs in container_statuses(p) {
                if let Some(t) = &state(cs).terminated {
                    let fin = time_or_zero_f64(t.finished_at.as_ref());
                    if last_finish_time == 0.0 || last_finish_time < fin {
                        last_finish_time = fin;
                    }
                }
            }
            if last_finish_time > 0.0 {
                vec![Metric::value(last_finish_time)]
            } else {
                Vec::new()
            }
        },
    )
    .add(
        "kube_pod_container_info",
        "Information about a container in a pod.",
        Gauge,
        Stable,
        "",
        |p| {
            let mut ms = Vec::new();
            for c in &spec(p).containers {
                for cs in container_statuses(p).iter().filter(|cs| cs.name == c.name) {
                    ms.push(mk(
                        &["container", "image_spec", "image", "image_id", "container_id"],
                        vec![
                            cs.name.clone(),
                            s(&c.image).to_owned(),
                            cs.image.clone(),
                            cs.image_id.clone(),
                            s(&cs.container_id).to_owned(),
                        ],
                        1.0,
                    ));
                }
            }
            ms
        },
    )
    .add(
        "kube_pod_container_resource_limits",
        "The number of requested limit resource by a container. It is recommended to use the kube_pod_resource_limits metric exposed by kube-scheduler instead, as it is more precise.",
        Gauge,
        Stable,
        "",
        |p| container_resources(p, limits),
    )
    .add(
        "kube_pod_container_resource_requests",
        "The number of requested request resource by a container. It is recommended to use the kube_pod_resource_requests metric exposed by kube-scheduler instead, as it is more precise.",
        Gauge,
        Stable,
        "",
        |p| container_resources(p, requests),
    )
    .add(
        "kube_pod_container_state_started",
        "Start time in unix timestamp for a pod container.",
        Gauge,
        Stable,
        "",
        |p| state_started(p, container_statuses),
    )
    .add(
        "kube_pod_container_status_last_terminated_reason",
        "Describes the last reason the container was in terminated state.",
        Gauge,
        Alpha,
        "",
        |p| last_terminated_reason(p, container_statuses),
    )
    .add(
        "kube_pod_container_status_last_terminated_exitcode",
        "Describes the exit code for the last container in terminated state.",
        Gauge,
        Alpha,
        "",
        |p| last_terminated_exit_code(p, container_statuses),
    )
    .add(
        "kube_pod_container_status_last_terminated_timestamp",
        "Last terminated time for a pod container in unix timestamp.",
        Gauge,
        Alpha,
        "",
        |p| last_terminated_timestamp(p, container_statuses),
    )
    .add(
        "kube_pod_container_status_ready",
        "Describes whether the containers readiness check succeeded.",
        Gauge,
        Stable,
        "",
        |p| per_status(p, container_statuses, |cs| bool_f64(cs.ready)),
    )
    .add(
        "kube_pod_container_status_restarts_total",
        "The number of container restarts per container.",
        Counter,
        Stable,
        "",
        |p| per_status(p, container_statuses, |cs| f64::from(cs.restart_count)),
    )
    .add(
        "kube_pod_container_status_running",
        "Describes whether the container is currently in running state.",
        Gauge,
        Stable,
        "",
        |p| per_status(p, container_statuses, |cs| bool_f64(state(cs).running.is_some())),
    )
    .add(
        "kube_pod_container_status_terminated",
        "Describes whether the container is currently in terminated state.",
        Gauge,
        Stable,
        "",
        |p| {
            per_status(p, container_statuses, |cs| {
                bool_f64(state(cs).terminated.is_some())
            })
        },
    )
    .add(
        "kube_pod_container_status_terminated_reason",
        "Describes the reason the container is currently in terminated state.",
        Gauge,
        Alpha,
        "",
        |p| terminated_reason(p, container_statuses),
    )
    .add(
        "kube_pod_container_status_waiting",
        "Describes whether the container is currently in waiting state.",
        Gauge,
        Stable,
        "",
        |p| per_status(p, container_statuses, |cs| bool_f64(state(cs).waiting.is_some())),
    )
    .add(
        "kube_pod_container_status_waiting_reason",
        "Describes the reason the container is currently in waiting state.",
        Gauge,
        Stable,
        "",
        |p| waiting_reason(p, container_statuses),
    )
    .add(
        "kube_pod_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |p| created_metric(&p.metadata),
    )
    .add(
        "kube_pod_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |p| deletion_metric(&p.metadata),
    )
    .add(
        "kube_pod_info",
        "Information about pod.",
        Gauge,
        Stable,
        "",
        |p| {
            // metav1.GetControllerOf: the first owner with controller=true.
            let created_by = p
                .metadata
                .owner_references
                .iter()
                .flatten()
                .find(|o| o.controller == Some(true));
            let (kind, cname) = created_by
                .map(|o| (o.kind.clone(), o.name.clone()))
                .unwrap_or_default();
            let sp = spec(p);
            let st = status(p);
            vec![mk(
                &[
                    "host_ip",
                    "pod_ip",
                    "node",
                    "created_by_kind",
                    "created_by_name",
                    "priority_class",
                    "host_network",
                ],
                vec![
                    s(&st.host_ip).to_owned(),
                    s(&st.pod_ip).to_owned(),
                    s(&sp.node_name).to_owned(),
                    kind,
                    cname,
                    s(&sp.priority_class_name).to_owned(),
                    sp.host_network.unwrap_or(false).to_string(),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_pod_ips",
        "Pod IP addresses",
        Gauge,
        Alpha,
        "",
        |p| {
            status(p)
                .pod_ips
                .iter()
                .flatten()
                .filter_map(|ip| {
                    go_ip_family(&ip.ip)
                        .map(|fam| mk(&["ip", "ip_family"], vec![ip.ip.clone(), fam.to_owned()], 1.0))
                })
                .collect()
        },
    )
    .add(
        "kube_pod_init_container_info",
        "Information about an init container in a pod.",
        Gauge,
        Stable,
        "",
        |p| {
            let mut ms = Vec::new();
            for c in init_containers(p) {
                let restart_policy = s(&c.restart_policy);
                for cs in init_container_statuses(p)
                    .iter()
                    .filter(|cs| cs.name == c.name)
                {
                    ms.push(mk(
                        &[
                            "container",
                            "image_spec",
                            "image",
                            "image_id",
                            "container_id",
                            "restart_policy",
                        ],
                        vec![
                            cs.name.clone(),
                            s(&c.image).to_owned(),
                            cs.image.clone(),
                            cs.image_id.clone(),
                            s(&cs.container_id).to_owned(),
                            restart_policy.to_owned(),
                        ],
                        1.0,
                    ));
                }
            }
            ms
        },
    )
    .add(
        "kube_pod_init_container_resource_limits",
        "The number of requested limit resource by an init container.",
        Gauge,
        Alpha,
        "",
        |p| init_container_resources(p, limits),
    )
    .add(
        "kube_pod_init_container_resource_requests",
        "The number of requested request resource by an init container.",
        Gauge,
        Alpha,
        "",
        |p| init_container_resources(p, requests),
    )
    .add(
        "kube_pod_init_container_state_started",
        "Start time in unix timestamp for a pod init container.",
        Gauge,
        Alpha,
        "",
        |p| state_started(p, init_container_statuses),
    )
    .add(
        "kube_pod_init_container_status_last_terminated_reason",
        "Describes the last reason the init container was in terminated state.",
        Gauge,
        Alpha,
        "",
        |p| last_terminated_reason(p, init_container_statuses),
    )
    .add(
        "kube_pod_init_container_status_last_terminated_exitcode",
        "Describes the exit code for the last init container in terminated state.",
        Gauge,
        Alpha,
        "",
        |p| last_terminated_exit_code(p, init_container_statuses),
    )
    .add(
        "kube_pod_init_container_status_last_terminated_timestamp",
        "Last terminated time for a pod init container in unix timestamp.",
        Gauge,
        Alpha,
        "",
        |p| last_terminated_timestamp(p, init_container_statuses),
    )
    .add(
        "kube_pod_init_container_status_ready",
        "Describes whether the init containers readiness check succeeded.",
        Gauge,
        Stable,
        "",
        |p| per_status(p, init_container_statuses, |cs| bool_f64(cs.ready)),
    )
    .add(
        "kube_pod_init_container_status_restarts_total",
        "The number of restarts for the init container.",
        Counter,
        Stable,
        "",
        |p| per_status(p, init_container_statuses, |cs| f64::from(cs.restart_count)),
    )
    .add(
        "kube_pod_init_container_status_running",
        "Describes whether the init container is currently in running state.",
        Gauge,
        Stable,
        "",
        |p| {
            per_status(p, init_container_statuses, |cs| {
                bool_f64(state(cs).running.is_some())
            })
        },
    )
    .add(
        "kube_pod_init_container_status_terminated",
        "Describes whether the init container is currently in terminated state.",
        Gauge,
        Stable,
        "",
        |p| {
            per_status(p, init_container_statuses, |cs| {
                bool_f64(state(cs).terminated.is_some())
            })
        },
    )
    .add(
        "kube_pod_init_container_status_terminated_reason",
        "Describes the reason the init container is currently in terminated state.",
        Gauge,
        Alpha,
        "",
        |p| terminated_reason(p, init_container_statuses),
    )
    .add(
        "kube_pod_init_container_status_waiting",
        "Describes whether the init container is currently in waiting state.",
        Gauge,
        Stable,
        "",
        |p| {
            per_status(p, init_container_statuses, |cs| {
                bool_f64(state(cs).waiting.is_some())
            })
        },
    )
    .add(
        "kube_pod_init_container_status_waiting_reason",
        "Describes the reason the init container is currently in waiting state.",
        Gauge,
        Alpha,
        "",
        |p| waiting_reason(p, init_container_statuses),
    )
    .annotations("kube_pod_annotations", Alpha, allow_annotations, |p| {
        &p.metadata
    })
    .labels("kube_pod_labels", Stable, allow_labels, |p| &p.metadata)
    .add(
        "kube_pod_overhead_cpu_cores",
        "The pod overhead in regards to cpu cores associated with running a pod.",
        Gauge,
        Alpha,
        "",
        |p| overhead(p, "cpu", ksm_f64),
    )
    .add(
        "kube_pod_overhead_memory_bytes",
        "The pod overhead in regards to memory associated with running a pod.",
        Gauge,
        Alpha,
        "",
        |p| overhead(p, "memory", value_f64),
    )
    .add(
        "kube_pod_owner",
        "Information about the Pod's owner.",
        Gauge,
        Stable,
        "",
        |p| owner_metrics(&p.metadata, true),
    )
    .add(
        "kube_pod_resourceclaim_info",
        "Information about a DRA ResourceClaim referenced by a pod, one series per pod.spec.resourceClaims entry; claim_name is the pod-local reference and resourceclaim_name is the resolved ResourceClaim object name.",
        Gauge,
        Alpha,
        "",
        |p| {
            let claims = spec(p).resource_claims.as_deref().unwrap_or(&[]);
            if claims.is_empty() {
                return Vec::new();
            }
            let resolved: HashMap<&str, &str> = status(p)
                .resource_claim_statuses
                .iter()
                .flatten()
                .filter_map(|st| {
                    st.resource_claim_name
                        .as_deref()
                        .map(|n| (st.name.as_str(), n))
                })
                .collect();
            claims
                .iter()
                .filter(|rc| {
                    rc.resource_claim_name.is_some() || rc.resource_claim_template_name.is_some()
                })
                .map(|rc| {
                    let claim_name = match &rc.resource_claim_name {
                        Some(n) => n.clone(),
                        None => resolved
                            .get(rc.name.as_str())
                            .map(|n| (*n).to_owned())
                            .unwrap_or_default(),
                    };
                    mk(
                        &[
                            "claim_name",
                            "resourceclaim_name",
                            "resourceclaim_template_name",
                        ],
                        vec![
                            rc.name.clone(),
                            claim_name,
                            s(&rc.resource_claim_template_name).to_owned(),
                        ],
                        1.0,
                    )
                })
                .collect()
        },
    )
    .add(
        "kube_pod_restart_policy",
        "Describes the restart policy in use by this pod.",
        Gauge,
        Stable,
        "",
        |p| vec![m1("type", s(&spec(p).restart_policy), 1.0)],
    )
    .add(
        "kube_pod_runtimeclass_name_info",
        "The runtimeclass associated with the pod.",
        Gauge,
        Alpha,
        "",
        |p| {
            spec(p)
                .runtime_class_name
                .iter()
                .map(|n| m1("runtimeclass_name", n.as_str(), 1.0))
                .collect()
        },
    )
    .add(
        "kube_pod_spec_volumes_persistentvolumeclaims_info",
        "Information about persistentvolumeclaim and ephemeral volumes in a pod.",
        Gauge,
        Stable,
        "",
        |p| pvc_volumes(p, false),
    )
    .add(
        "kube_pod_spec_volumes_persistentvolumeclaims_readonly",
        "Describes whether a persistentvolumeclaim is mounted read only. Ephemeral volumes always report 0 since the ephemeral volume source does not support a read-only flag.",
        Gauge,
        Stable,
        "",
        |p| pvc_volumes(p, true),
    )
    .add(
        "kube_pod_start_time",
        "Start time in unix timestamp for a pod.",
        Gauge,
        Stable,
        "",
        |p| {
            status(p)
                .start_time
                .iter()
                .map(|t| Metric::value(time_f64(t)))
                .collect()
        },
    )
    .add(
        "kube_pod_status_phase",
        "The pods current phase.",
        Gauge,
        Stable,
        "",
        |p| {
            one_of(
                "phase",
                &["Pending", "Succeeded", "Failed", "Unknown", "Running"],
                s(&status(p).phase),
            )
        },
    )
    .add(
        "kube_pod_status_qos_class",
        "The pods current qosClass.",
        Gauge,
        Alpha,
        "",
        |p| {
            one_of(
                "qos_class",
                &["BestEffort", "Burstable", "Guaranteed"],
                s(&status(p).qos_class),
            )
        },
    )
    .add(
        "kube_pod_status_ready",
        "Describes whether the pod is ready to serve requests.",
        Gauge,
        Stable,
        "",
        |p| condition_status(p, "Ready"),
    )
    .add(
        "kube_pod_status_ready_time",
        "Readiness achieved time in unix timestamp for a pod.",
        Gauge,
        Alpha,
        "",
        |p| condition_times(p, "Ready", "True"),
    )
    .add(
        "kube_pod_status_initialized_time",
        "Initialized time in unix timestamp for a pod.",
        Gauge,
        Alpha,
        "",
        |p| condition_times(p, "Initialized", "True"),
    )
    .add(
        "kube_pod_status_container_ready_time",
        "Readiness achieved time in unix timestamp for a pod containers.",
        Gauge,
        Alpha,
        "",
        |p| condition_times(p, "ContainersReady", "True"),
    )
    .add(
        "kube_pod_status_reason",
        "The pod status reasons",
        Gauge,
        Alpha,
        "",
        |p| {
            let mut ms: Vec<Metric> = POD_STATUS_REASONS
                .iter()
                .filter(|r| has_pod_status_reason(p, r))
                .map(|r| m1("reason", *r, 1.0))
                .collect();
            let reason = s(&status(p).reason);
            if !reason.is_empty() && !POD_STATUS_REASONS.contains(&reason) {
                ms.push(m1("reason", "Other", 1.0));
            }
            ms
        },
    )
    .add(
        "kube_pod_status_disruption_reason",
        "The pod disruption condition reason",
        Gauge,
        Alpha,
        "",
        |p| {
            conditions(p)
                .iter()
                .filter(|c| c.type_ == "DisruptionTarget")
                .map(|c| {
                    let reason = s(&c.reason);
                    let reason = if POD_DISRUPTION_CONDITION_REASONS.contains(&reason) {
                        reason
                    } else {
                        "Other"
                    };
                    m1("reason", reason, 1.0)
                })
                .collect()
        },
    )
    .add(
        "kube_pod_status_scheduled",
        "Describes the status of the scheduling process for the pod.",
        Gauge,
        Stable,
        "",
        |p| condition_status(p, "PodScheduled"),
    )
    .add(
        "kube_pod_status_scheduled_time",
        "Unix timestamp when pod moved into scheduled status",
        Gauge,
        Stable,
        "",
        |p| condition_times(p, "PodScheduled", "True"),
    )
    .add(
        "kube_pod_status_unschedulable",
        "Describes the unschedulable status for the pod.",
        Gauge,
        Stable,
        "",
        |p| {
            conditions(p)
                .iter()
                .filter(|c| c.type_ == "PodScheduled" && c.status == "False")
                .map(|_| Metric::value(1.0))
                .collect()
        },
    )
    .add(
        "kube_pod_status_unscheduled_time",
        "Unix timestamp when pod moved into unscheduled status",
        Gauge,
        Alpha,
        "",
        |p| condition_times(p, "PodScheduled", "False"),
    )
    .add(
        "kube_pod_tolerations",
        "Information about the pod tolerations",
        Gauge,
        Alpha,
        "",
        |p| {
            // getUniqueTolerations: first occurrence of each identity key.
            let mut seen: HashSet<[String; 5]> = HashSet::new();
            let mut ms = Vec::new();
            for t in spec(p).tolerations.iter().flatten() {
                let values = [
                    s(&t.key).to_owned(),
                    s(&t.operator).to_owned(),
                    s(&t.value).to_owned(),
                    s(&t.effect).to_owned(),
                    t.toleration_seconds
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                ];
                if seen.insert(values.clone()) {
                    ms.push(mk(
                        &["key", "operator", "value", "effect", "toleration_seconds"],
                        values.to_vec(),
                        1.0,
                    ));
                }
            }
            ms
        },
    )
    .opt_in(
        "kube_pod_nodeselectors",
        "Describes the Pod nodeSelectors.",
        Gauge,
        Alpha,
        "",
        |p| {
            let (k, v) =
                map_to_prometheus_labels(spec(p).node_selector.iter().flatten(), "nodeselector");
            vec![Metric::new(k, v, 1.0)]
        },
    )
    .add(
        "kube_pod_service_account",
        "The service account for a pod.",
        Gauge,
        Alpha,
        "",
        |p| {
            vec![m1(
                "service_account",
                s(&spec(p).service_account_name),
                1.0,
            )]
        },
    )
    .add(
        "kube_pod_scheduler",
        "The scheduler for a pod.",
        Gauge,
        Alpha,
        "",
        |p| vec![m1("name", s(&spec(p).scheduler_name), 1.0)],
    );
    f.build()
}
