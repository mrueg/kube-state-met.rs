//! Port of `internal/store/service.go`.

use k8s_openapi::api::core::v1::{Service, ServiceSpec};
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "services";
pub const GO_TYPE: &str = "*v1.Service";

/// A spec string field, `""` when the spec or the field is unset.
fn spec_str(sv: &Service, f: fn(&ServiceSpec) -> &Option<String>) -> String {
    sv.spec
        .as_ref()
        .map(|sp| s(f(sp)).to_owned())
        .unwrap_or_default()
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Service>> {
    let mut f = Families::new(&["namespace", "service", "uid"], |o: &Service| {
        vec![ns(&o.metadata), name(&o.metadata), s(&o.metadata.uid)]
    });
    f.add(
        "kube_service_info",
        "Information about service.",
        Gauge,
        Stable,
        "",
        |sv| {
            vec![mk(
                &[
                    "cluster_ip",
                    "external_name",
                    "load_balancer_ip",
                    "external_traffic_policy",
                ],
                values![
                    spec_str(sv, |sp| &sp.cluster_ip),
                    spec_str(sv, |sp| &sp.external_name),
                    spec_str(sv, |sp| &sp.load_balancer_ip),
                    spec_str(sv, |sp| &sp.external_traffic_policy),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_service_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |sv| created_metric(&sv.metadata),
    )
    .add(
        "kube_service_spec_type",
        "Type about service.",
        Gauge,
        Stable,
        "",
        |sv| vec![m1("type", spec_str(sv, |sp| &sp.type_), 1.0)],
    )
    .annotations("kube_service_annotations", Alpha, allow_annotations, |o| {
        &o.metadata
    })
    .labels("kube_service_labels", Stable, allow_labels, |o| &o.metadata)
    .add(
        "kube_service_spec_external_ip",
        "Service external ips. One series for each ip",
        Gauge,
        Stable,
        "",
        |sv| {
            sv.spec
                .as_ref()
                .and_then(|sp| sp.external_ips.as_ref())
                .into_iter()
                .flatten()
                .map(|ip| m1("external_ip", ip.as_str(), 1.0))
                .collect()
        },
    )
    .add(
        "kube_service_status_load_balancer_ingress",
        "Service load balancer ingress status",
        Gauge,
        Stable,
        "",
        |sv| {
            sv.status
                .as_ref()
                .and_then(|st| st.load_balancer.as_ref())
                .and_then(|lb| lb.ingress.as_ref())
                .into_iter()
                .flatten()
                .map(|i| mk(&["ip", "hostname"], values![s(&i.ip), s(&i.hostname)], 1.0))
                .collect()
        },
    )
    .add(
        "kube_service_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |sv| deletion_metric(&sv.metadata),
    );
    f.build()
}
