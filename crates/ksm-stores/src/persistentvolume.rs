//! Port of `internal/store/persistentvolume.go`.

use k8s_openapi::api::core::v1::{PersistentVolume, PersistentVolumeSpec};
use ksm_core::{FamilyGenerator, Metric};

use crate::quantity;
use crate::util::*;

pub const NAME: &str = "persistentvolumes";
pub const GO_TYPE: &str = "*v1.PersistentVolume";

fn spec(p: &PersistentVolume) -> Option<&PersistentVolumeSpec> {
    p.spec.as_ref()
}

/// Joins with `,`, skipping the separator while the result is still empty
/// (as the Go loop does).
fn go_join(items: Option<&Vec<String>>) -> String {
    let mut out = String::new();
    for item in items.into_iter().flatten() {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(item);
    }
    out
}

/// The label values of `kube_persistentvolume_info`.
fn info_metric(p: &PersistentVolume) -> Metric {
    let mut gce_pd_disk_name = String::new();
    let mut ebs_volume_id = String::new();
    let mut azure_disk_name = String::new();
    let mut fc_wwids = String::new();
    let mut fc_lun = String::new();
    let mut fc_target_wwns = String::new();
    let mut iscsi_target_portal = String::new();
    let mut iscsi_iqn = String::new();
    let mut iscsi_lun = String::new();
    let mut iscsi_initiator_name = String::new();
    let mut nfs_server = String::new();
    let mut nfs_path = String::new();
    let mut csi_driver = String::new();
    let mut csi_volume_handle = String::new();
    let mut local_fs = String::new();
    let mut local_path = String::new();
    let mut host_path = String::new();
    let mut host_path_type = String::new();

    if let Some(sp) = spec(p) {
        if let Some(gce) = &sp.gce_persistent_disk {
            gce_pd_disk_name.clone_from(&gce.pd_name);
        } else if let Some(ebs) = &sp.aws_elastic_block_store {
            ebs_volume_id.clone_from(&ebs.volume_id);
        } else if let Some(az) = &sp.azure_disk {
            azure_disk_name.clone_from(&az.disk_name);
        } else if let Some(fc) = &sp.fc {
            if let Some(lun) = fc.lun {
                fc_lun = lun.to_string();
            }
            fc_target_wwns = go_join(fc.target_wwns.as_ref());
            fc_wwids = go_join(fc.wwids.as_ref());
        } else if let Some(iscsi) = &sp.iscsi {
            iscsi_target_portal.clone_from(&iscsi.target_portal);
            iscsi_iqn.clone_from(&iscsi.iqn);
            iscsi_lun = iscsi.lun.to_string();
            if let Some(n) = &iscsi.initiator_name {
                iscsi_initiator_name.clone_from(n);
            }
        } else if let Some(nfs) = &sp.nfs {
            nfs_server.clone_from(&nfs.server);
            nfs_path.clone_from(&nfs.path);
        } else if let Some(csi) = &sp.csi {
            csi_driver.clone_from(&csi.driver);
            csi_volume_handle.clone_from(&csi.volume_handle);
        } else if let Some(local) = &sp.local {
            local_path.clone_from(&local.path);
            if let Some(fs) = &local.fs_type {
                local_fs.clone_from(fs);
            }
        } else if let Some(hp) = &sp.host_path {
            host_path.clone_from(&hp.path);
            if let Some(t) = &hp.type_ {
                host_path_type.clone_from(t);
            }
        }
    }

    let spec_str = |f: fn(&PersistentVolumeSpec) -> &Option<String>| {
        spec(p).map(|sp| s(f(sp))).unwrap_or("").to_owned()
    };
    mk(
        &[
            "storageclass",
            "gce_persistent_disk_name",
            "ebs_volume_id",
            "azure_disk_name",
            "fc_wwids",
            "fc_lun",
            "fc_target_wwns",
            "iscsi_target_portal",
            "iscsi_iqn",
            "iscsi_lun",
            "iscsi_initiator_name",
            "nfs_server",
            "nfs_path",
            "csi_driver",
            "csi_volume_handle",
            "local_path",
            "local_fs",
            "host_path",
            "host_path_type",
            "reclaim_policy",
        ],
        vec![
            spec_str(|sp| &sp.storage_class_name),
            gce_pd_disk_name,
            ebs_volume_id,
            azure_disk_name,
            fc_wwids,
            fc_lun,
            fc_target_wwns,
            iscsi_target_portal,
            iscsi_iqn,
            iscsi_lun,
            iscsi_initiator_name,
            nfs_server,
            nfs_path,
            csi_driver,
            csi_volume_handle,
            local_path,
            local_fs,
            host_path,
            host_path_type,
            spec_str(|sp| &sp.persistent_volume_reclaim_policy),
        ],
        1.0,
    )
}

