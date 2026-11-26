//! The `/metrics` handler of the metrics port (Go: `pkg/metricshandler`).

use std::collections::HashSet;
use std::io::Write as _;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ksm_core::store::{MetricsWriter, sanitize_headers};
use parking_lot::RwLock;

pub const TEXT_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The writers currently served; `None` until sharding is configured.
#[derive(Default)]
pub struct MetricsHandler {
    writers: RwLock<Option<Arc<Vec<Arc<MetricsWriter>>>>>,
    pub enable_gzip: bool,
}

/// The negotiated exposition format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    OpenMetrics(&'static str),
}

impl Format {
    pub fn content_type(self) -> String {
        match self {
            Format::Text => TEXT_CONTENT_TYPE.to_owned(),
            Format::OpenMetrics(v) => {
                format!(
                    "application/openmetrics-text; version={v}; charset=utf-8; escaping=underscores"
                )
            }
        }
    }
}

struct Accept<'a> {
    media: &'a str,
    q: f64,
    version: Option<&'a str>,
    order: usize,
}

/// `expfmt.NegotiateAccept` with kube-state-metrics' format list; anything but
/// OpenMetrics is served as text.
pub fn negotiate(accept: Option<&str>) -> Format {
    let Some(accept) = accept else {
        return Format::Text;
    };
    let mut clauses: Vec<Accept> = Vec::new();
    for (order, part) in accept.split(',').enumerate() {
        let mut params = part.split(';').map(str::trim);
        let media = params.next().unwrap_or("");
        let mut q = 1.0;
        let mut version = None;
        for p in params {
            if let Some((k, v)) = p.split_once('=') {
                match k.trim() {
                    "q" => q = v.trim().parse().unwrap_or(0.0),
                    "version" => version = Some(v.trim()),
                    _ => {}
                }
            }
        }
        clauses.push(Accept {
            media,
            q,
            version,
            order,
        });
    }
    // goautoneg: by q, then specificity, then original order.
    let specificity = |m: &str| match m {
        "*/*" => 0,
        m if m.ends_with("/*") => 1,
        _ => 2,
    };
    clauses.sort_by(|a, b| {
        b.q.total_cmp(&a.q)
            .then(specificity(b.media).cmp(&specificity(a.media)))
            .then(a.order.cmp(&b.order))
    });
    for c in &clauses {
        if c.q <= 0.0 {
            continue;
        }
        match c.media {
            "application/vnd.google.protobuf" => return Format::Text,
            "text/plain" if matches!(c.version, None | Some("0.0.4")) => return Format::Text,
            "application/openmetrics-text" => match c.version {
                None | Some("0.0.1") => return Format::OpenMetrics("0.0.1"),
                Some("1.0.0") => return Format::OpenMetrics("1.0.0"),
                _ => {}
            },
            _ => {}
        }
    }
    Format::Text
}

fn parse_resources(values: &[&str]) -> Option<HashSet<String>> {
    if values.is_empty() {
        return None;
    }
    Some(
        values
            .iter()
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// Decodes `%XX` and `+` in a query value.
fn query_unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl MetricsHandler {
    pub fn new(enable_gzip: bool) -> Self {
        MetricsHandler {
            writers: RwLock::new(None),
            enable_gzip,
        }
    }

    /// Replaces the writers served (also marks sharding as configured).
    pub fn set_writers(&self, writers: Vec<Arc<MetricsWriter>>) {
        *self.writers.write() = Some(Arc::new(writers));
    }


    /// Serves one scrape.
    pub async fn serve(self: Arc<Self>, query: Option<String>, headers: HeaderMap) -> Response {
        let writers = self.writers.read().clone();
        let Some(writers) = writers else {
            return plain_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "metrics are not available yet: sharding is not configured",
            );
        };
        let format = negotiate(headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()));
        let gzip = self.enable_gzip
            && headers
                .get(header::ACCEPT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(',')
                        .map(str::trim)
                        .any(|p| p == "gzip" || p.starts_with("gzip;"))
                });
        let mut requested = Vec::new();
        let mut excluded = Vec::new();
        for pair in query.as_deref().unwrap_or("").split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match query_unescape(k).as_str() {
                "resources" => requested.push(query_unescape(v)),
                "exclude_resources" => excluded.push(query_unescape(v)),
                _ => {}
            }
        }
        let requested = parse_resources(&requested.iter().map(String::as_str).collect::<Vec<_>>());
        let excluded = parse_resources(&excluded.iter().map(String::as_str).collect::<Vec<_>>());
        let body = tokio::task::spawn_blocking(move || {
            let active: Vec<&MetricsWriter> = writers
                .iter()
                .map(Arc::as_ref)
                .filter(|w| {
                    requested
                        .as_ref()
                        .is_none_or(|r| r.contains(&w.resource_name))
                })
                .filter(|w| {
                    excluded
                        .as_ref()
                        .is_none_or(|e| !e.contains(&w.resource_name))
                })
                .collect();
            let sanitized = sanitize_headers(format == Format::Text, &active);
            let mut out = Vec::with_capacity(64 * 1024);
            for (w, h) in active.iter().zip(&sanitized) {
                w.write_all(h, &mut out);
            }
            if matches!(format, Format::OpenMetrics(_)) {
                out.extend_from_slice(b"# EOF\n");
            }
            if gzip {
                let mut enc = flate2::write::GzEncoder::new(
                    Vec::with_capacity(out.len() / 8),
                    flate2::Compression::default(),
                );
                if enc.write_all(&out).is_ok()
                    && let Ok(z) = enc.finish()
                {
                    return z;
                }
            }
            out
        })
        .await
        .unwrap_or_default();
        let mut resp = Response::new(Body::from(body));
        if let Ok(ct) = HeaderValue::from_str(&format.content_type()) {
            resp.headers_mut().insert(header::CONTENT_TYPE, ct);
        }
        if gzip {
            resp.headers_mut()
                .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        }
        resp
    }
}

/// `http.Error`: the message plus a newline, as plain text.
pub fn plain_error(code: StatusCode, msg: &str) -> Response {
    (
        code,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        format!("{msg}\n"),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation() {
        assert_eq!(negotiate(None), Format::Text);
        assert_eq!(
            negotiate(Some("application/openmetrics-text")),
            Format::OpenMetrics("0.0.1")
        );
        assert_eq!(
            negotiate(Some("application/openmetrics-text; version=1.0.0")),
            Format::OpenMetrics("1.0.0")
        );
        assert_eq!(
            negotiate(Some(
                "application/openmetrics-text;version=0.0.1;q=0.5,text/plain;version=0.0.4;q=0.4"
            )),
            Format::OpenMetrics("0.0.1")
        );
        assert_eq!(
            negotiate(Some(
                "application/vnd.google.protobuf;proto=io.prometheus.client.MetricFamily;encoding=delimited"
            )),
            Format::Text
        );
        assert_eq!(negotiate(Some("*/*")), Format::Text);
        assert_eq!(
            negotiate(Some("text/plain;q=0.9,application/openmetrics-text;q=1")),
            Format::OpenMetrics("0.0.1")
        );
    }

    #[test]
    fn unescape() {
        assert_eq!(query_unescape("pods%2Cnodes"), "pods,nodes");
        assert_eq!(query_unescape("a+b"), "a b");
        assert_eq!(query_unescape("100%"), "100%");
    }
}
