# `--auth-filter`: controller-runtime `filters.WithAuthenticationAndAuthorization`

Path prefixes used below:

- `CRT` = `$(go env GOMODCACHE)/sigs.k8s.io/controller-runtime@v0.25.3/pkg/metrics/filters`
- `AS` = `$(go env GOMODCACHE)/k8s.io/apiserver@v0.37.0`
- `API` = `$(go env GOMODCACHE)/k8s.io/api@v0.37.2`
- `GO` = `$(go env GOROOT)/src`
- `KSM` = `/home/mrueg/repos/kube-state-metrics`

Evidence comes from three places:

- Code reading.
- A harness in the scratchpad (`gotool/main.go`) that ran the real filter against a fake API server, logging the request bodies.
- The real KSM binary run against kind, with `--auth-filter` and a `kubectl create token default` token.

## 1. Where KSM applies it

- Flag: `--auth-filter` (default false) (`KSM pkg/options/options.go:206`).
- **Metrics port.** Only `/metrics` is wrapped, together with the duration histogram (`KSM pkg/app/server.go:666-693`). `/healthz`, `/livez` and `/` stay open.
- **Telemetry port.** `/metrics` and the five `/debug/pprof/*` handlers are wrapped (`server.go:547-602`). `/readyz` and `/` stay open. Observed: `pprof 401`, `readyz 200`, `root 200`.
- **Failure to build the filter is not fatal.** It can fail in `rest.HTTPClientFor`, in `WithAuthenticationAndAuthorization`, or when applying the filter. The protected handler is then replaced by **503 `auth filter unavailable\n`** (via `http.Error`), and on telemetry the pprof handlers are dropped entirely (`server.go:565-597,673-691`).
- The filter is built from the **raw `kubeConfig` from `clientcmd.BuildConfigFromFlags`** (`server.go:205`). This is not the protobuf/User-Agent config from `util.CreateKubeClient`. The review calls therefore use JSON and, observed, `User-Agent: Go-http-client/1.1`.
- The two servers construct **independent filters**, so each has its own caches.

## 2. Pipeline per request (`CRT filters.go:99-136`)

Authenticator = `AuthenticatedGroupAdder(union(bearertoken, websocketProtocol))` (`AS pkg/authentication/authenticatorfactory/delegating.go:92-128`):

- Anonymous is disabled (`CRT filters.go:63`).
- There is no x509 and no request-header authenticator, because the config leaves them nil (`delegating.go:75-90`).
- `union.New` with `FailOnError=false`: it tries every authenticator and returns the **aggregate error** if none succeeds (`AS pkg/authentication/request/union/union.go:36-70`).

### 2a. Token extraction (headers read)

**`Authorization`** (`AS pkg/authentication/request/bearertoken/bearertoken.go:42-75`):

- The value is `TrimSpace`'d, then `SplitN(" ", 3)`. `parts[0]` must equal `bearer` case-insensitively (`BEARER x` works). The token is `parts[1]`.
- Anything after a second space is ignored: `Bearer good extra` authenticates as `good` (harness).
- `Bearer  tok` with two spaces gives an empty `parts[1]`: not-mine, so 401 (observed).
- `Basic ...` or no header: not-mine, so 401.
- If a bearer token is present but the review says unauthenticated, the authenticator returns **error `invalid bearer token`**, which leads to a **500** (see 2d).
- On success, the `Authorization` header is **deleted** before the inner handler runs (`:67`). The harness confirmed the inner handler saw `auth=`.

**`Sec-WebSocket-Protocol`**, only when the request is a WebSocket upgrade (`Connection: Upgrade` + `Upgrade: websocket`) (`AS pkg/authentication/request/websocket/protocol.go:48-108`):

- The token comes from the subprotocol `base64url.bearer.authorization.k8s.io.<base64url-nopad(token)>`.
- At least one other subprotocol is required. Several token protocols, bad base64 or non-UTF-8 are errors, which give a 500.
- On success the token protocol is removed from the header.

For KSM, a scrape endpoint, this path is harmless, but it is part of the observable surface.

### 2b. TokenReview (authentication)

`POST {host}/apis/authentication.k8s.io/v1/tokenreviews` with `Content-Type: application/json` and `Accept: application/json, */*`, using the kubeconfig credentials. Body as captured by the harness:

