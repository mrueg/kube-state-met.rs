//! The `-v` verbosity, for code that logs at a klog level (`klog.V(n)`).

use std::sync::atomic::{AtomicI32, Ordering};

static VERBOSITY: AtomicI32 = AtomicI32::new(0);

pub fn set_verbosity(v: i32) {
    VERBOSITY.store(v, Ordering::Relaxed);
}

/// `klog.V(level).Enabled()`: logging at `level` is on.
pub fn v(level: i32) -> bool {
    level <= VERBOSITY.load(Ordering::Relaxed)
}
