//! Port of `internal/store/endpoint.go`.

use k8s_openapi::api::core::v1::Endpoints;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "endpoints";
pub const GO_TYPE: &str = "*v1.Endpoints";

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Endpoints>> {
    let mut f = Families::new(&["namespace", "endpoint"], |o: &Endpoints| {
        vec![ns(&o.metadata), name(&o.metadata)]
    });
    f.add(
        "kube_endpoint_info",
        "Information about endpoint.",
        Gauge,
        Stable,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_endpoint_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |e| created_metric(&e.metadata),
    )
    .annotations("kube_endpoint_annotations", Alpha, allow_annotations, |o| {
        &o.metadata
    })
    .labels("kube_endpoint_labels", Stable, allow_labels, |o| {
        &o.metadata
    })
    .add(
        "kube_endpoint_address",
        "Information about Endpoint available and non available addresses.",
        Gauge,
        Stable,
        "",
        |e| {
            const KEYS: &[&str] = &["port_protocol", "port_number", "port_name", "ip", "ready"];
            let mut ms = Vec::new();
            for sub in e.subsets.iter().flatten() {
                for port in sub.ports.iter().flatten() {
                    let protocol = s(&port.protocol);
                    let number = port.port.to_string();
                    let pname = s(&port.name);
                    for a in sub.addresses.iter().flatten() {
                        ms.push(mk(
                            KEYS,
                            vec![
                                protocol.to_owned(),
                                number.clone(),
                                pname.to_owned(),
                                a.ip.clone(),
                                "true".to_owned(),
                            ],
                            1.0,
                        ));
                    }
                    for a in sub.not_ready_addresses.iter().flatten() {
                        ms.push(mk(
                            KEYS,
                            vec![
                                protocol.to_owned(),
                                number.clone(),
                                pname.to_owned(),
                                a.ip.clone(),
                                "false".to_owned(),
                            ],
                            1.0,
                        ));
                    }
                }
            }
            ms
        },
    )
    .add(
        "kube_endpoint_ports",
        "Information about the Endpoint ports.",
        Gauge,
        Stable,
        "v2.14.0",
        |e| {
            e.subsets
                .iter()
                .flatten()
                .flat_map(|sub| sub.ports.iter().flatten())
                .map(|port| {
                    mk(
                        &["port_name", "port_protocol", "port_number"],
                        vec![
                            s(&port.name).to_owned(),
                            s(&port.protocol).to_owned(),
                            port.port.to_string(),
                        ],
                        1.0,
                    )
                })
                .collect()
        },
    );
    f.build()
}
