# `--tls-config` (exporter-toolkit web config), `ListenAndServe` and the landing page

Path prefixes used below:

- `ET` = `$(go env GOMODCACHE)/github.com/prometheus/exporter-toolkit@v0.21.0/web`. The working-tree `go.mod:18` pins v0.21.0. Note that `git HEAD` still has v0.20.0, because `go.mod` and `go.sum` carry uncommitted changes.
- `PC` = `$(go env GOMODCACHE)/github.com/prometheus/common@v0.74.0`
- `GO` = `$(go env GOROOT)/src` (go1.27.4)
- `KSM` = `/home/mrueg/repos/kube-state-metrics`
- `CR` = `~/.cargo/registry/src/index.crates.io-*/`: rustls-0.23.46, tokio-rustls-0.26.6, bcrypt-0.19.3, listenfd-1.0.2.

Observed behaviour comes from running the real binary (`go build` at `KSM` HEAD plus the working tree) against a local kind cluster. Captures are in `fixtures/`.

## 1. How KSM wires it

- A single flag, `--tls-config` (default `""`), is defined at `KSM pkg/options/options.go:217`. The same string pointer is passed as `WebConfigFile` to **both** servers (`KSM pkg/app/server.go:333,369-372,383-386`).
- Each server calls `web.ListenAndServe(&server, &flags, sLogger)` in its own run-group actor (`server.go:399-411`). An error from either one, such as a bad config, ends the whole run (`server.go:413-415`), and the process exits with rc=1 (observed).
- `WebSystemdSocket` is never set (nil). **Systemd socket activation is unreachable in KSM.** `ListenAndServe` takes the TCP path (`ET tls_config.go:354,366-388`).
- Other server settings: the telemetry server has `ReadHeaderTimeout: 5s` only (`server.go:366-368`). The metrics server uses the `--server-*-timeout` flags (`server.go:376-382`).

## 2. `ListenAndServe` semantics (`ET tls_config.go:349-389`)

1. `checkFlags`: `WebConfigFile==nil` returns `ErrMissingFlag`. No listen address without systemd returns `ErrNoListeners` (`:85-103`).
2. If `WebSystemdSocket` is true, it uses `activation.Listeners()`. Zero listeners give the error `"no socket activation file descriptors found"` (`:354-364`). This path is not reachable in KSM.
3. Each address is bound with `net.Listen("tcp", addr)`, or `vsock.Listen` for addresses with a `vsock://` prefix (`:367-387`). A bind error is returned immediately.
4. `ServeMultiple` calls `configureServer` **once** and then serves all listeners (`:326-341`).
5. `configureServer` (`:423-484`):
   - An empty path means plain HTTP, with no handler wrapping at all: no headers, no basic auth and no rate limit (`:425-427`).
   - Otherwise it runs `validateUsers`, which parses the file and checks every hash with `bcrypt.Cost` (`ET handler.go:41-55`). Then `getConfig`. Then it wraps `server.Handler` in `webHandler` (`:445-454`).
   - `ConfigToTLSConfig` decides TLS. `errNoTLSConfig` means plain HTTP **with** the webHandler wrapper. Any other error is fatal (`:456-469`).
   - With TLS, `http2: false` clears `TLSNextProto`. `GetConfigForClient` **re-reads the whole file plus the cert, key and CA on every TLS handshake** (`:473-482`).
6. Startup logs come from `serveListener`: "Listening on" and "TLS is disabled."/"TLS is enabled." with `http2=` (`:486-494`). Plain HTTP always logs `http2=false`.

## 3. YAML schema (`ET tls_config.go:45-72,161-169`)

The file is decoded with `yaml.UnmarshalStrict` from go.yaml.in/yaml/v2 (`:184`), so **unknown keys are errors**. Observed error text: `yaml: unmarshal errors:\n  line 12: field bogus_key not found in type web.Config`.

