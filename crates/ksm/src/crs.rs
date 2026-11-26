//! Custom resource state metrics (Go: `pkg/customresourcestate`,
//! `internal/discovery`). Wave 4 of docs/port/MAP.md.

use crate::options::Options;

/// Fails while custom resource state metrics are not ported.
pub fn check_supported(_opts: &Options) -> Result<(), String> {
    Err("custom resource state metrics are not supported by this build yet".to_owned())
}
