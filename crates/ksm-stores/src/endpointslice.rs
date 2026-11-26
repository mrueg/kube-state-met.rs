//! Port of `internal/store/endpointslice.go`.

use k8s_openapi::api::discovery::v1::EndpointSlice;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "endpointslices";
pub const GO_TYPE: &str = "*v1.EndpointSlice";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<EndpointSlice>> {
    let mut f = Families::new(&["endpointslice", "namespace"], |o: &EndpointSlice| {
        vec![name(&o.metadata), ns(&o.metadata)]
    });
    f.add(
        "kube_endpointslice_info",
        "Information about endpointslice.",
        Gauge,
        Alpha,
        "",
        |e| vec![m1("addresstype", e.address_type.as_str(), 1.0)],
    )
    .add(
        "kube_endpointslice_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |e| created_metric(&e.metadata),
    )
    .add(
        "kube_endpointslice_endpoints_hints",
        "Topology routing hints attached to endpoints",
        Gauge,
        Alpha,
        "",
        |e| {
            let mut ms = Vec::new();
            for ep in e.endpoints.iter().flatten() {
                let zones = match ep.hints.as_ref().and_then(|h| h.for_zones.as_ref()) {
                    Some(z) if !z.is_empty() => z,
                    _ => continue,
                };
                let Some(address) = ep.addresses.first() else {
                    continue;
                };
                for zone in zones {
                    ms.push(mk(
                        &["address", "for_zone"],
                        vec![address.clone(), zone.name.clone()],
                        1.0,
                    ));
                }
            }
            ms
        },
    )
    .add(
        "kube_endpointslice_endpoints",
        "Endpoints attached to the endpointslice.",
        Gauge,
        Alpha,
        "",
        |e| {
            const KEYS: &[&str] = &[
                "ready",
                "serving",
                "hostname",
                "terminating",
                "targetref_kind",
                "targetref_name",
                "targetref_namespace",
                "endpoint_nodename",
                "endpoint_zone",
                "address",
            ];
            let mut ms = Vec::new();
            for ep in e.endpoints.iter().flatten() {
                let cond = ep.conditions.as_ref();
                let tr = ep.target_ref.as_ref();
                let base = [
                    opt_bool_string(cond.and_then(|c| c.ready)),
                    opt_bool_string(cond.and_then(|c| c.serving)),
                    s(&ep.hostname).to_owned(),
                    opt_bool_string(cond.and_then(|c| c.terminating)),
                    tr.map(|t| s(&t.kind).to_owned()).unwrap_or_default(),
                    tr.map(|t| s(&t.name).to_owned()).unwrap_or_default(),
                    tr.map(|t| s(&t.namespace).to_owned()).unwrap_or_default(),
                    s(&ep.node_name).to_owned(),
                    s(&ep.zone).to_owned(),
                ];
                for address in &ep.addresses {
                    let mut v = base.to_vec();
                    v.push(address.clone());
                    ms.push(mk(KEYS, v, 1.0));
                }
            }
            ms
        },
    )
    .add(
        "kube_endpointslice_ports",
        "Ports attached to the endpointslice.",
        Gauge,
        Alpha,
        "",
        |e| {
            e.ports
                .iter()
                .flatten()
                .map(|p| {
                    mk(
                        &["port_name", "port_protocol", "port_number"],
                        vec![
                            s(&p.name).to_owned(),
                            s(&p.protocol).to_owned(),
                            p.port.map(|n| n.to_string()).unwrap_or_default(),
                        ],
                        1.0,
                    )
                })
                .collect()
        },
    )
    .annotations(
        "kube_endpointslice_annotations",
        Alpha,
        allow_annotations,
        |o| &o.metadata,
    )
    .labels("kube_endpointslice_labels", Alpha, allow_labels, |o| {
        &o.metadata
    });
    f.build()
}