| Key | Type / default | Notes |
|---|---|---|
| `tls_server_config.cert_file` / `key_file` | path | Relative paths are joined to the **config file's directory** (`:117-131,188`). |
| `tls_server_config.cert` / `key` | inline PEM | `key` is a `secret.Secret`. File wins when both are set (`:226-242`). |
| `client_ca_file` / `client_ca` | path / inline PEM | `client_ca_file` wins (`:283-295`). Unparseable PEM is silently ignored because the `AppendCertsFromPEM` result is not checked. |
| `client_auth_type` | `""` | `RequestClientCert`, `RequireAnyClientCert` (alias `RequireClientCert`), `VerifyClientCertIfGiven`, `RequireAndVerifyClientCert`, `""`/`NoClientCert`. Anything else: `invalid ClientAuth: X` (`:302-315`). |
| `min_version` / `max_version` | `TLS12` / `TLS13` (`:177-181`) | Names `TLS10`..`TLS13`. Otherwise `unknown TLS version: X` (`:603-623`). |
| `cipher_suites` | Go defaults | Names must be in Go's `tls.CipherSuites()` (`GO crypto/tls/cipher_suites.go:57-73`). Insecure suites are rejected: `unknown cipher: X` (`ET :551-566`). |
| `curve_preferences` | Go defaults | `CurveP256`, `CurveP384`, `CurveP521`, `X25519` only. Otherwise `unknown curve: X` (`:574-592`). |
| `prefer_server_cipher_suites` | `true` | Parsed and ignored (`:64-70,256-257`). |
| `client_allowed_sans` | nil | Allows the client if **any** DNS, email, IP or URI SAN equals an entry exactly (`:133-159,297-300`). |
| `http_server_config.http2` | `true` | Affects TLS only. |
| `http_server_config.headers` | `{}` | Allowlist: `Strict-Transport-Security`, `X-XSS-Protection`, `Content-Security-Policy` (any value), `X-Content-Type-Options` (`nosniff`), `X-Frame-Options` (`deny`/`sameorigin`), case-sensitive (`ET handler.go:33-39,60-77`). |
| `rate_limit.interval` / `burst` | `0` / `0` | Go duration string. `interval: 0` disables the limiter whatever `burst` is (`handler.go:103-106`). |
| `basic_auth_users` | `map[user]bcrypt` | Each hash must pass `bcrypt.Cost`. Error example: `crypto/bcrypt: hashedSecret too short to be a bcrypted password` (observed). |

Validation order and observed startup errors:

- `missing one of cert or cert_file`, `missing one of key or key_file` (`:205-211`).
- Then `failed to read cert_file (<abs path>): ...` and `failed to load X509KeyPair: ...` (`:226-247`).
- `client CA's have been configured without a Client Auth Policy` when a CA is set but `client_auth_type` is empty or NoClientCert (`:317-319`).
- Header errors: `HTTP header "Foo" can not be configured` and `invalid value for X-Frame-Options. Expected one of: ["deny" "sameorigin"], but got: "allow"`.
- A missing file gives `open /x.yml: no such file or directory`.
- KSM prints each error as `run server group error: <msg>`.

**TLS on/off:** `IsEnabled()` is true if any of cert, key, CA or `client_auth_type` is set (`:110-115`). A config with only `basic_auth_users`, `headers` or `rate_limit` serves plain HTTP **with** auth, headers and the limiter. An **empty (0-byte) file** is valid: plain HTTP, wrapper on, no-op (observed 200).

## 4. Per-request behaviour (`ET handler.go:137-197`): the file is re-read on every request

On each HTTP request, `ServeHTTP` does the following:

1. `getConfig(path)`: reads and parses the file again. On error it logs `Unable to parse configuration` and replies **500 `Internal Server Error\n`** (`:138-143`). The file is **not** re-checked with `bcrypt.Cost` here. The CA and cert are not re-read here either; that happens at the TLS handshake.
2. Rate limiter. One `*rate.Limiter` per server, shared across all paths. It is retuned in place when the config changes, so the bucket carries over (`:102-135`). An empty limiter replies **429 `Too Many Requests\n`** (`:145-148`). This happens **before** the configured headers and basic auth: a 429 carries no configured headers (observed), and failed logins consume tokens.
3. Configured headers are set with `w.Header().Set` (`:151-153`), so they also appear on 401 responses (observed `x-frame-options: deny` on a 401).
4. No users: the inner handler runs (`:155-158`).
5. Basic auth:
   - An unknown user is checked against a fixed dummy hash, as anti-enumeration (`:164-169`).
   - Results are cached under `hex(user):hex(hash):hex(pass)`, up to 100 entries, evicting about 10% at random when full (`ET cache.go:23,57-83`).
   - bcrypt runs under a **global mutex** (`:181-183`).
   - Failure replies **401 `Unauthorized\n`** with `WWW-Authenticate: Basic` (`:195-196`).

