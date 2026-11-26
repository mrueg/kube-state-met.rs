//! Port of `internal/store/networkpolicy.go`.

use k8s_openapi::api::networking::v1::NetworkPolicy;
use ksm_core::{FamilyGenerator, Metric};

use crate::util::*;

pub const NAME: &str = "networkpolicies";
pub const GO_TYPE: &str = "*v1.NetworkPolicy";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

#[expect(
    clippy::cast_precision_loss,
    reason = "Go converts the slice length to float64 the same way"
)]
fn len_f64(n: usize) -> f64 {
    n as f64
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<NetworkPolicy>> {
    let mut f = Families::new(&["namespace", "networkpolicy"], |o: &NetworkPolicy| {
        vec![ns(&o.metadata), name(&o.metadata)]
    });
    // Go emits the creation timestamp unconditionally, even when it is zero.
    f.add(
        "kube_networkpolicy_created",
        "Unix creation timestamp of network policy",
        Gauge,
        Alpha,
        "",
        |o| {
            vec![Metric::value(time_or_zero_f64(
                o.metadata.creation_timestamp.as_ref(),
            ))]
        },
    )
    .annotations(
        "kube_networkpolicy_annotations",
        Alpha,
        allow_annotations,
        |o| &o.metadata,
    )
    .labels("kube_networkpolicy_labels", Alpha, allow_labels, |o| {
        &o.metadata
    })
    .add(
        "kube_networkpolicy_spec_ingress_rules",
        "Number of ingress rules on the networkpolicy",
        Gauge,
        Alpha,
        "",
        |o| {
            let n = o
                .spec
                .as_ref()
                .and_then(|sp| sp.ingress.as_ref())
                .map_or(0, Vec::len);
            vec![Metric::value(len_f64(n))]
        },
    )
    .add(
        "kube_networkpolicy_spec_egress_rules",
        "Number of egress rules on the networkpolicy",
        Gauge,
        Alpha,
        "",
        |o| {
            let n = o
                .spec
                .as_ref()
                .and_then(|sp| sp.egress.as_ref())
                .map_or(0, Vec::len);
            vec![Metric::value(len_f64(n))]
        },
    );
    f.build()
}
