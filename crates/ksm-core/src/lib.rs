//! Core of kube-state-met.rs, a Rust port of kube-state-metrics: the metric
//! text format, family generators, metric stores and writers, allow/deny and
//! opt-in filters, sharding and label helpers. Nothing here talks to the
//! Kubernetes API; see the `kube-state-metrics` binary crate for that.

pub mod ecma;
pub mod filter;
pub mod generator;
pub mod gostd;
pub mod gostr;
pub mod labels;
pub mod log;
pub mod metric;
pub mod re2;
pub mod sharding;
pub mod store;

pub use generator::{FamilyGenerator, Stability};
pub use metric::{Family, Metric, MetricType};