Over TLS, a broken file makes every new handshake fail. Go logs `http: TLS handshake error ... yaml: unmarshal errors`. Existing keep-alive connections still get the 500 from step 1. Plain HTTP sent to a TLS port gets Go's raw `HTTP/1.0 400 Bad Request\r\n\r\nClient sent an HTTP request to an HTTPS server.\n` (observed).

## 5. Landing page (`ET landing_page.go`, templates `ET landing_page.html`, `ET landing_page.css`)

- It uses `text/template`, **not `html/template`** (`landing_page.go:25`), so nothing is HTML-escaped. `Metrics for Kubernetes' state` is emitted verbatim.
- CSS is rendered first with `HeaderColor="#e6522c"` (`:90-99`). `Form.Width = (0+1)/2 = 0.5`, which prints as `0.5em` (`:82-89`). `{{.ExtraCSS}}` is empty, so the last CSS line is empty and `</style>` follows on the next line.
- `RoutePrefix` defaults to `"/"` and `Profiling` defaults to `"true"` (`:101-109`). Link addresses lose their leading `/` and the template puts `RoutePrefix` back (`:111-113`).
- **The metrics server** leaves `Profiling` empty (`KSM server.go:705-723`), so its page **shows the pprof box** with `/debug/pprof/heap` and `/debug/pprof/profile?seconds=60` links. The metrics port has **no pprof handlers**, so these links lead to the landing handler and return 404.
- **The telemetry server** sets `Profiling: "false"` (`server.go:617-628`), so the box is hidden, even though telemetry does serve `/debug/pprof/*` (`server.go:556-562`).
- `ServeHTTP` (`:127-134`):
  - Any path other than exactly `/` gets `http.NotFound`: `404 page not found\n`, with `text/plain; charset=utf-8` and `X-Content-Type-Options: nosniff` (`GO net/http/server.go:2377-2398`).
  - Otherwise it sets `Content-Type: text/html; charset=UTF-8` (uppercase UTF-8) and writes the bytes. **Any method** is accepted (observed `POST /` returns 200).
- If `NewLandingPage` fails, `/` is simply not registered (`KSM server.go:637-644`).
- **Byte-exact captures**, with Version = `(version=v2.99.0, branch=main, revision=abc123)`:
  - `fixtures/landing-telemetry.html`: 1034 B, sha256 `337e3778…db08a6b6`.
  - `fixtures/landing-metrics.html`: 1590 B, sha256 `308f1340…80251e83d`.
  - Whitespace-only lines are kept, including the `          ` lines around `{{ range }}` and the three `      ` lines for the empty Form, ExtraHTML and pprof blocks. The file ends with `</html>\n`.
  - To reproduce, embed the two fixtures with the version string as the only substitution. Do not rebuild them with a template engine.
- `version.Info()` returns `fmt.Sprintf("(version=%s, branch=%s, revision=%s)", Version, Branch, GetRevision())` (`PC version/info.go:73-75`):
  - `Version` and `Branch` come from `-ldflags -X github.com/prometheus/common/version.*` (`KSM Makefile:72`). Unset values give the empty string.
  - `GetRevision()` is the ldflag value, else `debug.ReadBuildInfo` `vcs.revision` (+`-modified`), else `"unknown"` (`info.go:100-105,123-150`).

## 6. Mapping to Rust