```json
{"kind":"TokenReview","apiVersion":"authentication.k8s.io/v1","metadata":{},"spec":{"token":"<tok>"},"status":{"user":{}}}
```

- `spec.audiences` is **never** sent. Nothing puts audiences in the context and `APIAudiences` is nil, so `checkAuds=false` (`AS plugin/pkg/authenticator/token/webhook/webhook.go:111-117,158-167`).
- **Timeout:** 10s per call (`CRT filters.go:66`, applied at `webhook.go:127-130`). The call runs in a context detached from the request, capped at 30s (`AS pkg/authentication/token/cache/cached_token_authenticator.go:45,191`).
- **Retry:** backoff 500ms ×1.5, jitter 0.2, 5 steps (`CRT filters.go:69-74`). Only transient errors are retried: connection reset, HTTP/2 lost, 500 InternalError, 504 Timeout, 429, or anything with Retry-After (`AS pkg/util/webhook/webhook.go:63-73,121-145`).
- **Result** (`webhook.go:169-194`):
  - `status.authenticated=false` with `status.error` set gives an error. Without `status.error` it is (not ok, nil). **Both end as a 500 at the filter**, because bearertoken turns not-ok into `invalid bearer token` (verified: `Bearer bad` and `Bearer errtok` each gave a 500).
  - On success, the user is `{Name: status.user.username, UID, Groups, Extra}`.
- **Group adder.** `system:authenticated` is **appended** unless the user is `system:anonymous` or already has `system:authenticated` or `system:unauthenticated` (`AS pkg/authentication/group/authenticated_group_adder.go:39-65`).
- **Cache** (`cached_token_authenticator.go:96-127,144-229`):
  - Key: HMAC-SHA256 over the token and audiences, with a random per-process key.
  - Storage: 32 striped simple caches.
  - TTL: **success 1m, failure (not ok, no error) 1m** (`CRT filters.go:64`; `delegating.go:104`).
  - **Errors are not cached** (`cacheErrs=false`, `:207-209`). A TokenReview returning `status.error`, or a transport failure, is retried on the next scrape.
  - Concurrent identical lookups are deduplicated with singleflight.

### 2c. SubjectAccessReview (authorization)

Attributes: `{User: <authn user>, Verb: strings.ToLower(req.Method), Path: req.URL.Path}` (`CRT filters.go:115-119`). These are non-resource attributes, and `ResourceRequest` stays false.

`POST {host}/apis/authorization.k8s.io/v1/subjectaccessreviews` (`AS plugin/pkg/authorizer/webhook/webhook.go:186-206,487-501`). Body as captured by the harness for `GET /metrics?x=1`:

```json
{"kind":"SubjectAccessReview","apiVersion":"authorization.k8s.io/v1","metadata":{},"spec":{"nonResourceAttributes":{"path":"/metrics","verb":"get"},"user":"alice","groups":["g1","system:authenticated"],"extra":{"k":["v"]},"uid":"u1"},"status":{"allowed":false}}
```

- The path **excludes the query string**. The verb is the **lowercased HTTP method** (`head`, `post` were seen), not the Kubernetes verb mapping. `user`, `groups`, `extra` and `uid` all use `omitempty` (`API authorization/v1/types.go:193-227`).
- **Cache:** an LRU of 8192 entries (`webhook.go:128`), keyed by `json.Marshal(spec)` (`:220-224`). **Allowed: 5m. Not allowed: 30s** (`CRT filters.go:83-84`; `webhook.go:275-281`). There is a size guard of 10000 bytes on verb and path (`:540-549`).
- **Retry and backoff** are the same as for TokenReview. There is no explicit per-call timeout; the request context is used.
- **On error:** it returns `decisionOnError=NoOpinion` together with the error (`AS pkg/authorization/authorizerfactory/delegating.go:60-68`; `webhook.go:263-272`), and the filter gives a 500.
- **Decision** (`webhook.go:284-292`):
  - `denied && allowed` gives Deny **plus an error**, so a 500.
  - `denied` gives Deny, so a 403.
  - `allowed` gives Allow.
  - Otherwise NoOpinion, so a **403**, because the filter only passes `DecisionAllow` (`CRT filters.go:128`).

### 2d. What the client receives

