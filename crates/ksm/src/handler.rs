//! The `/metrics` handler of the metrics port (Go: `pkg/metricshandler`).

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ksm_core::store::{MetricsWriter, sanitize_headers};
use parking_lot::RwLock;

/// Size of the chunks a scrape is streamed in.
const CHUNK: usize = 1 << 20;

pub const TEXT_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The writers currently served; `None` until sharding is configured.
pub struct MetricsHandler {
    writers: RwLock<Option<Arc<Vec<Arc<MetricsWriter>>>>>,
    pub enable_gzip: bool,
}

/// The negotiated exposition format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    /// Version and name escaping scheme.
    OpenMetrics(&'static str, &'static str),
}

impl Format {
    pub fn content_type(self) -> String {
        match self {
            Format::Text => TEXT_CONTENT_TYPE.to_owned(),
            Format::OpenMetrics(v, escaping) => {
                format!(
                    "application/openmetrics-text; version={v}; charset=utf-8; escaping={escaping}"
                )
            }
        }
    }
}

/// One clause of an Accept header (goautoneg's `Accept`).
struct Clause<'a> {
    kind: &'a str,
    sub: &'a str,
    q: f64,
    params: Vec<(&'a str, &'a str)>,
}

impl Clause<'_> {
    fn param(&self, k: &str) -> &str {
        self.params
            .iter()
            .rev()
            .find(|(key, _)| *key == k)
            .map_or("", |(_, v)| v)
    }
}

/// goautoneg's `nextSplitElement`: the text before the first `sep` and the
/// rest after it (empty when there is none).
fn next_split(s: &str, sep: char) -> (&str, &str) {
    s.split_once(sep).unwrap_or((s, ""))
}

/// `goautoneg.ParseAccept`: clauses in its sort order (Go's `sort.Sort`,
/// an insertion sort for the short lists headers have).
fn parse_accept(header: &str) -> Vec<Clause<'_>> {
    let mut out: Vec<Clause> = Vec::new();
    let mut remaining = header;
    while !remaining.is_empty() {
        let part;
        (part, remaining) = next_split(remaining, ',');
        let part = part.trim_matches(' ');
        let (media, mut params) = next_split(part, ';');
        let (kind, rest) = next_split(media, '/');
        let kind = kind.trim_matches(' ');
        let sub = if rest.is_empty() {
            if kind != "*" {
                continue;
            }
            "*"
        } else {
            let (sub, extra) = next_split(rest, '/');
            if !extra.is_empty() {
                continue;
            }
            sub.trim_matches(' ')
        };
        let mut c = Clause {
            kind,
            sub,
            q: 1.0,
            params: Vec::new(),
        };
        while !params.is_empty() {
            let p;
            (p, params) = next_split(params, ';');
            let (k, rest) = next_split(p, '=');
            if rest.is_empty() {
                continue;
            }
            let (v, extra) = next_split(rest, '=');
            if !extra.is_empty() {
                continue;
            }
            let k = k.trim_matches(' ');
            if k == "q" {
                // strconv.ParseFloat(v, 32), untrimmed: a syntax error gives 0.
                c.q = match ksm_core::gostd::parse_float(v) {
                    Ok(f) => f64::from(f as f32),
                    Err(ksm_core::gostd::NumErr::Range) if v.starts_with('-') => f64::NEG_INFINITY,
                    Err(ksm_core::gostd::NumErr::Range) => f64::INFINITY,
                    Err(_) => 0.0,
                };
            } else {
                c.params.push((k, v.trim_matches(' ')));
            }
        }
        out.push(c);
    }
    let less = |a: &Clause, b: &Clause| {
        a.q > b.q || (a.kind != "*" && b.kind == "*") || (a.sub != "*" && b.sub == "*")
    };
    for i in 1..out.len() {
        let mut j = i;
        while j > 0 && less(&out[j], &out[j - 1]) {
            out.swap(j, j - 1);
            j -= 1;
        }
    }
    out
}

