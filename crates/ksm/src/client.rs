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
        incluster().map_err(|e| {
            tracing::warn!("error creating inClusterConfig, falling back to default config: {e}");
            "invalid configuration: no configuration has been provided, try setting KUBERNETES_MASTER environment variable".to_owned()
        })?
    } else if kubeconfig.is_empty() {
        let url = with_default_scheme(apiserver, false)
            .parse()
            .map_err(|e| format!("invalid apiserver URL {apiserver:?}: {e}"))?;
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
            for nc in kc
                .clusters
                .iter_mut()
                .filter(|c| Some(&c.name) == cluster_name.as_ref())
            {
                if let Some(c) = nc.cluster.as_mut() {
                    c.server = Some(apiserver.to_owned());
                }
            }
        }
        default_server_schemes(&mut kc);
        Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default())
            .await
            .map_err(|e| e.to_string())?
    };
    if let Ok(ua) = format!(
        "kube-state-metrics/{VERSION} ({}/{}) kubernetes/{}",
        std::env::consts::OS,
        go_arch(),
        crate::server::revision()
    )
    .parse()
    {
        config.headers.push((http::header::USER_AGENT, ua));
    }
    Ok(config)
}

/// `runtime.GOARCH` naming.
pub fn go_arch() -> &'static str {
    let little = cfg!(target_endian = "little");
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "powerpc64" if little => "ppc64le",
        "powerpc64" => "ppc64",
        "mips64" if little => "mips64le",
        "mips" if little => "mipsle",
        "loongarch64" => "loong64",
        other => other,
    }
}

const SERVICE_ACCOUNT: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

/// `rest.InClusterConfig`: the service host and port from the environment
/// and the service account token are required; a CA that can't be loaded is
/// logged and the system roots are used, and the namespace file is not read
/// (kube's `Config::incluster` requires both files).
fn incluster() -> Result<Config, String> {
    let host = std::env::var("KUBERNETES_SERVICE_HOST").unwrap_or_default();
    let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_default();
    if host.is_empty() || port.is_empty() {
        return Err("unable to load in-cluster configuration, KUBERNETES_SERVICE_HOST and KUBERNETES_SERVICE_PORT must be defined".to_owned());
    }
    let token = format!("{SERVICE_ACCOUNT}/token");
    std::fs::read(&token).map_err(|e| crate::server::read_file_error(&token, &e))?;
    let hostport = crate::server::join_host_port(&host, &port);
    let url = format!("https://{hostport}")
        .parse()
        .map_err(|e| format!("invalid in-cluster API server URL: {e}"))?;
    let mut config = Config::new(url);
    config.auth_info.token_file = Some(token);
    let ca = format!("{SERVICE_ACCOUNT}/ca.crt");
    let certs: Vec<Vec<u8>> = std::fs::read(&ca)
        .map(|b| {
            crate::web::pem_certs(&b)
                .iter()
                .map(|c| c.to_vec())
                .collect()
        })
        .unwrap_or_default();
    if certs.is_empty() {
        tracing::error!(
            "Expected to load root CA config from {ca}, but got err: no valid certificates"
        );
    } else {
        config.root_cert = Some(certs);
        config.root_cert_file = Some(ca.into());
    }
    Ok(config)
}

/// `rest.DefaultServerURL`: a host without a scheme gets `https://` when the
/// config has TLS settings (a CA, a client certificate or `insecure`),
/// `http://` otherwise.
fn with_default_scheme(host: &str, tls: bool) -> String {
    let has_scheme = host.split_once("://").is_some_and(|(scheme, rest)| {
        scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && !rest.is_empty()
            && !rest.starts_with('/')
    });
    if has_scheme {
        host.to_owned()
    } else if tls {
        format!("https://{host}")
    } else {
        format!("http://{host}")
    }
}

/// Gives every cluster's server a scheme as client-go does: `https://` when
/// the cluster or a user referring to it has TLS settings.
fn default_server_schemes(kc: &mut Kubeconfig) {
    let user_cert = |cluster: &str| {
        kc.contexts.iter().any(|c| {
            c.context.as_ref().is_some_and(|ctx| {
                ctx.cluster == cluster
                    && kc.auth_infos.iter().any(|a| {
                        Some(&a.name) == ctx.user.as_ref()
                            && a.auth_info.as_ref().is_some_and(|ai| {
                                ai.client_certificate.is_some()
                                    || ai.client_certificate_data.is_some()
                            })
                    })
            })
        })
    };
    let certs: Vec<bool> = kc.clusters.iter().map(|c| user_cert(&c.name)).collect();
    for (nc, cert) in kc.clusters.iter_mut().zip(certs) {
        if let Some(c) = nc.cluster.as_mut()
            && let Some(server) = &c.server
        {
            let tls = cert
                || c.certificate_authority.is_some()
                || c.certificate_authority_data.is_some()
                || c.insecure_skip_tls_verify == Some(true);
            c.server = Some(with_default_scheme(server, tls));
        }
    }
}

/// Creates a client and checks the API server answers `/version`.
pub async fn create_client(apiserver: &str, kubeconfig: &str) -> Result<kube::Client, String> {
    let config = build_config(apiserver, kubeconfig)
        .await
        .map_err(|e| format!("failed to build config from flags: {e}"))?;
    let client =
        kube::Client::try_from(config).map_err(|e| format!("failed to create client: {e}"))?;
    tracing::info!("Tested communication with server");
    let v = client.apiserver_version().await.map_err(|e| {
        format!("failed to create client: error while trying to communicate with apiserver: {e}")
    })?;
    tracing::info!(
        major = %v.major, minor = %v.minor, gitVersion = %v.git_version, gitTreeState = %v.git_tree_state,
        gitCommit = %v.git_commit, platform = %v.platform,
        "Run with Kubernetes cluster version"
    );
    tracing::info!("Communication with server successful");
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_without_a_scheme_get_one() {
        assert_eq!(
            with_default_scheme("127.0.0.1:8080", false),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            with_default_scheme("localhost:6443", true),
            "https://localhost:6443"
        );
        assert_eq!(
            with_default_scheme("example.com", false),
            "http://example.com"
        );
        assert_eq!(
            with_default_scheme("https://k8s:6443", false),
            "https://k8s:6443"
        );
        assert_eq!(with_default_scheme("http://k8s", true), "http://k8s");
    }
}
