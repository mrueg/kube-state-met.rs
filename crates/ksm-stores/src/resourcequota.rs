//! Port of `internal/store/resourcequota.go`.

use k8s_openapi::api::core::v1::ResourceQuota;
use ksm_core::FamilyGenerator;

use crate::quantity;
use crate::util::*;

pub const NAME: &str = "resourcequotas";
pub const GO_TYPE: &str = "*v1.ResourceQuota";
pub const CLUSTER_SCOPED: bool = false;
/// Whether this module is ported (the front door falls back to Go otherwise).
pub const PORTED: bool = true;

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<ResourceQuota>> {
    let mut f = Families::new(&["namespace", "resourcequota"], |r: &ResourceQuota| {
        vec![ns(&r.metadata), name(&r.metadata)]
    });
    f.add(
        "kube_resourcequota_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |r| created_metric(&r.metadata),
    )
    .add(
        "kube_resourcequota",
        "Information about resource quota.",
        Gauge,
        Stable,
        "",
        |r| {
            let Some(st) = r.status.as_ref() else {
                return Vec::new();
            };
            [(&st.hard, "hard"), (&st.used, "used")]
                .into_iter()
                .flat_map(|(list, ty)| {
                    list.iter().flatten().map(move |(res, q)| {
                        mk(
                            &["resource", "type"],
                            vec![res.clone(), ty.to_owned()],
                            quantity::ksm_f64(q),
                        )
                    })
                })
                .collect()
        },
    )
    .annotations(
        "kube_resourcequota_annotations",
        Alpha,
        allow_annotations,
        |r| &r.metadata,
    )
    .labels("kube_resourcequota_labels", Stable, allow_labels, |r| {
        &r.metadata
    });
    f.build()
}
