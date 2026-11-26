//! Port of `internal/store/ingress.go`.

use k8s_openapi::api::networking::v1::Ingress;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "ingresses";
pub const GO_TYPE: &str = "*v1.Ingress";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<Ingress>> {
    let mut f = Families::new(&["namespace", "ingress"], |o: &Ingress| {
        vec![ns(&o.metadata), name(&o.metadata)]
    });
    f.add(
        "kube_ingress_info",
        "Information about ingress.",
        Gauge,
        Stable,
        "",
        |i| {
            let mut class = i
                .spec
                .as_ref()
                .and_then(|sp| sp.ingress_class_name.as_deref())
                .unwrap_or("_default");
            if let Some(c) = i
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("kubernetes.io/ingress.class"))
            {
                class = c;
            }
            vec![m1("ingressclass", class, 1.0)]
        },
    )
    .annotations("kube_ingress_annotations", Alpha, allow_annotations, |o| {
        &o.metadata
    })
    .labels("kube_ingress_labels", Stable, allow_labels, |o| &o.metadata)
    .add(
        "kube_ingress_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |i| created_metric(&i.metadata),
    )
    .add(
        "kube_ingress_metadata_resource_version",
        "Resource version representing a specific version of ingress.",
        Gauge,
        Alpha,
        "",
        |i| rv_metric(&i.metadata),
    )
    .add(
        "kube_ingress_path",
        "Ingress host, paths and backend service information.",
        Gauge,
        Stable,
        "",
        |i| {
            let mut ms = Vec::new();
            let rules = i.spec.as_ref().and_then(|sp| sp.rules.as_ref());
            for rule in rules.into_iter().flatten() {
                let Some(http) = rule.http.as_ref() else {
                    continue;
                };
                let host = s(&rule.host);
                for path in &http.paths {
                    let p = s(&path.path);
                    let path_type = path.path_type.as_str();
                    if let Some(svc) = path.backend.service.as_ref() {
                        let port = svc.port.as_ref().and_then(|p| p.number).unwrap_or(0);
                        ms.push(mk(
                            &["host", "path", "path_type", "service_name", "service_port"],
                            vec![
                                host.to_owned(),
                                p.to_owned(),
                                path_type.to_owned(),
                                svc.name.clone(),
                                port.to_string(),
                            ],
                            1.0,
                        ));
                    } else {
                        // Go dereferences Backend.Resource here (it panics when
                        // unset); an unset resource reads as empty strings.
                        let res = path.backend.resource.as_ref();
                        ms.push(mk(
                            &[
                                "host",
                                "path",
                                "path_type",
                                "resource_api_group",
                                "resource_kind",
                                "resource_name",
                            ],
                            vec![
                                host.to_owned(),
                                p.to_owned(),
                                path_type.to_owned(),
                                res.map(|r| s(&r.api_group).to_owned()).unwrap_or_default(),
                                res.map(|r| r.kind.clone()).unwrap_or_default(),
                                res.map(|r| r.name.clone()).unwrap_or_default(),
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
        "kube_ingress_tls",
        "Ingress TLS host and secret information.",
        Gauge,
        Stable,
        "",
        |i| {
            let mut ms = Vec::new();
            let tls = i.spec.as_ref().and_then(|sp| sp.tls.as_ref());
            for t in tls.into_iter().flatten() {
                for host in t.hosts.iter().flatten() {
                    ms.push(mk(
                        &["tls_host", "secret"],
                        vec![host.clone(), s(&t.secret_name).to_owned()],
                        1.0,
                    ));
                }
            }
            ms
        },
    );
    f.build()
}