| Go feature | Rust |
|---|---|
| Listener, plain/TLS | `tokio::net::TcpListener` plus a hyper-util `auto::Builder` loop. TLS uses `tokio_rustls::LazyConfigAcceptor` (`CR tokio-rustls src/server.rs:97,272`): read the ClientHello, **re-read the config file**, build a fresh `rustls::ServerConfig`, then `into_stream`. This is the equivalent of `GetConfigForClient`. On error, drop the connection and log. |
| min/max version | `ServerConfig::builder_with_protocol_versions` (`CR rustls src/server/server_conn.rs:488`). rustls supports only TLS 1.2 and 1.3. `TLS10`/`TLS11` must parse but clamp to 1.2, or be a startup error. Decide which and record it in DECISIONS.md. |
| cipher_suites | A custom `CryptoProvider { cipher_suites: filtered, .. }`. rustls has no CBC suites (`CR rustls src/crypto/aws_lc_rs/mod.rs:102-140`). Go ignores `cipher_suites` for TLS 1.3, so keep all TLS 1.3 suites and filter only the 1.2 ones. |
| curve_preferences | `CryptoProvider.kx_groups`. X25519 → `X25519`, CurveP256 → `SECP256R1`, CurveP384 → `SECP384R1`. **There is no P-521** (`mod.rs:254-275`). |
| client_auth_type | `NoClientCert` → `with_no_client_auth()`. `VerifyClientCertIfGiven` → `WebPkiClientVerifier::builder(roots).allow_unauthenticated()` (`CR rustls src/webpki/client_verifier.rs:125,274`). `RequireAndVerifyClientCert` → the default builder. `RequestClientCert` and `RequireAnyClientCert` need a custom `ClientCertVerifier` (`src/verify.rs:160-203`) that accepts any cert, with `client_auth_mandatory` set to false or true. |
| client_allowed_sans | Inside the verifier, after chain verification, parse the SANs from the end-entity cert, for example with `x509-parser`. URI SANs must be compared as strings exactly as written. IPs must be compared in Go `net.IP.String()` form. |
| http2 flag | `alpn_protocols = [h2, http/1.1]` or `[http/1.1]`. Plain HTTP stays HTTP/1.1 only. Go's net/http does not do h2c here, so do not enable h2c prior-knowledge. |
| headers, rate limit, basic auth | A tower `Layer` that reads the file per request. Use a token bucket compatible with `rate.Every(interval)`/`burst`. Hash checks with `bcrypt::verify` (`CR bcrypt src/lib.rs:366`) accept `$2a/$2b/$2x/$2y` (`:208`). Its truncation at 72 bytes matches Go. Run it under a `Mutex`, or in `spawn_blocking` with a semaphore of 1, plus a 100-entry cache. |
| systemd | Not needed, because KSM cannot enable it. If added later: `listenfd::ListenFd::from_env()` (`CR listenfd src/unix.rs:112-124`). |

## Semantic traps

1. **The config file is re-read on every request and on every TLS handshake.** The TLS cert and key can be rotated without a restart. A broken edit turns every request into a 500, or makes every handshake fail, until it is fixed. Do not cache the file.
2. **Startup validates bcrypt hashes; per-request reloads do not.** An invalid hash added later just never matches.
3. **Empty `--tls-config` means no wrapper at all.** A present but empty file means the wrapper is active and does nothing. With TLS keys absent, the wrapper still applies basic auth, headers and the rate limit over plain HTTP.
4. **The rate limit runs before auth and before headers.** Each server has its own bucket, shared by `/metrics`, `/healthz`, `/livez`, `/readyz` and `/`. `burst: 0` with an interval set rejects **every** request (`x/time/rate@v0.16.0 rate.go:78`).
5. **Relative `cert_file`, `key_file` and `client_ca_file` resolve against the config file's directory**, not the CWD.
6. **Unknown YAML keys are fatal** (strict decoding). `prefer_server_cipher_suites` must still be accepted.
7. **The metrics-port landing page links to pprof endpoints that do not exist there.** The telemetry page hides the pprof box although pprof is served. Reproduce both as they are.
8. **The landing HTML is not escaped**, and its `Content-Type` has uppercase `UTF-8`. Only `/` exactly is served; any method is accepted.
9. **rustls cannot do TLS 1.0/1.1, CBC suites or P-521.** Go accepts all of these. A config using them must get a documented decision.
10. With `client_allowed_sans` set, Go calls `VerifyPeerCertificate` even when the client sent no cert (`GO crypto/tls/handshake_server.go:1019-1024`). This is code reading only, not observed. `ET tls_config.go:136` then indexes `rawCerts[0]` and panics, and net/http's per-connection recover drops the connection, so such clients never succeed. Rejecting them in Rust is equivalent.