Every error goes through `http.Error`. The headers are `Content-Type: text/plain; charset=utf-8` and `X-Content-Type-Options: nosniff`, and the body is the message plus `\n` (`GO net/http/server.go:2377-2395`).

| Situation | Code | Body |
|---|---|---|
| No usable token (no header, non-bearer, empty token) | 401 | `Unauthorized\n` (no `WWW-Authenticate` header) |
| Token present; TokenReview unauthenticated, errored, or transport failure | 500 | `Authentication failed\n` |
| SAR transport error, or allowed+denied | 500 | `Authorization for user <username> failed\n` |
| SAR denied / no opinion | 403 | `Authorization denied for user <username>\n` (observed: `Authorization denied for user system:serviceaccount:default:default`) |
| Allowed | inner handler | `Authorization` header stripped |
| Filter construction failed (KSM) | 503 | `auth filter unavailable\n` |

`<username>` is the raw `status.user.username`. Logging: authentication errors at error level (`Authentication failed` with `err="invalid bearer token"` observed); 401 and 403 at V(4) (`CRT filters.go:104-131`).

## 3. Rust port recipe

1. Read `Authorization` with the parsing rules from 2a: trim, split into at most 3 parts on single spaces, case-insensitive `bearer`, non-empty `parts[1]`. Optionally support the websocket subprotocol path, or document it as dropped.
2. Look the token up in a cache keyed by HMAC or SHA-256 of the token. Entries expire after 60s, for both authenticated and not-authenticated results. Errors are not cached. Collapse concurrent lookups with a per-key `tokio::sync::OnceCell` or `futures::Shared`.
3. On a miss, use `kube::Api::<TokenReview>::all(client).create(&PostParams::default(), &tr)`, with `spec.token` only, under a 10s `tokio::time::timeout` and the retry policy from 2b. Use the same `kube::Client` as everything else, configured from `--kubeconfig`/`--apiserver`. Content type is irrelevant; JSON is what Go sends.
4. Append `system:authenticated` to the groups when the 2b rules call for it.
5. Build the SAR spec `{nonResourceAttributes:{path: uri.path(), verb: method.as_str().to_ascii_lowercase()}, user, groups, extra, uid}`. Use the serialized spec JSON as the cache key, in an LRU of 8192 with TTL 5m for allowed and 30s otherwise. Allow only when `allowed && !denied`.
6. Reply exactly as in the 2d table, then call the inner service with `Authorization` removed.
7. Implement it as a tower layer applied **only** to `/metrics` (both ports) and `/debug/pprof/*` (telemetry).
8. Keep a separate instance per port, or share one. Sharing is not observable except through cache hit rates.

## Semantic traps

1. **An invalid or expired token gives a 500, not a 401** (`bearertoken.go:71-73` + `union.go:70` + `CRT filters.go:104-107`). Only a missing or non-bearer token gives a 401. A Rust port that "fixes" this changes alerting semantics.
2. **The verb is the lowercased HTTP method.** HEAD gives `head`, so a ClusterRole granting only `get` on `/metrics` gets **403 for HEAD**. The path never contains the query string, and there is no trailing-slash normalisation.
3. **TTL asymmetry:** tokens are cached 1m for both outcomes, SAR allow 5m, SAR deny 30s. RBAC revocations therefore take up to 5 minutes, and grants up to 30 seconds.
4. **Errors are never cached** in either cache. An API-server outage makes every scrape re-hit the API server through the full backoff, about 5 attempts and roughly 5s.
5. **The `Authorization` header is removed** before the wrapped handler runs.
6. **The 401 carries no `WWW-Authenticate` header.** This contrasts with web-config basic auth, which sends `WWW-Authenticate: Basic`. If `basic_auth_users` and `--auth-filter` are both set, the two fight over the single `Authorization` header: basic-auth credentials give 401 from the filter, and bearer credentials give 401 from the web layer. The configuration is unusable in Go too; do not try to make it work.
7. **`Bearer a b` authenticates with token `a`.** A double space after `Bearer` gives a 401.
8. **NoOpinion from the SAR is a 403**, not a pass-through.
9. **A build failure degrades to 503 per endpoint** instead of exiting. On telemetry, pprof then disappears (404 through the landing handler), rather than returning 503.