/// Kube-state-metrics' negotiable formats, in order: type, subtype and the
/// parameters (charset aside) a clause must carry (`expfmt.matchFormat`).
/// `None` marks the formats served as text.
type Offer = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    Option<&'static str>,
);

const FORMATS: [Offer; 6] = [
    (
        "application",
        "openmetrics-text",
        &[("version", "1.0.0")],
        Some("1.0.0"),
    ),
    (
        "application",
        "openmetrics-text",
        &[("version", "0.0.1")],
        Some("0.0.1"),
    ),
    (
        "application",
        "vnd.google.protobuf",
        &[
            ("proto", "io.prometheus.client.MetricFamily"),
            ("encoding", "delimited"),
        ],
        None,
    ),
    (
        "application",
        "vnd.google.protobuf",
        &[
            ("proto", "io.prometheus.client.MetricFamily"),
            ("encoding", "text"),
        ],
        None,
    ),
    (
        "application",
        "vnd.google.protobuf",
        &[
            ("proto", "io.prometheus.client.MetricFamily"),
            ("encoding", "compact-text"),
        ],
        None,
    ),
    ("text", "plain", &[("version", "0.0.4")], None),
];

/// `expfmt.NegotiateAccept` with kube-state-metrics' formats; anything but
/// OpenMetrics is served as text. OpenMetrics carries the escaping scheme
/// the matching clause asked for (`escapingFor`), underscores otherwise.
pub fn negotiate(accept: Option<&str>) -> Format {
    for c in parse_accept(accept.unwrap_or("")) {
        for (kind, sub, params, om) in FORMATS {
            if (c.kind != "*" && c.kind != kind) || (c.sub != "*" && c.sub != sub) {
                continue;
            }
            let version = match (c.param("version"), kind, sub) {
                ("", "application", "openmetrics-text") if c.kind == kind && c.sub == sub => {
                    "0.0.1"
                }
                ("", "text", "plain") if c.kind == kind && c.sub == sub => "0.0.4",
                (v, ..) => v,
            };
            let matched = params.iter().all(|(k, v)| {
                let got = if *k == "version" { version } else { c.param(k) };
                got == *v
            });
            if !matched {
                continue;
            }
            return match om {
                Some(v) => Format::OpenMetrics(v, escaping(&c)),
                None => Format::Text,
            };
        }
    }
    Format::Text
}

/// `expfmt.NegotiateAccept` with promhttp's default formats (protobuf
/// delimited, text and compact text; text 0.0.4; text 1.0.0), for the
/// telemetry port. Protobuf is answered with text 0.0.4.
pub fn negotiate_telemetry(accept: Option<&str>) -> String {
    const PROTO: &str = "io.prometheus.client.MetricFamily";
    for c in parse_accept(accept.unwrap_or("")) {
        let kind_ok =
            |k: &str, s: &str| (c.kind == "*" || c.kind == k) && (c.sub == "*" || c.sub == s);
        if kind_ok("application", "vnd.google.protobuf")
            && c.param("proto") == PROTO
            && matches!(c.param("encoding"), "delimited" | "text" | "compact-text")
        {
            return format!("{TEXT_CONTENT_TYPE}; escaping={}", escaping(&c));
        }
        if kind_ok("text", "plain") {
            let exact = c.kind == "text" && c.sub == "plain";
            let version = match c.param("version") {
                "" if exact => "0.0.4",
                v => v,
            };
            if version == "0.0.4" || version == "1.0.0" {
                return format!(
                    "text/plain; version={version}; charset=utf-8; escaping={}",
                    escaping(&c)
                );
            }
        }
    }
    format!("{TEXT_CONTENT_TYPE}; escaping=underscores")
}

/// `escapingFor`: the scheme a clause asked for, underscores otherwise.
fn escaping(c: &Clause) -> &'static str {
    match c.param("escaping") {
        "allow-utf-8" => "allow-utf-8",
        "dots" => "dots",
        "values" => "values",
        _ => "underscores",
    }
}

