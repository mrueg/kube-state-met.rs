//! Port of `internal/store/mutatingwebhookconfiguration.go`.

use k8s_openapi::api::admissionregistration::v1::MutatingWebhookConfiguration;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "mutatingwebhookconfigurations";
pub const GO_TYPE: &str = "*v1.MutatingWebhookConfiguration";

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<MutatingWebhookConfiguration>> {
    let mut f = Families::new(
        &["mutatingwebhookconfiguration"],
        |w: &MutatingWebhookConfiguration| vec![name(&w.metadata)],
    );
    f.add(
        "kube_mutatingwebhookconfiguration_info",
        "Information about the MutatingWebhookConfiguration.",
        Gauge,
        Alpha,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_mutatingwebhookconfiguration_created",
        "Unix creation timestamp.",
        Gauge,
        Alpha,
        "",
        |w| created_metric(&w.metadata),
    )
    .add(
        "kube_mutatingwebhookconfiguration_metadata_resource_version",
        "Resource version representing a specific version of the MutatingWebhookConfiguration.",
        Gauge,
        Alpha,
        "",
        |w| rv_metric(&w.metadata),
    )
    .add(
        "kube_mutatingwebhookconfiguration_webhook_clientconfig_service",
        "Service used by the apiserver to connect to a mutating webhook.",
        Gauge,
        Alpha,
        "",
        |w| {
            w.webhooks
                .iter()
                .flatten()
                .map(|wh| {
                    let (svc_name, svc_ns) = match &wh.client_config.service {
                        Some(svc) => (svc.name.clone(), svc.namespace.clone()),
                        None => (String::new(), String::new()),
                    };
                    mk(
                        &["webhook_name", "service_name", "service_namespace"],
                        values![wh.name.as_str(), svc_name, svc_ns],
                        1.0,
                    )
                })
                .collect()
        },
    );
    f.build()
}
