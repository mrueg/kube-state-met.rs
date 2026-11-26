//! Port of `internal/store/certificatesigningrequest.go`.

use k8s_openapi::api::certificates::v1::CertificateSigningRequest;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "certificatesigningrequests";
pub const GO_TYPE: &str = "*v1.CertificateSigningRequest";
pub const CLUSTER_SCOPED: bool = true;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<CertificateSigningRequest>> {
    let mut f = Families::new(
        &["certificatesigningrequest", "signer_name"],
        |c: &CertificateSigningRequest| vec![name(&c.metadata), c.spec.signer_name.as_str()],
    );
    f.annotations(
        "kube_certificatesigningrequest_annotations",
        Alpha,
        allow_annotations,
        |c| &c.metadata,
    )
    .labels(
        "kube_certificatesigningrequest_labels",
        Stable,
        allow_labels,
        |c| &c.metadata,
    )
    .add(
        "kube_certificatesigningrequest_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |c| created_metric(&c.metadata),
    )
    .add(
        "kube_certificatesigningrequest_condition",
        "The number of each certificatesigningrequest condition",
        Gauge,
        Stable,
        "",
        condition_counts,
    )
    .add(
        "kube_certificatesigningrequest_cert_length",
        "Length of the issued cert",
        Gauge,
        Stable,
        "",
        |c| {
            let len = c
                .status
                .as_ref()
                .and_then(|s| s.certificate.as_ref())
                .map_or(0, |b| b.0.len());
            vec![Metric::value(usize_f64(len))]
        },
    );
    f.build()
}

/// `float64(n)` of a length or count.
#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts int to float64 the same way"
)]
fn usize_f64(n: usize) -> f64 {
    n as f64
}

/// `addCSRConditionMetrics`: the number of Approved, Denied and Failed
/// conditions.
fn condition_counts(c: &CertificateSigningRequest) -> Vec<Metric> {
    let (mut approved, mut denied, mut failed) = (0usize, 0usize, 0usize);
    for cond in c
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .into_iter()
        .flatten()
    {
        match cond.type_.as_str() {
            "Approved" => approved += 1,
            "Denied" => denied += 1,
            "Failed" => failed += 1,
            _ => {}
        }
    }
    vec![
        m1("condition", "approved", usize_f64(approved)),
        m1("condition", "denied", usize_f64(denied)),
        m1("condition", "failed", usize_f64(failed)),
    ]
}
