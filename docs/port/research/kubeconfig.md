# Kubeconfig / client construction: client-go `BuildConfigFromFlags` vs `kube::Config`

Path prefixes used below:

- `CG` = `$(go env GOMODCACHE)/k8s.io/client-go@v0.37.2`
- `KC` = `~/.cargo/registry/src/index.crates.io-*/kube-client-5.2.0`
- `K` = `~/.cargo/registry/src/index.crates.io-*/kube-5.2.0`
- `KSM` = `/home/mrueg/repos/kube-state-metrics`

## 1. What KSM does

- `--apiserver` and `--kubeconfig` both go straight into `clientcmd.BuildConfigFromFlags(opts.Apiserver, opts.Kubeconfig)` (`KSM pkg/app/server.go:205`, `KSM pkg/util/utils.go:44-45`).
- After that, KSM sets these fields (`KSM pkg/util/utils.go:50-52`):
  - `UserAgent = "kube-state-metrics/<ver> (<GOOS>/<GOARCH>) kubernetes/<rev>"`
  - `AcceptContentTypes = "application/vnd.kubernetes.protobuf,application/json"`
  - `ContentType = "application/vnd.kubernetes.protobuf"`
- The config is rebuilt every time and is not memoised, because the kubeconfig file is watched for rotation (`KSM pkg/util/utils.go:40-43`).
- Auth plugins come from a blank import, `_ "k8s.io/client-go/plugin/pkg/client/auth"` (`KSM pkg/app/server.go:42`).

## 2. `BuildConfigFromFlags(masterUrl, kubeconfigPath)` semantics

The function is in `CG tools/clientcmd/client_config.go:680-694`. Its behaviour by case:

1. **Both empty.** It logs a warning and tries `rest.InClusterConfig()`. If that succeeds, it returns it (`:681-690`). If not, it falls through to step 3 with an empty `ExplicitPath`.
2. **Loading rules.** These are `&ClientConfigLoadingRules{ExplicitPath: kubeconfigPath}` (`:692`). The `Precedence` list is **empty** and there is no `DefaultClientConfig`. As a result:
   - **`KUBECONFIG` and `~/.kube/config` are never read.** Those come only from `NewDefaultClientConfigLoadingRules` (`CG tools/clientcmd/loader.go:160-180`), which is not used here.
   - A non-existent explicit path is a hard error, the `os.Stat` result (`loader.go:207-212`).
3. **Overrides.** `&ConfigOverrides{ClusterInfo: {Server: masterUrl}}` replaces only the server URL of the current context's cluster. The CA, TLS server name and user credentials of that cluster and user are kept (`:693`).
4. **`DeferredLoadingClientConfig.ClientConfig()`** (`CG tools/clientcmd/merged_client_builder.go:96-128`):
   - If the merged config is valid, it is returned. `IsDefaultConfig` is always false here because `DefaultClientConfig == nil` (`loader.go:386-395`).
   - Only on an *empty-config* error does it try in-cluster again. In that case `masterUrl` overrides the in-cluster host while keeping the in-cluster token and CA (`client_config.go:617-644`, `Possible()` at `:668-673`).
   - Otherwise it returns `ErrEmptyConfig` ("no configuration has been provided…", `CG tools/clientcmd/validation.go:34`).

**Resulting matrix:**

| `--apiserver` | `--kubeconfig` | Result |
|---|---|---|
| "" | "" | In-cluster config. If not in a pod: `invalid configuration: no configuration has been provided` |
| URL | "" | **Host = URL only, with no token or CA**. A merged config with just a server is valid (`client_config.go:172-197`), so in-cluster credentials are *not* used even inside a pod |
| "" | path | The current context of that single file. Error if the file is missing |
| URL | path | The current context of the file, with `cluster.server` replaced by URL. The file's CA and credentials still apply |

**`InClusterConfig`** (`CG rest/config.go:546-577`):
- It requires `KUBERNETES_SERVICE_HOST` and `KUBERNETES_SERVICE_PORT` and reads `/var/run/secrets/kubernetes.io/serviceaccount/token`.
- A missing or invalid `ca.crt` is **only logged**.
- It sets `BearerTokenFile`, so the token is periodically re-read.
- It never reads the namespace file.

## 3. Reproducing it with kube 5.2

kube API surface:
- `Config::new(uri)` (`KC src/config/mod.rs:208-226`)
- `Config::incluster()`, which is the env-var variant (`:258-273`)
- `Config::from_kubeconfig(&KubeConfigOptions)`, which **always reads `KUBECONFIG`/`~/.kube/config`** through `Kubeconfig::read()` (`:319-322`, `KC src/config/file_loader.rs:34-45`, `KC src/config/file_config.rs:509-514`)
- `Kubeconfig::read_from(path)` (`file_config.rs:441-500`)
- `Config::from_custom_kubeconfig(kc, &opts)` (`mod.rs:327-333`)
- `Config::infer()`, which tries kubeconfig then in-cluster, i.e. **different** precedence (`mod.rs:237-254`)

