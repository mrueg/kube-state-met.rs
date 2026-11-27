//! Metric families of every Kubernetes resource kube-state-metrics exposes
//! (Go: `internal/store`). One module per resource; each exports `NAME`
//! (the `--resources` name), `GO_TYPE` (the `resource` label of the list and
//! watch counters) and `families(allow_annotations, allow_labels)`.

pub mod cron;
pub mod quantity;
pub mod util;

pub mod certificatesigningrequest;
pub mod clusterrole;
pub mod clusterrolebinding;
pub mod configmap;
pub mod cronjob;
pub mod daemonset;
pub mod deployment;
pub mod endpoint;
pub mod endpointslice;
pub mod horizontalpodautoscaler;
pub mod ingress;
pub mod ingressclass;
pub mod job;
pub mod lease;
pub mod limitrange;
pub mod mutatingadmissionpolicy;
pub mod mutatingadmissionpolicybinding;
pub mod mutatingwebhookconfiguration;
pub mod namespace;
pub mod networkpolicy;
pub mod node;
pub mod persistentvolume;
pub mod persistentvolumeclaim;
pub mod pod;
pub mod poddisruptionbudget;
pub mod replicaset;
pub mod replicationcontroller;
pub mod resourcequota;
pub mod role;
pub mod rolebinding;
pub mod secret;
pub mod service;
pub mod serviceaccount;
pub mod statefulset;
pub mod storageclass;
pub mod validatingadmissionpolicy;
pub mod validatingadmissionpolicybinding;
pub mod validatingwebhookconfiguration;
pub mod volumeattachment;
pub mod volumeattributesclass;

/// Calls `$m!` with every resource: `module, type, "goFamiliesFunction";`.
#[macro_export]
macro_rules! for_each_resource {
    ($m:ident) => {
        $m! {
            certificatesigningrequest, k8s_openapi::api::certificates::v1::CertificateSigningRequest, "csrMetricFamilies";
            clusterrole, k8s_openapi::api::rbac::v1::ClusterRole, "clusterRoleMetricFamilies";
            clusterrolebinding, k8s_openapi::api::rbac::v1::ClusterRoleBinding, "clusterRoleBindingMetricFamilies";
            configmap, k8s_openapi::api::core::v1::ConfigMap, "configMapMetricFamilies";
            cronjob, k8s_openapi::api::batch::v1::CronJob, "cronJobMetricFamilies";
            daemonset, k8s_openapi::api::apps::v1::DaemonSet, "daemonSetMetricFamilies";
            deployment, k8s_openapi::api::apps::v1::Deployment, "deploymentMetricFamilies";
            endpoint, k8s_openapi::api::core::v1::Endpoints, "endpointMetricFamilies";
            endpointslice, k8s_openapi::api::discovery::v1::EndpointSlice, "endpointSliceMetricFamilies";
            horizontalpodautoscaler, k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler, "hpaMetricFamilies";
            ingress, k8s_openapi::api::networking::v1::Ingress, "ingressMetricFamilies";
            ingressclass, k8s_openapi::api::networking::v1::IngressClass, "ingressClassMetricFamilies";
            job, k8s_openapi::api::batch::v1::Job, "jobMetricFamilies";
            lease, k8s_openapi::api::coordination::v1::Lease, "leaseMetricFamilies";
            limitrange, k8s_openapi::api::core::v1::LimitRange, "limitRangeMetricFamilies";
            mutatingadmissionpolicy, k8s_openapi::api::admissionregistration::v1::MutatingAdmissionPolicy, "mutatingAdmissionPolicyMetricFamilies";
            mutatingadmissionpolicybinding, k8s_openapi::api::admissionregistration::v1::MutatingAdmissionPolicyBinding, "mutatingAdmissionPolicyBindingMetricFamilies";
            mutatingwebhookconfiguration, k8s_openapi::api::admissionregistration::v1::MutatingWebhookConfiguration, "mutatingWebhookConfigurationMetricFamilies";
            namespace, k8s_openapi::api::core::v1::Namespace, "namespaceMetricFamilies";
            networkpolicy, k8s_openapi::api::networking::v1::NetworkPolicy, "networkPolicyMetricFamilies";
            node, k8s_openapi::api::core::v1::Node, "nodeMetricFamilies";
            persistentvolumeclaim, k8s_openapi::api::core::v1::PersistentVolumeClaim, "persistentVolumeClaimMetricFamilies";
            persistentvolume, k8s_openapi::api::core::v1::PersistentVolume, "persistentVolumeMetricFamilies";
            poddisruptionbudget, k8s_openapi::api::policy::v1::PodDisruptionBudget, "podDisruptionBudgetMetricFamilies";
            pod, k8s_openapi::api::core::v1::Pod, "podMetricFamilies";
            replicaset, k8s_openapi::api::apps::v1::ReplicaSet, "replicaSetMetricFamilies";
            replicationcontroller, k8s_openapi::api::core::v1::ReplicationController, "replicationControllerMetricFamilies";
            resourcequota, k8s_openapi::api::core::v1::ResourceQuota, "resourceQuotaMetricFamilies";
            role, k8s_openapi::api::rbac::v1::Role, "roleMetricFamilies";
            rolebinding, k8s_openapi::api::rbac::v1::RoleBinding, "roleBindingMetricFamilies";
            secret, k8s_openapi::api::core::v1::Secret, "secretMetricFamilies";
            serviceaccount, k8s_openapi::api::core::v1::ServiceAccount, "serviceAccountMetricFamilies";
            service, k8s_openapi::api::core::v1::Service, "serviceMetricFamilies";
            statefulset, k8s_openapi::api::apps::v1::StatefulSet, "statefulSetMetricFamilies";
            storageclass, k8s_openapi::api::storage::v1::StorageClass, "storageClassMetricFamilies";
            validatingadmissionpolicy, k8s_openapi::api::admissionregistration::v1::ValidatingAdmissionPolicy, "validatingAdmissionPolicyMetricFamilies";
            validatingadmissionpolicybinding, k8s_openapi::api::admissionregistration::v1::ValidatingAdmissionPolicyBinding, "validatingAdmissionPolicyBindingMetricFamilies";
            validatingwebhookconfiguration, k8s_openapi::api::admissionregistration::v1::ValidatingWebhookConfiguration, "validatingWebhookConfigurationMetricFamilies";
            volumeattachment, k8s_openapi::api::storage::v1::VolumeAttachment, "volumeAttachmentMetricFamilies";
            volumeattributesclass, k8s_openapi::api::storage::v1::VolumeAttributesClass, "volumeAttributesClassMetricFamilies";
        }
    };
}

/// Names of every built-in resource, sorted.
pub fn resource_names() -> Vec<&'static str> {
    macro_rules! names {
        ($($module:ident, $ty:ty, $gofn:literal;)*) => { vec![$($module::NAME),*] };
    }
    let mut v = for_each_resource!(names);
    v.sort_unstable();
    v
}