/// promhttp's content-encoding choice (gddo `NegotiateContentEncoding` over
/// identity and gzip): gzip when it has the highest q of the two.
pub fn accepts_gzip_promhttp(headers: &HeaderMap) -> bool {
    let mut best = ("identity", -1.0f64);
    let specs = accept_encoding_specs(headers);
    for offer in ["identity", "gzip"] {
        for (value, q) in &specs {
            if *q > best.1 && (value == "*" || value == offer) {
                best = (offer, *q);
            }
        }
    }
    best.0 == "gzip" && best.1 != 0.0
}

/// gddo's `header.ParseAccept` for Accept-Encoding: `(token, q)` pairs, a
/// malformed element ends its header line.
fn accept_encoding_specs(headers: &HeaderMap) -> Vec<(String, f64)> {
    let token = |b: u8| b > 31 && b < 127 && !b" \t\"(),/:;<=>?@[]\\{}".contains(&b);
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    let skip = |s: &[u8]| -> usize { s.iter().take_while(|b| space(**b)).count() };
    let mut specs = Vec::new();
    for line in headers.get_all(header::ACCEPT_ENCODING) {
        let mut s = line.as_bytes();
        loop {
            let n = s.iter().take_while(|b| token(**b) || **b == b'/').count();
            if n == 0 {
                break;
            }
            let value = String::from_utf8_lossy(&s[..n]).into_owned();
            s = &s[n..];
            s = &s[skip(s)..];
            let mut q = 1.0;
            if let Some(rest) = s.strip_prefix(b";") {
                let rest = &rest[skip(rest)..];
                let Some(rest) = rest.strip_prefix(b"q=") else {
                    break;
                };
                let (v, used) = expect_quality(rest);
                if v < 0.0 {
                    break;
                }
                q = v;
                s = &rest[used..];
            }
            specs.push((value, q));
            s = &s[skip(s)..];
            let Some(rest) = s.strip_prefix(b",") else {
                break;
            };
            s = &rest[skip(rest)..];
        }
    }
    specs
}

/// gddo's `expectQuality`: `0` or `1`, optionally `.` and digits; the value
/// and the bytes used, or -1 when malformed.
fn expect_quality(s: &[u8]) -> (f64, usize) {
    let q = match s.first() {
        Some(b'0') => 0.0,
        Some(b'1') => 1.0,
        _ => return (-1.0, 0),
    };
    if s.get(1) != Some(&b'.') {
        return (q, 1);
    }
    let digits = s[2..].iter().take_while(|b| b.is_ascii_digit()).count();
    let (mut n, mut d) = (0i64, 1i64);
    for b in &s[2..2 + digits] {
        n = n.wrapping_mul(10).wrapping_add(i64::from(b - b'0'));
        d = d.wrapping_mul(10);
    }
    (q + n as f64 / d as f64, 2 + digits)
}