Do **not** use `infer()` or `from_kubeconfig()`. Use this instead:

```rust
async fn build_config(apiserver: &str, kubeconfig: &str) -> anyhow::Result<kube::Config> {
    use kube::config::{Config, Kubeconfig, KubeConfigOptions};
    if apiserver.is_empty() && kubeconfig.is_empty() {
        tracing::warn!("Neither --kubeconfig nor --master was specified. Using the inClusterConfig.");
        match Config::incluster() {
            Ok(c) => return Ok(c),
            Err(e) => tracing::warn!("error creating inClusterConfig, falling back to default config: {e}"),
        }
        anyhow::bail!("invalid configuration: no configuration has been provided");
        // (client-go's in-cluster retry in DeferredLoading cannot succeed here: incluster just failed)
    }
    if kubeconfig.is_empty() {
        return Ok(Config::new(apiserver.parse()?)); // host only, no creds (matches client-go)
    }
    let mut kc = Kubeconfig::read_from(kubeconfig)?;              // missing file => error
    if !apiserver.is_empty() {                                    // override server of the *current* cluster
        let ctx_name = kc.current_context.clone().unwrap_or_default();
        let cluster_name = kc.contexts.iter().find(|c| c.name == ctx_name)
            .and_then(|c| c.context.as_ref()).map(|c| c.cluster.clone());
        for nc in kc.clusters.iter_mut().filter(|c| Some(&c.name) == cluster_name.as_ref()) {
            if let Some(c) = nc.cluster.as_mut() { c.server = Some(apiserver.to_string()); }
        }
    }
    Ok(Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default()).await?)
}
```

Override the server **before** loading. `new_from_loader` derives `proxy_url` and `tls_server_name` from the cluster and parses `server` into `cluster_url` (`KC src/config/mod.rs:335-378`). Setting `config.cluster_url` after loading works too, but skips the `no_proxy` evaluation for the new host.

## 4. Auth plugin coverage

| Mechanism | client-go 0.37.2 (as KSM builds it) | kube 5.2 |
|---|---|---|
| Static token / `token-file` | Token wins over file. The file is read at build time and then refreshed (`client_config.go:277-287`) | Inline token wins over `token_file`. The file is re-read at least once a minute (`KC src/client/auth/mod.rs:331-341`). Always on |
| Client cert / key (file or data) | `client_config.go:296-301` | Always on: `identity_pem` (`KC src/config/mod.rs:418-421`) |
| Basic auth | `:302-305` | Always on (`auth/mod.rs:327-329`) |
| exec (`client.authentication.k8s.io`) | Built into `rest` (`CG rest/transport.go:25,124-139`). Exec and auth-provider together is an error (`:120`) | Always on. **Runs synchronously when the Client is built** and refreshes on expiry (`auth/mod.rs:343-367`, `auth_exec` at `:565-609`) |
| `auth-provider: oidc` | Plugin imported by `plugins.go:21`. Refreshes using `refresh-token` and `idp-issuer-url` (`CG plugin/pkg/client/auth/oidc/oidc.go:38-51`) | Feature `oidc` gives a refreshable token (`auth/mod.rs:403-409`). Without the feature, a static `id-token` is used (`:410-419`) |
| `auth-provider: gcp` | **Stub that returns an error** ("The gcp auth plugin has been removed", `CG plugin/pkg/client/auth/gcp/gcp_stub.go:26-33`), imported via `plugins_providers.go:21-22` | **Supported**: `cmd-path` command, cached `access-token`, and the `oauth` feature for application-default credentials (`auth/mod.rs:420-470`) |
| `auth-provider: azure` | Error stub (`.../azure/azure_stub.go:26-33`) | Error: "not supported; use kubelogin" (`auth/mod.rs:393-395`) |
| Impersonation | `client_config.go:288-295` | Always on: headers from `auth_info.impersonate*` (`KC src/client/config_ext.rs:368-393`) |

**Cargo features.** The `kube` crate defaults to `client`, `rustls-tls` and `ring` (`K Cargo.toml [features] default`). For parity with KSM, enable `kube = { version = "5.2", features = ["runtime", "oidc", "gzip", "http-proxy"] }`:
- Add `"socks5"` if SOCKS proxies are needed.
- Do **not** add `oauth`, since client-go cannot do gcp anyway.
- With `rustls-tls` and no explicit CA, the OS platform verifier is used (`KC src/client/tls.rs:71-80`). The `webpki-roots` feature switches to bundled roots.

## 5. Raw `/version` and `/livez`

