//! `--auth-filter`: controller-runtime's
//! `filters.WithAuthenticationAndAuthorization` (TokenReview, then a
//! non-resource SubjectAccessReview), with apiserver's caches. See
//! docs/port/research/auth-filter.md.
//!
//! Not ported: the WebSocket subprotocol token (`Sec-WebSocket-Protocol`),
//! which only matters for WebSocket upgrades a scrape endpoint never sees.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::handler::plain_error;

/// Token cache TTL, for authenticated and unauthenticated results.
const TOKEN_TTL: Duration = Duration::from_secs(60);
/// SubjectAccessReview cache TTLs.
const ALLOW_TTL: Duration = Duration::from_secs(300);
const DENY_TTL: Duration = Duration::from_secs(30);
/// SubjectAccessReview LRU size.
const SAR_CACHE_SIZE: usize = 8192;
/// TokenReview call timeout.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);

/// An authenticated user (`user.Info`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserInfo {
    pub name: String,
    pub uid: String,
    pub groups: Vec<String>,
    pub extra: BTreeMap<String, Vec<String>>,
}

/// The decision of a SubjectAccessReview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Allow,
    Deny,
}

/// A cached TokenReview result: expiry and the user (None: not authenticated).
type TokenEntry = (Instant, Option<UserInfo>);

/// One filter instance (Go builds one per server, each with its caches).
pub struct AuthFilter {
    client: kube::Client,
    tokens: Mutex<HashMap<[u8; 32], TokenEntry>>,
    sar: Mutex<HashMap<String, (Instant, Decision, u64)>>,
    tick: std::sync::atomic::AtomicU64,
}

/// The bearer token of the `Authorization` header (`bearertoken`), or
/// `None` when the request has none to offer.
pub fn bearer_token(v: Option<&str>) -> Option<String> {
    let v = v?.trim_matches(|c: char| c.is_ascii_whitespace());
    let mut parts = v.splitn(3, ' ');
    let scheme = parts.next()?;
    let token = parts.next()?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    Some(token.to_owned())
}

/// `AuthenticatedGroupAdder`.
fn add_authenticated_group(u: &mut UserInfo) {
    if u.name == "system:anonymous" {
        return;
    }
    if u.groups
        .iter()
        .any(|g| g == "system:authenticated" || g == "system:unauthenticated")
    {
        return;
    }
    u.groups.push("system:authenticated".to_owned());
}

/// apiserver's webhook retry: only transient errors.
fn transient(e: &kube::Error) -> bool {
    match e {
        kube::Error::Api(s) => matches!(s.code, 429 | 500 | 504),
        kube::Error::HyperError(_) | kube::Error::Service(_) => true,
        _ => false,
    }
}

/// The SubjectAccessReview spec, as Go marshals it (omitempty fields).
pub fn sar_spec(u: &UserInfo, verb: &str, path: &str) -> Value {
    let mut spec = serde_json::Map::new();
    spec.insert(
        "nonResourceAttributes".into(),
        json!({"path": path, "verb": verb}),
    );
    if !u.name.is_empty() {
        spec.insert("user".into(), json!(u.name));
    }
    if !u.groups.is_empty() {
        spec.insert("groups".into(), json!(u.groups));
    }
    if !u.extra.is_empty() {
        spec.insert("extra".into(), json!(u.extra));
    }
    if !u.uid.is_empty() {
        spec.insert("uid".into(), json!(u.uid));
    }
    Value::Object(spec)
}

