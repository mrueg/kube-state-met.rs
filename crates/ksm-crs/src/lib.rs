//! Custom resource state metrics (Go: `pkg/customresourcestate`): the
//! `CustomResourceStateMetrics` configuration and the metric families it
//! generates from unstructured custom resources.
//!
//! This crate is pure: it never talks to the API server. The binary decodes
//! the configuration with [`from_config`], resolves each resource's GVK
//! through CRD discovery ([`Resource::resolved`]; `*` versions and kinds are
//! allowed there), and builds the family generators with
//! [`Resource::families`]. Objects are passed as the whole unstructured
//! object, a `serde_json::Value`.

mod config;
pub mod flect;
mod flect_data;
use ksm_core::gostr;
mod path;
mod registry;
mod value;
mod yamlgo;

pub use config::{
    Generator, GroupVersionKind, Labels, Metric, MetricGauge, MetricInfo, MetricMeta,
    MetricStateSet, Metrics, MetricsSpec, Resource, ResourceConfig, decode_metrics, from_config,
};
pub use registry::{GVK_GROUP_LABEL, GVK_KIND_LABEL, GVK_VERSION_LABEL};
pub use value::{go_fmt_v, to_float64};
