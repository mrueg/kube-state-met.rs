//! Port of `internal/store/serviceaccount.go`.

use k8s_openapi::api::core::v1::ServiceAccount;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "serviceaccounts";
pub const GO_TYPE: &str = "*v1.ServiceAccount";

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<ServiceAccount>> {
    let mut f = Families::new(&["namespace", "serviceaccount"], |o: &ServiceAccount| {
        vec![ns(&o.metadata), name(&o.metadata)]
    });
    f.add(
        "kube_serviceaccount_info",
        "Information about a service account",
        Gauge,
        Alpha,
        "",
        |o| {
            vec![mk(
                &["uid", "automount_token"],
                vec![
                    s(&o.metadata.uid).to_owned(),
                    opt_bool_string(o.automount_service_account_token),
                ],
                1.0,
            )]
        },
    )
    .add(
        "kube_serviceaccount_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |o| created_metric(&o.metadata),
    )
    .add(
        "kube_serviceaccount_deleted",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |o| deletion_metric(&o.metadata),
    )
    .add(
        "kube_serviceaccount_secret",
        "Secret being referenced by a service account",
        Gauge,
        Alpha,
        "",
        |o| {
            o.secrets
                .iter()
                .flatten()
                .map(|r| m1("name", s(&r.name), 1.0))
                .collect()
        },
    )
    .add(
        "kube_serviceaccount_image_pull_secret",
        "Secret being referenced by a service account for the purpose of pulling images",
        Gauge,
        Alpha,
        "",
        |o| {
            o.image_pull_secrets
                .iter()
                .flatten()
                .map(|r| m1("name", r.name.as_str(), 1.0))
                .collect()
        },
    )
    .annotations(
        "kube_serviceaccount_annotations",
        Alpha,
        allow_annotations,
        |o| &o.metadata,
    )
    .labels("kube_serviceaccount_labels", Alpha, allow_labels, |o| {
        &o.metadata
    });
    f.build()
}