fn parse_resources(values: &[String]) -> Option<HashSet<String>> {
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

/// `url.QueryUnescape`: decodes `%XX` and `+`; `None` for a bad escape.
fn query_unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = |c: u8| (c as char).to_digit(16);
                let hi = b.get(i + 1).copied().and_then(hex)?;
                let lo = b.get(i + 2).copied().and_then(hex)?;
                out.push((hi * 16 + lo) as u8);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// The values of `key` in a query (`url.ParseQuery`, which skips pairs
/// holding a `;` or a bad escape).
pub fn query_values(query: &str, key: &str) -> Vec<String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty() && !pair.contains(';'))
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let (k, v) = (query_unescape(k)?, query_unescape(v)?);
            (k == key).then_some(v)
        })
        .collect()
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
        let query = query.unwrap_or_default();
        let requested = query_values(&query, "resources");
        let excluded = query_values(&query, "exclude_resources");
        let requested = parse_resources(&requested);
        let excluded = parse_resources(&excluded);
        // Stream the body in chunks: a large cluster's output is hundreds of
        // megabytes, which should not be held in memory at once.
        let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<bytes::Bytes>>(4);
        let panic_tx = tx.clone();
        let render = tokio::task::spawn_blocking(move || {
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
            // A failed send means the client went away: stop rendering and
            // compressing instead of finishing a body nobody reads.
            let gz_tx = tx.clone();
            let mut gz = gzip.then(|| {
                crate::pgzip::ParallelGzip::new(
                    flate2::Compression::default(),
                    rayon::current_num_threads(),
                    move |data: Vec<u8>| {
                        gz_tx
                            .blocking_send(Ok(bytes::Bytes::from(data)))
                            .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
                    },
                )
            });
            let mut send = |buf: &mut Vec<u8>| {
                let data = std::mem::replace(buf, Vec::with_capacity(CHUNK + CHUNK / 4));
                let sent = match gz.as_mut() {
                    Some(enc) => enc.push(data).is_ok(),
                    None => tx.blocking_send(Ok(bytes::Bytes::from(data))).is_ok(),
                };
                if sent {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(())
                }
            };
            let mut out = Vec::with_capacity(CHUNK + CHUNK / 4);
            let mut flow = ControlFlow::Continue(());
            for (w, h) in active.iter().zip(&sanitized) {
                flow = w.write_all_chunked(h, &mut out, CHUNK, &mut send);
                if flow.is_break() {
                    break;
                }
            }
            if flow.is_continue() {
                if matches!(format, Format::OpenMetrics(..)) {
                    out.extend_from_slice(b"# EOF\n");
                }
                flow = send(&mut out);
            }
            drop(send);
            // After a break the stream stays unterminated, so a client still
            // reading sees an error rather than a truncated body.
            if let Some(enc) = gz
                && flow.is_continue()
            {
                let _ = enc.finish();
            }
        });
        // A panic while rendering ends the body with an error, so the client
        // sees a broken response rather than a short one (Go's net/http aborts
        // the connection when a handler panics).
        tokio::spawn(async move {
            if render.await.is_err() {
                let _ = panic_tx
                    .send(Err(std::io::Error::other("rendering metrics failed")))
                    .await;
            }
        });
        let stream =
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|b| (b, rx)) });
        let mut resp = Response::new(Body::from_stream(stream));
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

    /// Recorded from Go: `expfmt.NegotiateAccept` with kube-state-metrics'
    /// formats (common v0.74.0), non-OpenMetrics turned into text.
    #[test]
    fn negotiation_matches_go() {
        let cases: &[(&str, &str)] = &[
            ("", "text/plain; version=0.0.4; charset=utf-8"),
            ("*/*", "text/plain; version=0.0.4; charset=utf-8"),
            (
                "application/openmetrics-text",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
            (
                "application/openmetrics-text; version=1.0.0",
                "application/openmetrics-text; version=1.0.0; charset=utf-8; escaping=underscores",
            ),
            (
                "application/openmetrics-text;version=1.0.0;escaping=allow-utf-8;q=0.6,application/openmetrics-text;version=0.0.1;q=0.5,text/plain;version=1.0.0;escaping=allow-utf-8;q=0.4,text/plain;version=0.0.4;q=0.3,*/*;q=0.2",
                "application/openmetrics-text; version=1.0.0; charset=utf-8; escaping=allow-utf-8",
            ),
            (
                "application/vnd.google.protobuf;proto=io.prometheus.client.MetricFamily;encoding=delimited;q=0.7,application/openmetrics-text;version=1.0.0;escaping=dots;q=0.5",
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (
                "application/openmetrics-text;q=0",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
            (
                "application/vnd.google.protobuf;q=0.9,application/openmetrics-text;q=0.5",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
            (
                "text/plain;q=0.9,application/openmetrics-text;q=1",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
            (
                "application/openmetrics-text;q=0.5 ,text/plain;q=0.4",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
            (
                "application/openmetrics-text;version=2.0.0",
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            ("application/*", "text/plain; version=0.0.4; charset=utf-8"),
            (
                "application/openmetrics-text;escaping=values",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=values",
            ),
            (
                "application/openmetrics-text;escaping=bogus",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
            ("*", "text/plain; version=0.0.4; charset=utf-8"),
            ("text/", "text/plain; version=0.0.4; charset=utf-8"),
            (
                "application/openmetrics-text/x",
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (
                "application/openmetrics-text;version=1.0.0=;escaping=dots",
                "application/openmetrics-text; version=1.0.0; charset=utf-8; escaping=dots",
            ),
            (
                "application/openmetrics-text;version=",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
            (
                "text/plain;version=1.0.0,application/openmetrics-text;q=0.1",
                "application/openmetrics-text; version=0.0.1; charset=utf-8; escaping=underscores",
            ),
        ];
        for (accept, want) in cases {
            let got = negotiate((!accept.is_empty()).then_some(*accept)).content_type();
            assert_eq!(&got, want, "Accept: {accept}");
        }
    }

    /// Recorded from Go's promhttp.HandlerFor (client_golang v1.25.0).
    #[test]
    fn telemetry_negotiation_matches_promhttp() {
        let accepts: &[(&str, &str)] = &[
            (
                "",
                "text/plain; version=0.0.4; charset=utf-8; escaping=underscores",
            ),
            (
                "*/*",
                "text/plain; version=0.0.4; charset=utf-8; escaping=underscores",
            ),
            (
                "text/plain;version=1.0.0;escaping=allow-utf-8",
                "text/plain; version=1.0.0; charset=utf-8; escaping=allow-utf-8",
            ),
            (
                "text/plain;version=1.0.0",
                "text/plain; version=1.0.0; charset=utf-8; escaping=underscores",
            ),
            (
                "application/openmetrics-text;version=1.0.0;escaping=allow-utf-8;q=0.6,application/openmetrics-text;version=0.0.1;q=0.5,text/plain;version=1.0.0;escaping=allow-utf-8;q=0.4,text/plain;version=0.0.4;q=0.3,*/*;q=0.2",
                "text/plain; version=1.0.0; charset=utf-8; escaping=allow-utf-8",
            ),
            (
                "text/*",
                "text/plain; version=0.0.4; charset=utf-8; escaping=underscores",
            ),
            (
                "text/plain;version=2.0.0",
                "text/plain; version=0.0.4; charset=utf-8; escaping=underscores",
            ),
            (
                "text/plain;escaping=dots",
                "text/plain; version=0.0.4; charset=utf-8; escaping=dots",
            ),
        ];
        for (accept, want) in accepts {
            assert_eq!(
                &negotiate_telemetry((!accept.is_empty()).then_some(*accept)),
                want,
                "Accept: {accept}"
            );
        }
        let encodings: &[(&str, bool)] = &[
            ("", false),
            ("gzip", true),
            ("gzip;q=0", false),
            ("identity;q=1, gzip;q=0.5", false),
            ("*", false),
            ("*;q=0", false),
            ("deflate, gzip;q=0.9", true),
            ("gzip;q=0.5, identity;q=0.5", false),
            ("x-gzip", false),
            ("gzip ;q=1.0", true),
            ("gzip;q=", false),
            ("GZIP", false),
        ];
        for (enc, want) in encodings {
            let mut h = HeaderMap::new();
            if !enc.is_empty() {
                h.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(enc).unwrap());
            }
            assert_eq!(accepts_gzip_promhttp(&h), *want, "Accept-Encoding: {enc}");
        }
    }

    #[test]
    fn query_like_go() {
        assert_eq!(
            query_unescape("pods%2Cnodes").as_deref(),
            Some("pods,nodes")
        );
        assert_eq!(query_unescape("a+b").as_deref(), Some("a b"));
        assert_eq!(query_unescape("100%"), None);
        assert_eq!(query_unescape("%+1"), None);
        assert_eq!(query_unescape("%zz"), None);
        assert_eq!(
            query_values("resources=pods&resources=nodes", "resources"),
            ["pods", "nodes"]
        );
        // url.ParseQuery drops pairs with a ';' or a bad escape.
        assert!(query_values("resources=pods;x=1", "resources").is_empty());
        assert!(query_values("resources=pods%zz", "resources").is_empty());
        assert_eq!(query_values("x=%zz&resources=pods", "resources"), ["pods"]);
    }
}
