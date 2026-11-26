//! Kubernetes client setup (Go: `pkg/util.CreateKubeClient`,
//! `clientcmd.BuildConfigFromFlags`). See docs/port/research/kubeconfig.md.

use kube::config::{Config, KubeConfigOptions, Kubeconfig};

/// Version of this build, as `version.Version` in Go.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `clientcmd.BuildConfigFromFlags(apiserver, kubeconfig)`: both empty means
/// in-cluster; `--apiserver` alone a host-only config (no credentials); with a
/// kubeconfig, `--apiserver` overrides the current cluster's server.
/// `KUBECONFIG` and `~/.kube/config` are never read.
pub async fn build_config(apiserver: &str, kubeconfig: &str) -> Result<Config, String> {
    let mut config = if apiserver.is_empty() && kubeconfig.is_empty() {
        tracing::warn!(
            "Neither --kubeconfig nor --master was specified.  Using the inClusterConfig.  This might not work."
        );
        Config::incluster().map_err(|e| {
            tracing::warn!("error creating inClusterConfig, falling back to default config: {e}");
            "invalid configuration: no configuration has been provided, try setting KUBERNETES_MASTER environment variable".to_owned()
        })?
    } else if kubeconfig.is_empty() {
        let url = apiserver.parse().map_err(|e| format!("invalid apiserver URL {apiserver:?}: {e}"))?;
        Config::new(url)
    } else {
        let mut kc = Kubeconfig::read_from(kubeconfig).map_err(|e| e.to_string())?;
        if !apiserver.is_empty() {
            let ctx_name = kc.current_context.clone().unwrap_or_default();
            let cluster_name = kc
                .contexts
                .iter()
                .find(|c| c.name == ctx_name)
                .and_then(|c| c.context.as_ref())
                .map(|c| c.cluster.clone());
            for nc in kc.clusters.iter_mut().filter(|c| Some(&c.name) == cluster_name.as_ref()) {
                if let Some(c) = nc.cluster.as_mut() {
                    c.server = Some(apiserver.to_owned());
                }
            }
        }
        Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default())
            .await
            .map_err(|e| e.to_string())?
    };
    if let Ok(ua) = format!(
        "kube-state-metrics/{VERSION} ({}/{}) kubernetes/unknown",
        std::env::consts::OS,
        go_arch()
    )
    .parse()
    {
        config.headers.push((http::header::USER_AGENT, ua));
    }
    Ok(config)
}

/// `runtime.GOARCH` naming.
pub fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "powerpc64" => "ppc64le",
        other => other,
    }
}

/// Creates a client and checks the API server answers `/version`.
pub async fn create_client(apiserver: &str, kubeconfig: &str) -> Result<kube::Client, String> {
    let config = build_config(apiserver, kubeconfig)
        .await
        .map_err(|e| format!("failed to build config from flags: {e}"))?;
    let client = kube::Client::try_from(config).map_err(|e| format!("failed to create client: {e}"))?;
    tracing::info!("Tested communication with server");
    let v = client
        .apiserver_version()
        .await
        .map_err(|e| format!("failed to create client: error while trying to communicate with apiserver: {e}"))?;
    tracing::info!(
        major = %v.major, minor = %v.minor, gitVersion = %v.git_version, gitTreeState = %v.git_tree_state,
        gitCommit = %v.git_commit, platform = %v.platform,
        "Run with Kubernetes cluster version"
    );
    tracing::info!("Communication with server successful");
    Ok(client)
}
