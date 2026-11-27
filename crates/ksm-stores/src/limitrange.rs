//! Port of `internal/store/limitrange.go`.

use k8s_openapi::api::core::v1::LimitRange;
use ksm_core::FamilyGenerator;

use crate::quantity;
use crate::util::*;

pub const NAME: &str = "limitranges";
pub const GO_TYPE: &str = "*v1.LimitRange";

/// Go's `limitRangeMetricFamilies` is a fixed list: it has no annotation or
/// label families, so the allow lists are unused.
pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<LimitRange>> {
    let mut f = Families::new(&["namespace", "limitrange"], |r: &LimitRange| {
        vec![ns(&r.metadata), name(&r.metadata)]
    });
    f.add(
        "kube_limitrange",
        "Information about limit range.",
        Gauge,
        Stable,
        "",
        |r| {
            let mut ms = Vec::new();
            for item in r.spec.iter().flat_map(|sp| &sp.limits) {
                let constraints = [
                    (&item.min, "min"),
                    (&item.max, "max"),
                    (&item.default, "default"),
                    (&item.default_request, "defaultRequest"),
                    (&item.max_limit_request_ratio, "maxLimitRequestRatio"),
                ];
                for (list, constraint) in constraints {
                    for (resource, q) in list.iter().flatten() {
                        ms.push(mk(
                            &["resource", "type", "constraint"],
                            vec![resource.clone(), item.type_.clone(), constraint.to_owned()],
                            quantity::ksm_f64(q),
                        ));
                    }
                }
            }
            ms
        },
    )
    .add(
        "kube_limitrange_created",
        "Unix creation timestamp",
        Gauge,
        Stable,
        "",
        |r| created_metric(&r.metadata),
    );
    f.build()
}