KSM calls them as follows:
- `/version` through discovery `ServerVersionWithContext` (`KSM pkg/util/utils.go:80`).
- `/livez` and `/readyz` probe delegation through `CoreV1().RESTClient().Get().AbsPath("/livez").Do(ctx)`. Any non-2xx response or transport error gives 503 (`KSM pkg/app/server.go:646-663`).

With kube:

```rust
let info = client.apiserver_version().await?;            // GET /version -> k8s_openapi::apimachinery::pkg::version::Info (KC src/client/mod.rs:421-424)
let req = http::Request::get("/livez").body(Vec::new())?;
let ok = client.request_text(req).await.is_ok();         // non-2xx -> Err(Error::Api(Status)) (mod.rs:295-301, 585-611)
```

`BaseUriLayer` prepends `cluster_url`, including any path prefix (`KC src/client/middleware/base_uri.rs:59-78`). That matches `AbsPath`, which also keeps the host's path prefix. Pass a cancellable future (for example `tokio::time::timeout`) to mirror `r.Context()`.

## 6. User agent and content type

- **User agent.** kube-client sets **no `User-Agent`** (it does not appear anywhere in `KC src/`). Add it explicitly: `config.headers.push((http::header::USER_AGENT, format!("kube-state-metrics/{VER} ({os}/{arch}) kubernetes/{rev}").parse()?))`. These headers are applied by `extra_headers_layer` (`KC src/config/mod.rs:196-197`, `config_ext.rs:368-370`).
- **Content type.** kube is JSON-only. List and get requests send no Accept header, and metadata requests send `application/json;as=PartialObjectMetadata…` (`KCO src/request.rs:8-12`). KSM's protobuf negotiation cannot be reproduced and is not needed for correctness. Expect higher apiserver CPU and bytes. k8s-openapi has no protobuf decoding.
- **Rate limiting.** client-go defaults to QPS 5 and Burst 10 for non-watch requests (`CG rest/config.go:48-49`). Watches are not rate limited (`CG rest/request.go:765-766`). kube has no client-side rate limiter. Its retry layer retries 429, 503 and 504 by default (`default_retry: true`, `KC src/config/mod.rs:198-199,224`; `KC src/client/retry.rs:54-56`).
- **Timeouts.** kube defaults to a 30 s connect timeout, a 295 s write timeout, and no read timeout (`KC src/config/mod.rs:447-448`). Watches rely on the watcher's idle timeout.

## Semantic traps

1. **`BuildConfigFromFlags` ignores `KUBECONFIG` and `~/.kube/config`** (`client_config.go:691-693`). `kube::Config::infer()` and `from_kubeconfig()` read them. Never use either for KSM flags.
2. **`--apiserver` alone inside a pod sends no service-account token.** The server-only merged config is valid, so in-cluster is not consulted (`merged_client_builder.go:104-116`). Use `Config::new(url)`, not `incluster()` plus an override.
3. **`--apiserver` with `--kubeconfig` overrides only `server`.** The CA and `tls-server-name` from the file still apply, so a cert mismatch against the new host is expected behaviour.
4. **kube's in-cluster config is stricter.**
   - kube requires `ca.crt` and the `namespace` file (`KC src/config/incluster_config.rs:99-107`, `mod.rs:288-312`). client-go only logs a missing CA and never reads the namespace file (`rest/config.go:561-568`).
   - Fall back to `Config::new` plus `token_file` if the port must tolerate a missing namespace file.
5. **kube's `Kubeconfig::from_env` errors if any `KUBECONFIG` entry is missing**, whereas client-go skips missing entries. This is irrelevant here because KSM never reads `KUBECONFIG`, but it would bite `infer()`.
6. **gcp auth-provider behaviour differs.** client-go 0.37 errors, while kube executes `cmd-path`. To keep error parity, reject `auth_provider.name == "gcp"` explicitly.
7. **exec plugins run during `Client::try_from(config)`.** They block the async runtime thread, so build the client inside `spawn_blocking` if the plugin is slow. client-go runs exec lazily on the first request.
8. **No User-Agent and no protobuf by default in kube.** Set the UA header manually, and accept JSON-only traffic.
9. **CA rotation.** client-go 0.37 has `ClientsAllowCARotation` set to Beta, default on (`CG features/known_features.go:115-117`). kube re-reads the CA only for `root_cert_file`, which only `incluster()` sets, and only with rustls (`KC src/config/mod.rs:143-151`). A kubeconfig CA path is read once. KSM already rebuilds the config on kubeconfig change, so do the same: rebuild the `Client` and restart the watchers.
10. **Retry semantics.** kube's default `RetryLayer` retries 429, 503 and 504 transparently. For `/livez` delegation this can hide a failing apiserver behind retries. Build the probe client with `config.default_retry = false` and a short timeout.
