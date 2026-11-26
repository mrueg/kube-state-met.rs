//! Records the rustc version for `kube-state-metrics version`.

use std::process::Command;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rustc unknown".to_owned());
    println!("cargo:rustc-env=KSM_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");
}