pub fn families(
    allow_annotations: &[String],
    allow_labels: &[String],
) -> Vec<FamilyGenerator<PersistentVolume>> {
    let mut f = Families::new(&["persistentvolume"], |p: &PersistentVolume| {
        vec![name(&p.metadata)]
    });
    f.add(
        "kube_persistentvolume_claim_ref",
        "Information about the Persistent Volume Claim Reference.",
        Gauge,
        Stable,
        "",
        |p| match spec(p).and_then(|sp| sp.claim_ref.as_ref()) {
            None => Vec::new(),
            Some(cr) => vec![mk(
                &["name", "claim_namespace"],
                vec![s(&cr.name).to_owned(), s(&cr.namespace).to_owned()],
                1.0,
            )],
        },
    )
    .annotations(
        "kube_persistentvolume_annotations",
        Alpha,
        allow_annotations,
        |p| &p.metadata,
    )
    .labels("kube_persistentvolume_labels", Stable, allow_labels, |p| {
        &p.metadata
    })
    .add(
        "kube_persistentvolume_status_phase",
        "The phase indicates if a volume is available, bound to a claim, or released by a claim.",
        Gauge,
        Stable,
        "",
        |p| {
            let phase = p.status.as_ref().map(|st| s(&st.phase)).unwrap_or("");
            if phase.is_empty() {
                return Vec::new();
            }
            ["Pending", "Available", "Bound", "Released", "Failed"]
                .iter()
                .map(|ph| m1("phase", *ph, bool_f64(phase == *ph)))
                .collect()
        },
    )
    .add(
        "kube_persistentvolume_info",
        "Information about persistentvolume.",
        Gauge,
        Stable,
        "",
        |p| vec![info_metric(p)],
    )
    .add(
        "kube_persistentvolume_capacity_bytes",
        "Persistentvolume capacity in bytes.",
        Gauge,
        Stable,
        "",
        |p| {
            let v = spec(p)
                .and_then(|sp| sp.capacity.as_ref())
                .and_then(|c| c.get("storage"))
                .map_or(0.0, quantity::value_f64);
            vec![Metric::value(v)]
        },
    )
    .add(
        "kube_persistentvolume_created",
        "Unix creation timestamp",
        Gauge,
        Alpha,
        "",
        |p| created_metric(&p.metadata),
    )
    .add(
        "kube_persistentvolume_deletion_timestamp",
        "Unix deletion timestamp",
        Gauge,
        Alpha,
        "",
        |p| deletion_metric(&p.metadata),
    )
    .opt_in(
        "kube_persistentvolume_csi_attributes",
        "CSI attributes of the Persistent Volume.",
        Gauge,
        Alpha,
        "",
        |p| {
            let Some(csi) = spec(p).and_then(|sp| sp.csi.as_ref()) else {
                return Vec::new();
            };
            let mut csi_mounter = String::new();
            let mut csi_map_options = String::new();
            for (k, v) in csi.volume_attributes.iter().flatten() {
                // storage attributes handled by external CEPH CSI driver
                match k.as_str() {
                    "mapOptions" => csi_map_options.clone_from(v),
                    "mounter" => csi_mounter.clone_from(v),
                    _ => {}
                }
            }
            vec![mk(
                &["csi_mounter", "csi_map_options"],
                vec![csi_mounter, csi_map_options],
                1.0,
            )]
        },
    )
    .add(
        "kube_persistentvolume_volume_mode",
        "Volume Mode information for the PersistentVolume.",
        Gauge,
        Alpha,
        "",
        |p| {
            // Filesystem is the default mode used when volumeMode parameter is omitted.
            let mode = spec(p)
                .and_then(|sp| sp.volume_mode.as_deref())
                .unwrap_or("Filesystem");
            vec![m1("volumemode", mode, 1.0)]
        },
    )
    .add(
        "kube_persistentvolume_access_mode",
        "The access mode(s) specified by the persistent volume.",
        Gauge,
        Alpha,
        "",
        |p| {
            spec(p)
                .and_then(|sp| sp.access_modes.as_ref())
                .into_iter()
                .flatten()
                .map(|am| m1("access_mode", am.as_str(), 1.0))
                .collect()
        },
    );
    f.build()
}
