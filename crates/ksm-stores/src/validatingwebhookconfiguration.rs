//! Port of `internal/store/validatingwebhookconfiguration.go`.

use k8s_openapi::api::admissionregistration::v1::ValidatingWebhookConfiguration;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "validatingwebhookconfigurations";
pub const GO_TYPE: &str = "*v1.ValidatingWebhookConfiguration";

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<ValidatingWebhookConfiguration>> {
    let mut f = Families::new(
        &["validatingwebhookconfiguration"],
        |w: &ValidatingWebhookConfiguration| vec![name(&w.metadata)],
    );
    f.add(
        "kube_validatingwebhookconfiguration_info",
        "Information about the ValidatingWebhookConfiguration.",
        Gauge,
        Alpha,
        "",
        |_| vec![Metric::value(1.0)],
    )
    .add(
        "kube_validatingwebhookconfiguration_created",
        "Unix creation timestamp.",
        Gauge,
        Alpha,
        "",
        |w| created_metric(&w.metadata),
    )
    .add(
        "kube_validatingwebhookconfiguration_metadata_resource_version",
        "Resource version representing a specific version of the ValidatingWebhookConfiguration.",
        Gauge,
        Alpha,
        "",
        |w| rv_metric(&w.metadata),
    )
    .add(
        "kube_validatingwebhookconfiguration_webhook_clientconfig_service",
        "Service used by the apiserver to connect to a validating webhook.",
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
