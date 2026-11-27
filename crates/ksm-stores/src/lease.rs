//! Port of `internal/store/lease.go`.

use k8s_openapi::api::coordination::v1::Lease;
use ksm_core::FamilyGenerator;

use crate::util::*;

pub const NAME: &str = "leases";
pub const GO_TYPE: &str = "*v1.Lease";

pub fn families(
    _allow_annotations: &[String],
    _allow_labels: &[String],
) -> Vec<FamilyGenerator<Lease>> {
    let mut f = Families::new(&["lease"], |o: &Lease| vec![name(&o.metadata)]);
    f.add(
        "kube_lease_owner",
        "Information about the Lease's owner.",
        Gauge,
        Alpha,
        "",
        |l| {
            const KEYS: &[&str] = &["owner_kind", "owner_name", "namespace", "lease_holder"];
            let holder = l
                .spec
                .as_ref()
                .and_then(|sp| sp.holder_identity.as_deref())
                .unwrap_or("");
            let namespace = ns(&l.metadata);
            match l.metadata.owner_references.as_deref() {
                None | Some([]) => vec![mk(
                    KEYS,
                    vec![
                        String::new(),
                        String::new(),
                        namespace.into(),
                        holder.into(),
                    ],
                    1.0,
                )],
                Some(owners) => owners
                    .iter()
                    .map(|o| {
                        mk(
                            KEYS,
                            vec![
                                o.kind.clone(),
                                o.name.clone(),
                                namespace.into(),
                                holder.into(),
                            ],
                            1.0,
                        )
                    })
                    .collect(),
            }
        },
    )
    .add(
        "kube_lease_renew_time",
        "Kube lease renew time.",
        Gauge,
        Alpha,
        "",
        |l| {
            let renew = l.spec.as_ref().and_then(|sp| sp.renew_time.as_ref());
            match renew {
                Some(t) if !micro_time_is_zero(Some(t)) => {
                    vec![m1("namespace", ns(&l.metadata), micro_time_f64(t))]
                }
                _ => Vec::new(),
            }
        },
    );
    f.build()
}