impl AuthFilter {
    pub fn new(client: kube::Client) -> Self {
        AuthFilter {
            client,
            tokens: Mutex::new(HashMap::new()),
            sar: Mutex::new(HashMap::new()),
            tick: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// POSTs a review with apiserver's webhook backoff (500 ms × 1.5, jitter
    /// 0.2, 5 steps) on transient errors.
    async fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        let mut delay = Duration::from_millis(500);
        let mut last = String::new();
        for attempt in 0..5 {
            if attempt > 0 {
                let jitter = 1.0 + 0.2 * f64::from(u32::from(rand_byte())) / 255.0;
                tokio::time::sleep(delay.mul_f64(jitter)).await;
                delay = delay.mul_f64(1.5);
            }
            let req = http::Request::post(path)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, */*")
                .body(bytes.clone())
                .map_err(|e| e.to_string())?;
            match self.client.request::<Value>(req).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    last = e.to_string();
                    if !transient(&e) {
                        break;
                    }
                }
            }
        }
        Err(last)
    }

    /// TokenReview with the 1 minute cache; errors are not cached.
    async fn authenticate(&self, token: &str) -> Result<Option<UserInfo>, String> {
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let now = Instant::now();
        if let Some((exp, u)) = self.tokens.lock().get(&key)
            && *exp > now
        {
            return Ok(u.clone());
        }
        let body = json!({"kind": "TokenReview", "apiVersion": "authentication.k8s.io/v1",
            "metadata": {}, "spec": {"token": token}, "status": {"user": {}}});
        let r = tokio::time::timeout(
            TOKEN_TIMEOUT,
            self.post("/apis/authentication.k8s.io/v1/tokenreviews", &body),
        )
        .await
        .map_err(|_| "context deadline exceeded".to_owned())??;
        let status = &r["status"];
        if status["authenticated"].as_bool() != Some(true) {
            if let Some(e) = status["error"].as_str().filter(|e| !e.is_empty()) {
                return Err(e.to_owned());
            }
            self.cache_token(key, None);
            return Ok(None);
        }
        let u = &status["user"];
        let strs = |v: &Value| -> Vec<String> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        let user = UserInfo {
            name: u["username"].as_str().unwrap_or("").to_owned(),
            uid: u["uid"].as_str().unwrap_or("").to_owned(),
            groups: strs(&u["groups"]),
            extra: u["extra"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), strs(v)))
                .collect(),
        };
        self.cache_token(key, Some(user.clone()));
        Ok(Some(user))
    }

    fn cache_token(&self, key: [u8; 32], u: Option<UserInfo>) {
        let now = Instant::now();
        let mut c = self.tokens.lock();
        if c.len() >= 4096 {
            c.retain(|_, (exp, _)| *exp > now);
        }
        c.insert(key, (now + TOKEN_TTL, u));
    }

    /// SubjectAccessReview with the 5 m allow / 30 s deny LRU cache.
    async fn authorize(&self, u: &UserInfo, verb: &str, path: &str) -> Result<Decision, String> {
        let spec = sar_spec(u, verb, path);
        let key = spec.to_string();
        let now = Instant::now();
        let tick = self.tick.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut c = self.sar.lock();
            if let Some((exp, d, used)) = c.get_mut(&key)
                && *exp > now
            {
                *used = tick;
                return Ok(*d);
            }
        }
        let body = json!({"kind": "SubjectAccessReview", "apiVersion": "authorization.k8s.io/v1",
            "metadata": {}, "spec": spec, "status": {"allowed": false}});
        let r = self
            .post("/apis/authorization.k8s.io/v1/subjectaccessreviews", &body)
            .await?;
        let allowed = r["status"]["allowed"].as_bool().unwrap_or(false);
        let denied = r["status"]["denied"].as_bool().unwrap_or(false);
        let (d, ttl) = if denied && allowed {
            return Err(format!(
                "an error on the server ({}) has prevented the request from succeeding",
                "allowed and denied"
            ));
        } else if allowed && !denied {
            (Decision::Allow, ALLOW_TTL)
        } else {
            // Denied and NoOpinion both end as 403.
            (Decision::Deny, DENY_TTL)
        };
        let mut c = self.sar.lock();
        if c.len() >= SAR_CACHE_SIZE
            && let Some(oldest) = c
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(k, _)| k.clone())
        {
            c.remove(&oldest);
        }
        c.insert(key, (now + ttl, d, tick));
        Ok(d)
    }
}

/// A cheap pseudo-random byte for the retry jitter.
fn rand_byte() -> u8 {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    n.to_le_bytes()[0]
}

/// The filter as an axum middleware.
pub async fn auth_middleware(
    State(f): State<Arc<AuthFilter>>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = bearer_token(
        req.headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
    );
    let Some(token) = token else {
        tracing::debug!("Authentication failed: no credentials");
        return plain_error(StatusCode::UNAUTHORIZED, "Unauthorized");
    };
    let user = match f.authenticate(&token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            tracing::error!(err = "invalid bearer token", "Authentication failed");
            return plain_error(StatusCode::INTERNAL_SERVER_ERROR, "Authentication failed");
        }
        Err(e) => {
            tracing::error!(err = %e, "Authentication failed");
            return plain_error(StatusCode::INTERNAL_SERVER_ERROR, "Authentication failed");
        }
    };
    let mut user = user;
    add_authenticated_group(&mut user);
    let verb = req.method().as_str().to_lowercase();
    let path = req.uri().path().to_owned();
    match f.authorize(&user, &verb, &path).await {
        Ok(Decision::Allow) => {
            req.headers_mut().remove(header::AUTHORIZATION);
            next.run(req).await
        }
        Ok(Decision::Deny) => {
            tracing::debug!(user = %user.name, verb, path, "Authorization denied for user");
            plain_error(
                StatusCode::FORBIDDEN,
                &format!("Authorization denied for user {}", user.name),
            )
        }
        Err(e) => {
            tracing::error!(err = %e, user = %user.name, "Authorization failed");
            plain_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Authorization for user {} failed", user.name),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert_eq!(bearer_token(Some("Bearer abc")), Some("abc".into()));
        assert_eq!(bearer_token(Some("  BEARER abc  ")), Some("abc".into()));
        assert_eq!(bearer_token(Some("Bearer a b")), Some("a".into()));
        assert_eq!(bearer_token(Some("Bearer  tok")), None);
        assert_eq!(bearer_token(Some("Basic abc")), None);
        assert_eq!(bearer_token(Some("Bearer")), None);
        assert_eq!(bearer_token(None), None);
    }

    #[test]
    fn groups_and_spec() {
        let mut u = UserInfo {
            name: "alice".into(),
            uid: "u1".into(),
            groups: vec!["g1".into()],
            extra: [("k".to_owned(), vec!["v".to_owned()])].into(),
        };
        add_authenticated_group(&mut u);
        assert_eq!(u.groups, ["g1", "system:authenticated"]);
        add_authenticated_group(&mut u);
        assert_eq!(u.groups.len(), 2);
        assert_eq!(
            sar_spec(&u, "get", "/metrics").to_string(),
            r#"{"extra":{"k":["v"]},"groups":["g1","system:authenticated"],"nonResourceAttributes":{"path":"/metrics","verb":"get"},"uid":"u1","user":"alice"}"#
        );
        let mut anon = UserInfo {
            name: "system:anonymous".into(),
            ..UserInfo::default()
        };
        add_authenticated_group(&mut anon);
        assert!(anon.groups.is_empty());
    }
}
