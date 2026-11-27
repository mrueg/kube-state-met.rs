//! `--tls-config`: the exporter-toolkit web configuration (Go:
//! `github.com/prometheus/exporter-toolkit/web` v0.21.0, `ListenAndServe`).
//! See docs/port/research/web-config.md.
//!
//! The file is read again on every request (headers, rate limit, basic
//! auth) and on every TLS handshake (certificates, client CAs), as Go does.
//! rustls has no TLS 1.0/1.1, no CBC cipher suites and no P-521: such
//! settings are accepted and dropped (DECISIONS.md ADR-013).

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use parking_lot::Mutex;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{Acceptor, WebPkiClientVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig};
use serde::{Deserialize, Deserializer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::handler::plain_error;
use crate::server::go_io_error;

/// Go's TLS version numbers.
const TLS10: u16 = 0x0301;
const TLS11: u16 = 0x0302;
const TLS12: u16 = 0x0303;
const TLS13: u16 = 0x0304;

/// The bcrypt hash of "fakepassword", checked for unknown users.
const FAKE_HASH: &str = "$2y$10$QOauhQNbBCuQDKes6eFzPeMqBSjb7Mr5DUmpZ/VcEd00UAV/LDeSi";

/// `extraHTTPHeaders`.
const EXTRA_HEADERS: &[(&str, &[&str])] = &[
    ("Content-Security-Policy", &[]),
    ("Strict-Transport-Security", &[]),
    ("X-Content-Type-Options", &["nosniff"]),
    ("X-Frame-Options", &["deny", "sameorigin"]),
    ("X-XSS-Protection", &[]),
];

/// Go's `tls.CipherSuites()` names (TLS 1.3 suites are not configurable).
const CIPHERS: &[&str] = &[
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_CHACHA20_POLY1305_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA",
    "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
    "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
];

const CURVES: &[&str] = &["CurveP256", "CurveP384", "CurveP521", "X25519"];

fn custom<E: serde::de::Error>(msg: String) -> E {
    E::custom(format!("\u{1}{msg}"))
}

fn cipher_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v: Option<Vec<String>> = Option::deserialize(d)?;
    let v = v.unwrap_or_default();
    for c in &v {
        if !CIPHERS.contains(&c.as_str()) {
            return Err(custom(format!("unknown cipher: {c}")));
        }
    }
    Ok(v)
}

fn curve_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v: Option<Vec<String>> = Option::deserialize(d)?;
    let v = v.unwrap_or_default();
    for c in &v {
        if !CURVES.contains(&c.as_str()) {
            return Err(custom(format!("unknown curve: {c}")));
        }
    }
    Ok(v)
}

fn tls_version<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    let v: Option<String> = Option::deserialize(d)?;
    match v.as_deref() {
        None => Ok(None),
        Some("TLS13") => Ok(Some(TLS13)),
        Some("TLS12") => Ok(Some(TLS12)),
        Some("TLS11") => Ok(Some(TLS11)),
        Some("TLS10") => Ok(Some(TLS10)),
        Some(s) => Err(custom(format!("unknown TLS version: {s}"))),
    }
}

/// `time.Duration` as yaml.v2 decodes it: a Go duration string or nanoseconds.
fn go_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Dur {
        N(i64),
        S(String),
    }
    match Option::<Dur>::deserialize(d)? {
        None => Ok(None),
        Some(Dur::N(n)) => Ok(Some(n)),
        Some(Dur::S(s)) => crate::options::goparse::parse_duration(&s)
            .map(Some)
            .map_err(|_| {
                custom(format!(
                    "yaml: unmarshal errors:\n  line 0: cannot unmarshal !!str `{s}` into time.Duration"
                ))
            }),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    tls_server_config: Option<RawTls>,
    http_server_config: Option<RawHttp>,
    rate_limit: Option<RawRate>,
    basic_auth_users: Option<BTreeMap<String, Option<String>>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawTls {
    cert: Option<String>,
    key: Option<String>,
    client_ca: Option<String>,
    cert_file: Option<String>,
    key_file: Option<String>,
    client_auth_type: Option<String>,
    client_ca_file: Option<String>,
    #[serde(default, deserialize_with = "cipher_list")]
    cipher_suites: Vec<String>,
    #[serde(default, deserialize_with = "curve_list")]
    curve_preferences: Vec<String>,
    #[serde(default, deserialize_with = "tls_version")]
    min_version: Option<u16>,
    #[serde(default, deserialize_with = "tls_version")]
    max_version: Option<u16>,
    #[expect(dead_code, reason = "parsed and ignored, as in Go")]
    prefer_server_cipher_suites: Option<bool>,
    client_allowed_sans: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawHttp {
    http2: Option<bool>,
    headers: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRate {
    burst: Option<i64>,
    #[serde(default, deserialize_with = "go_duration")]
    interval: Option<i64>,
}

/// `TLSConfig`, relative paths joined to the config file's directory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TlsConfig {
    pub cert: String,
    pub key: String,
    pub client_ca_text: String,
    pub cert_path: String,
    pub key_path: String,
    pub client_auth: String,
    pub client_cas: String,
    pub cipher_suites: Vec<String>,
    pub curve_preferences: Vec<String>,
    pub min_version: u16,
    pub max_version: u16,
    pub client_allowed_sans: Option<Vec<String>>,
}

impl TlsConfig {
    /// `IsEnabled`.
    pub fn is_enabled(&self) -> bool {
        !self.cert_path.is_empty()
            || !self.cert.is_empty()
            || !self.key_path.is_empty()
            || !self.key.is_empty()
            || !self.client_cas.is_empty()
            || !self.client_ca_text.is_empty()
            || !self.client_auth.is_empty()
    }
}

/// `RateLimiterConfig`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateConfig {
    pub burst: i64,
    /// Nanoseconds.
    pub interval: i64,
}

/// `Config`.
#[derive(Clone, Debug, Default)]
pub struct WebConfig {
    pub tls: TlsConfig,
    pub http2: bool,
    pub headers: BTreeMap<String, String>,
    pub rate: RateConfig,
    pub users: BTreeMap<String, String>,
}

/// Go's yaml.v2 error text for a strict decoding error.
fn yaml_error(e: &serde_yaml_ng::Error) -> String {
    let msg = e.to_string();
    let line = e.location().map_or(0, |l| l.line());
    if let Some(i) = msg.find('\u{1}') {
        let rest = &msg[i + 1..];
        let rest = rest.split(" at line ").next().unwrap_or(rest);
        return rest.replace("line 0:", &format!("line {line}:"));
    }
    if let Some((_, rest)) = msg.split_once("unknown field `") {
        let field = rest.split('`').next().unwrap_or("");
        let ty = if rest.contains("tls_server_config") {
            "web.Config"
        } else if rest.contains("cert_file") {
            "web.TLSConfig"
        } else if rest.contains("http2") {
            "web.HTTPConfig"
        } else {
            "web.RateLimiterConfig"
        };
        return format!(
            "yaml: unmarshal errors:\n  line {line}: field {field} not found in type {ty}"
        );
    }
    let body = msg.split(" at line ").next().unwrap_or(&msg);
    let body = body.rsplit(": invalid type: ").next().unwrap_or(body);
    let body = body.strip_prefix("invalid type: ").unwrap_or(body);
    if let Some((got, expected)) = body.split_once(", expected ") {
        let (tag, value) = match got.split_once(' ') {
            Some(("string", v)) => ("!!str", v.trim_matches('"').to_owned()),
            Some(("integer", v)) => ("!!int", v.trim_matches('`').to_owned()),
            Some(("floating", v)) => (
                "!!float",
                v.trim_start_matches("point ").trim_matches('`').to_owned(),
            ),
            Some(("boolean", v)) => ("!!bool", v.trim_matches('`').to_owned()),
            _ if got == "sequence" => ("!!seq", String::new()),
            _ if got == "map" => ("!!map", String::new()),
            _ => ("!!str", got.to_owned()),
        };
        let into = match expected {
            "struct RawConfig" => "web.Config",
            "struct RawTls" => "web.TLSConfig",
            "struct RawHttp" => "web.HTTPConfig",
            "struct RawRate" => "web.RateLimiterConfig",
            "i64" => "int",
            "a boolean" => "bool",
            "a string" => "string",
            "a sequence" => "[]string",
            "a map" => "map[string]string",
            other => other,
        };
        // yaml.v2 shortens long values in this message.
        let shown = if value.chars().count() > 10 {
            format!("{}...", value.chars().take(7).collect::<String>())
        } else {
            value
        };
        let what = if shown.is_empty() {
            tag.to_owned()
        } else {
            format!("{tag} `{shown}`")
        };
        return format!(
            "yaml: unmarshal errors:\n  line {line}: cannot unmarshal {what} into {into}"
        );
    }
    format!("yaml: unmarshal errors:\n  line {line}: {body}")
}

/// `validateHeaderConfig`.
fn validate_headers(h: &BTreeMap<String, String>) -> Result<(), String> {
    for (k, v) in h {
        let Some((_, allowed)) = EXTRA_HEADERS.iter().find(|(n, _)| n == k) else {
            return Err(format!("HTTP header {k:?} can not be configured"));
        };
        if allowed.is_empty() || allowed.contains(&v.as_str()) {
            continue;
        }
        let list: Vec<String> = allowed.iter().map(|a| format!("{a:?}")).collect();
        return Err(format!(
            "invalid value for {k}. Expected one of: [{}], but got: {v:?}",
            list.join(" ")
        ));
    }
    Ok(())
}

/// `joinDir`.
fn join_dir(dir: &std::path::Path, p: &str) -> String {
    if p.is_empty() || p.starts_with('/') {
        return p.to_owned();
    }
    crate::server::clean_path(&dir.join(p).to_string_lossy())
}

/// `getConfig`: reads and strictly decodes the file.
pub fn get_config(path: &str) -> Result<WebConfig, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("open {path}: {}", go_io_error(&e)))?;
    let text = String::from_utf8_lossy(&bytes);
    let raw: RawConfig = if text.trim().is_empty() {
        RawConfig::default()
    } else {
        Option::<RawConfig>::deserialize(serde_yaml_ng::Deserializer::from_str(&text))
            .map_err(|e| yaml_error(&e))?
            .unwrap_or_default()
    };
    let t = raw.tls_server_config.unwrap_or_default();
    let h = raw.http_server_config.unwrap_or_default();
    let r = raw.rate_limit.unwrap_or_default();
    let dir = std::path::Path::new(path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let dir = if dir.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        dir
    };
    let c = WebConfig {
        tls: TlsConfig {
            cert: t.cert.unwrap_or_default(),
            key: t.key.unwrap_or_default(),
            client_ca_text: t.client_ca.unwrap_or_default(),
            cert_path: join_dir(&dir, &t.cert_file.unwrap_or_default()),
            key_path: join_dir(&dir, &t.key_file.unwrap_or_default()),
            client_auth: t.client_auth_type.unwrap_or_default(),
            client_cas: join_dir(&dir, &t.client_ca_file.unwrap_or_default()),
            cipher_suites: t.cipher_suites,
            curve_preferences: t.curve_preferences,
            min_version: t.min_version.unwrap_or(TLS12),
            max_version: t.max_version.unwrap_or(TLS13),
            client_allowed_sans: t.client_allowed_sans,
        },
        http2: h.http2.unwrap_or(true),
        headers: h.headers.unwrap_or_default(),
        rate: RateConfig {
            burst: r.burst.unwrap_or(0),
            interval: r.interval.unwrap_or(0),
        },
        users: raw
            .basic_auth_users
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, v.unwrap_or_default()))
            .collect(),
    };
    validate_headers(&c.headers)?;
    Ok(c)
}

/// `bcrypt.Cost`'s validation of a hash.
fn bcrypt_cost(hash: &str) -> Result<u32, String> {
    let b = hash.as_bytes();
    if b.len() < 59 {
        return Err("crypto/bcrypt: hashedSecret too short to be a bcrypted password".to_owned());
    }
    if b[0] != b'$' {
        return Err(format!(
            "crypto/bcrypt: bcrypt hashes must start with '$', but hashedSecret started with '{}'",
            char::from(b[0])
        ));
    }
    if b[1] > b'2' {
        return Err(format!(
            "crypto/bcrypt: bcrypt algorithm version '{}' requested is newer than current version '2'",
            char::from(b[1])
        ));
    }
    let n = if b[2] != b'$' { 4 } else { 3 };
    let cost = String::from_utf8_lossy(&b[n..n + 2]).into_owned();
    let c: i64 = cost
        .parse()
        .map_err(|_| format!("strconv.Atoi: parsing {cost:?}: invalid syntax"))?;
    if !(4..=31).contains(&c) {
        return Err(format!(
            "crypto/bcrypt: cost {c} is outside allowed inclusive range 4..31"
        ));
    }
    Ok(u32::try_from(c).unwrap_or(4))
}

/// `validateUsers`.
fn validate_users(path: &str) -> Result<(), String> {
    let c = get_config(path)?;
    for h in c.users.values() {
        bcrypt_cost(h)?;
    }
    Ok(())
}

/// Reads PEM certificates, Go's `X509KeyPair` error texts.
fn load_cert(
    c: &TlsConfig,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), String> {
    let cert_data = if c.cert_path.is_empty() {
        c.cert.as_bytes().to_vec()
    } else {
        std::fs::read(&c.cert_path).map_err(|e| {
            format!(
                "failed to read cert_file ({}): open {}: {}",
                c.cert_path,
                c.cert_path,
                go_io_error(&e)
            )
        })?
    };
    let key_data = if c.key_path.is_empty() {
        c.key.as_bytes().to_vec()
    } else {
        std::fs::read(&c.key_path).map_err(|e| {
            format!(
                "failed to read key_file ({}): open {}: {}",
                c.key_path,
                c.key_path,
                go_io_error(&e)
            )
        })?
    };
    let pair = |m: &str| format!("failed to load X509KeyPair: tls: {m}");
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_data.as_slice())
        .filter_map(Result::ok)
        .collect();
    if certs.is_empty() {
        return Err(pair("failed to find any PEM data in certificate input"));
    }
    let key = rustls_pemfile::private_key(&mut key_data.as_slice())
        .ok()
        .flatten()
        .ok_or_else(|| pair("failed to find any PEM data in key input"))?;
    let provider = rustls::crypto::ring::default_provider();
    rustls::sign::CertifiedKey::from_der(certs.clone(), key.clone_key(), &provider).map_err(
        |e| match e {
            rustls::Error::InconsistentKeys(_) => pair("private key does not match public key"),
            other => pair(&format!("failed to parse private key: {other}")),
        },
    )?;
    Ok((certs, key))
}

/// `ConfigToTLSConfig`'s checks; `Ok(None)` is `errNoTLSConfig`.
pub fn check_tls(c: &TlsConfig) -> Result<Option<()>, String> {
    if !c.is_enabled() {
        return Ok(None);
    }
    if c.cert_path.is_empty() && c.cert.is_empty() {
        return Err("missing one of cert or cert_file".to_owned());
    }
    if c.key_path.is_empty() && c.key.is_empty() {
        return Err("missing one of key or key_file".to_owned());
    }
    load_cert(c)?;
    if !c.client_cas.is_empty() {
        std::fs::read(&c.client_cas)
            .map_err(|e| format!("open {}: {}", c.client_cas, go_io_error(&e)))?;
    }
    match c.client_auth.as_str() {
        "RequestClientCert"
        | "RequireAnyClientCert"
        | "RequireClientCert"
        | "VerifyClientCertIfGiven"
        | "RequireAndVerifyClientCert" => {}
        "" | "NoClientCert" => {
            if !c.client_cas.is_empty() || !c.client_ca_text.is_empty() {
                return Err(
                    "client CA's have been configured without a Client Auth Policy".to_owned(),
                );
            }
        }
        other => return Err(format!("invalid ClientAuth: {other}")),
    }
    Ok(Some(()))
}

/// Go's client authentication modes on rustls (`tls.ClientAuthType`).
#[derive(Debug)]
struct GoClientVerifier {
    /// Chain verification (the `Verify*` modes).
    inner: Option<Arc<dyn ClientCertVerifier>>,
    /// The CA certificates; Go accepts a (self-signed) leaf found among them.
    anchors: Vec<Vec<u8>>,
    mandatory: bool,
    allowed_sans: Option<Vec<String>>,
    provider: Arc<CryptoProvider>,
}

/// The SAN values of a DER certificate, in Go's string forms.
fn san_values(der: &[u8]) -> Result<Vec<String>, String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der)
        .map_err(|e| format!("error parsing client certificate: {e}"))?;
    let mut out = Vec::new();
    if let Ok(Some(ext)) = cert.subject_alternative_name() {
        use x509_parser::extensions::GeneralName;
        for n in &ext.value.general_names {
            match n {
                GeneralName::DNSName(s) | GeneralName::RFC822Name(s) | GeneralName::URI(s) => {
                    out.push((*s).to_owned());
                }
                GeneralName::IPAddress(b) => {
                    if let Ok(a) = <[u8; 4]>::try_from(*b) {
                        out.push(std::net::Ipv4Addr::from(a).to_string());
                    } else if let Ok(a) = <[u8; 16]>::try_from(*b) {
                        let v6 = std::net::Ipv6Addr::from(a);
                        match v6.to_ipv4_mapped() {
                            Some(v4) => out.push(v4.to_string()),
                            None => out.push(v6.to_string()),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

impl ClientCertVerifier for GoClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.as_ref().map_or(&[], |i| i.root_hint_subjects())
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if let Some(i) = &self.inner {
            if self
                .anchors
                .iter()
                .any(|a| a.as_slice() == end_entity.as_ref())
            {
                let secs = i64::try_from(now.as_secs()).unwrap_or(i64::MAX);
                let valid = x509_parser::parse_x509_certificate(end_entity)
                    .ok()
                    .zip(x509_parser::time::ASN1Time::from_timestamp(secs).ok())
                    .is_some_and(|((_, c), t)| c.validity().is_valid_at(t));
                if !valid {
                    return Err(rustls::Error::InvalidCertificate(
                        rustls::CertificateError::Expired,
                    ));
                }
            } else {
                i.verify_client_cert(end_entity, intermediates, now)?;
            }
        }
        if let Some(allowed) = &self.allowed_sans {
            let sans = san_values(end_entity).map_err(rustls::Error::General)?;
            if !sans.iter().any(|s| allowed.contains(s)) {
                return Err(rustls::Error::General(format!(
                    "could not find allowed SANs in client cert, found: [{}]",
                    allowed.join(" ")
                )));
            }
        }
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn cipher_id(name: &str) -> Option<rustls::CipherSuite> {
    use rustls::CipherSuite as C;
    Some(match name {
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256" => C::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384" => C::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256" => C::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384" => C::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256" => {
            C::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
        }
        "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256" => {
            C::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
        }
        _ => return None,
    })
}

fn pem_certs(pem: &[u8]) -> Vec<CertificateDer<'static>> {
    rustls_pemfile::certs(&mut &pem[..])
        .filter_map(Result::ok)
        .collect()
}

/// Builds the rustls configuration of a web config (`ConfigToTLSConfig`
/// plus the ALPN protocols of `http2`).
pub fn rustls_config(c: &WebConfig) -> Result<Arc<ServerConfig>, String> {
    let t = &c.tls;
    check_tls(t)?;
    let (certs, key) = load_cert(t)?;
    let mut provider = rustls::crypto::ring::default_provider();
    if !t.cipher_suites.is_empty() {
        let wanted: Vec<rustls::CipherSuite> = t
            .cipher_suites
            .iter()
            .filter_map(|n| cipher_id(n))
            .collect();
        provider.cipher_suites.retain(|s| match s {
            rustls::SupportedCipherSuite::Tls13(_) => true,
            rustls::SupportedCipherSuite::Tls12(_) => wanted.contains(&s.suite()),
        });
    }
    if !t.curve_preferences.is_empty() {
        use rustls::crypto::ring::kx_group;
        let mut groups: Vec<&'static dyn rustls::crypto::SupportedKxGroup> = Vec::new();
        for n in &t.curve_preferences {
            match n.as_str() {
                "X25519" => groups.push(kx_group::X25519),
                "CurveP256" => groups.push(kx_group::SECP256R1),
                "CurveP384" => groups.push(kx_group::SECP384R1),
                _ => {}
            }
        }
        // Only P-521, which rustls lacks: keep the default groups rather
        // than failing every handshake.
        if !groups.is_empty() {
            provider.kx_groups = groups;
        }
    }
    let mut versions: Vec<&'static rustls::SupportedProtocolVersion> = Vec::new();
    if t.min_version <= TLS12 && TLS12 <= t.max_version {
        versions.push(&rustls::version::TLS12);
    }
    if t.min_version <= TLS13 && TLS13 <= t.max_version {
        versions.push(&rustls::version::TLS13);
    }
    if versions.is_empty() {
        return Err("tls: no supported versions satisfy MinVersion and MaxVersion".to_owned());
    }
    let provider = Arc::new(provider);
    let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&versions)
        .map_err(|e| e.to_string())?;
    let anchors: Vec<CertificateDer<'static>> = match t.client_auth.as_str() {
        "VerifyClientCertIfGiven" | "RequireAndVerifyClientCert" => {
            if !t.client_cas.is_empty() {
                std::fs::read(&t.client_cas)
                    .map(|b| pem_certs(&b))
                    .unwrap_or_default()
            } else if !t.client_ca_text.is_empty() {
                pem_certs(t.client_ca_text.as_bytes())
            } else {
                // Go verifies against the system roots without client CAs.
                rustls_native_certs::load_native_certs().certs
            }
        }
        _ => Vec::new(),
    };
    let verify = |allow_unauth: bool| -> Result<Arc<dyn ClientCertVerifier>, String> {
        let mut roots = RootCertStore::empty();
        for c in &anchors {
            let _ = roots.add(c.clone());
        }
        let b = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider));
        let b = if allow_unauth {
            b.allow_unauthenticated()
        } else {
            b
        };
        b.build().map_err(|e| format!("tls: client verifier: {e}"))
    };
    let sans = t.client_allowed_sans.clone();
    let go_verifier = |inner: Option<Arc<dyn ClientCertVerifier>>, mandatory: bool| {
        // With client_allowed_sans Go panics on a client without a
        // certificate (the connection is dropped): require one.
        let mandatory = mandatory || sans.is_some();
        Arc::new(GoClientVerifier {
            anchors: if inner.is_some() {
                anchors.iter().map(|c| c.to_vec()).collect()
            } else {
                Vec::new()
            },
            inner,
            mandatory,
            allowed_sans: sans.clone(),
            provider: Arc::clone(&provider),
        }) as Arc<dyn ClientCertVerifier>
    };
    let builder = match t.client_auth.as_str() {
        "RequestClientCert" => builder.with_client_cert_verifier(go_verifier(None, false)),
        "RequireAnyClientCert" | "RequireClientCert" => {
            builder.with_client_cert_verifier(go_verifier(None, true))
        }
        "VerifyClientCertIfGiven" => {
            builder.with_client_cert_verifier(go_verifier(Some(verify(true)?), false))
        }
        "RequireAndVerifyClientCert" => {
            builder.with_client_cert_verifier(go_verifier(Some(verify(false)?), true))
        }
        _ => builder.with_no_client_auth(),
    };
    let mut cfg = builder
        .with_single_cert(certs, key)
        .map_err(|e| format!("failed to load X509KeyPair: tls: {e}"))?;
    cfg.alpn_protocols = if c.http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    Ok(Arc::new(cfg))
}

/// x/time/rate's token bucket, retuned in place.
#[derive(Debug)]
struct Limiter {
    /// Tokens per second; `None` is `rate.Inf`.
    limit: Option<f64>,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl Limiter {
    fn rate(interval: i64) -> Option<f64> {
        (interval > 0).then(|| 1e9 / interval as f64)
    }

    fn advance(&self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        let t = match self.limit {
            Some(l) => self.tokens + elapsed * l,
            None => self.burst,
        };
        t.min(self.burst)
    }

    fn retune(&mut self, c: RateConfig) {
        let now = Instant::now();
        self.tokens = self.advance(now);
        self.last = now;
        self.limit = Self::rate(c.interval);
        self.burst = c.burst as f64;
    }

    fn allow(&mut self) -> bool {
        if self.limit.is_none() {
            return true;
        }
        let now = Instant::now();
        let t = self.advance(now) - 1.0;
        if t < 0.0 {
            return false;
        }
        self.tokens = t;
        self.last = now;
        true
    }
}

#[derive(Default)]
struct LimiterState {
    limiter: Option<Limiter>,
    config: RateConfig,
}

/// `webHandler`.
pub struct WebHandler {
    path: String,
    limiter: Mutex<LimiterState>,
    cache: Mutex<HashMap<String, bool>>,
    bcrypt: tokio::sync::Mutex<()>,
}

impl WebHandler {
    pub fn new(path: &str) -> Self {
        WebHandler {
            path: path.to_owned(),
            limiter: Mutex::new(LimiterState::default()),
            cache: Mutex::new(HashMap::new()),
            bcrypt: tokio::sync::Mutex::new(()),
        }
    }

    /// `rateLimiter`: whether the request passes the configured limit.
    fn rate_allows(&self, c: RateConfig) -> bool {
        let c = if c.interval == 0 {
            RateConfig::default()
        } else {
            c
        };
        let mut s = self.limiter.lock();
        let changed = c != s.config;
        if changed && c.interval != 0 {
            match &mut s.limiter {
                Some(l) => l.retune(c),
                None => {
                    s.limiter = Some(Limiter {
                        limit: Limiter::rate(c.interval),
                        burst: c.burst as f64,
                        tokens: c.burst as f64,
                        last: Instant::now(),
                    })
                }
            }
        }
        s.config = c;
        if changed {
            if c.interval == 0 {
                tracing::info!("Rate Limiter is disabled.");
            } else {
                tracing::info!(burst = c.burst, interval = %crate::options::goparse::format_duration(c.interval), "Rate Limiter is enabled.");
            }
        }
        if c.interval == 0 {
            return true;
        }
        s.limiter.as_mut().is_none_or(Limiter::allow)
    }

    fn cache_get(&self, key: &str) -> Option<bool> {
        self.cache.lock().get(key).copied()
    }

    fn cache_set(&self, key: String, v: bool) {
        let mut c = self.cache.lock();
        if c.len() >= 100 {
            let n = (c.len() / 10).max(1);
            let victims: Vec<String> = c.keys().take(n + 1).cloned().collect();
            for k in victims {
                c.remove(&k);
            }
        }
        c.insert(key, v);
    }
}

/// `r.BasicAuth()`.
fn basic_auth(req: &Request) -> Option<(String, String)> {
    let v = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = (v.get(..6)?, v.get(6..)?);
    if !scheme.eq_ignore_ascii_case("basic ") {
        return None;
    }
    let dec = base64::engine::general_purpose::STANDARD
        .decode(rest)
        .ok()?;
    let s = String::from_utf8_lossy(&dec).into_owned();
    let (u, p) = s.split_once(':')?;
    Some((u.to_owned(), p.to_owned()))
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// `webHandler.ServeHTTP` as an axum middleware.
pub async fn web_middleware(
    State(h): State<Arc<WebHandler>>,
    req: Request,
    next: Next,
) -> Response {
    let c = match get_config(&h.path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(err = %e, "Unable to parse configuration");
            return plain_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
        }
    };
    if !h.rate_allows(c.rate) {
        return plain_error(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests");
    }
    let headers: Vec<(header::HeaderName, HeaderValue)> = c
        .headers
        .iter()
        .filter_map(|(k, v)| {
            Some((
                header::HeaderName::from_bytes(k.as_bytes()).ok()?,
                HeaderValue::from_str(v).ok()?,
            ))
        })
        .collect();
    let with_headers = |mut r: Response| {
        for (k, v) in &headers {
            r.headers_mut().insert(k.clone(), v.clone());
        }
        r
    };
    if c.users.is_empty() {
        return with_headers(next.run(req).await);
    }
    if let Some((user, pass)) = basic_auth(&req) {
        let (hash, valid_user) = match c.users.get(&user) {
            Some(h) => (h.clone(), true),
            None => (FAKE_HASH.to_owned(), false),
        };
        let key = format!("{}:{}:{}", hex(&user), hex(&hash), hex(&pass));
        let ok = match h.cache_get(&key) {
            Some(ok) => ok,
            None => {
                let _g = h.bcrypt.lock().await;
                let (p, hh) = (pass.clone(), hash.clone());
                let res = tokio::task::spawn_blocking(move || bcrypt::verify(p, &hh))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(false);
                let ok = valid_user && res;
                h.cache_set(key, ok);
                ok
            }
        };
        if ok && valid_user {
            return with_headers(next.run(req).await);
        }
    }
    let mut r = with_headers(plain_error(StatusCode::UNAUTHORIZED, "Unauthorized"));
    r.headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic"));
    r
}

/// `configureServer`'s startup checks. Returns whether TLS is on.
pub fn configure(path: &str) -> Result<bool, String> {
    validate_users(path)?;
    let c = get_config(path)?;
    let tls = check_tls(&c.tls)?.is_some();
    // net/http's http2.ConfigureServer check, done when ServeTLS starts.
    if tls
        && c.http2
        && !c.tls.cipher_suites.is_empty()
        && !c.tls.cipher_suites.iter().any(|n| {
            n == "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256"
                || n == "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256"
        })
    {
        return Err("http2: TLSConfig.CipherSuites is missing an HTTP/2-required AES_128_GCM_SHA256 cipher (need at least one of TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 or TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256)".to_owned());
    }
    Ok(tls)
}

/// Serves `app` on `listener` with the web config at `path` until `stop`,
/// then lets open connections finish (the caller bounds the wait).
pub async fn serve(
    listener: tokio::net::TcpListener,
    app: Router,
    path: String,
    tls: bool,
    timeouts: crate::conn::Timeouts,
    stop: CancellationToken,
) {
    let handler = Arc::new(WebHandler::new(&path));
    // Log the initial rate limiter state.
    if let Ok(c) = get_config(&path) {
        handler.rate_allows(RateConfig {
            burst: c.rate.burst,
            interval: c.rate.interval,
        });
        // The probe above must not use up a token.
        if let Some(l) = handler.limiter.lock().limiter.as_mut() {
            l.tokens = l.burst;
        }
    }
    let app = app.layer(axum::middleware::from_fn_with_state(
        Arc::clone(&handler),
        web_middleware,
    ));
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    loop {
        let (tcp, addr) = tokio::select! {
            () = stop.cancelled() => break,
            r = listener.accept() => match r {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "accept");
                    continue;
                }
            },
        };
        let deadlines = crate::conn::Deadlines::new(timeouts);
        let app = crate::conn::with_deadlines(app.clone(), deadlines.clone());
        let path = path.clone();
        let watcher = graceful.watcher();
        tokio::spawn(async move {
            let svc = hyper_util::service::TowerToHyperService::new(
                tower::ServiceBuilder::new()
                    .map_request(|r: http::Request<hyper::body::Incoming>| r.map(Body::new))
                    .service(app),
            );
            if !tls {
                let b = hyper::server::conn::http1::Builder::new();
                let io = crate::conn::DeadlineIo::new(tcp, deadlines);
                let conn = b.serve_connection(TokioIo::new(io), svc);
                let _ = watcher.watch(conn).await;
                return;
            }
            serve_tls(tcp, addr, &path, svc, watcher, deadlines).await;
        });
    }
    drop(listener);
    graceful.shutdown().await;
}

async fn serve_tls<S>(
    tcp: tokio::net::TcpStream,
    addr: SocketAddr,
    path: &str,
    svc: S,
    watcher: hyper_util::server::graceful::Watcher,
    deadlines: crate::conn::Deadlines,
) where
    S: hyper::service::Service<
            http::Request<hyper::body::Incoming>,
            Response = Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    let mut tcp = tcp;
    // Plain HTTP on the TLS port gets Go's canned answer.
    let mut first = [0u8; 1];
    if matches!(tcp.peek(&mut first).await, Ok(1)) && first[0] != 0x16 {
        let _ = tcp
            .write_all(b"HTTP/1.0 400 Bad Request\r\n\r\nClient sent an HTTP request to an HTTPS server.\n")
            .await;
        // Read what the client sent, so closing does not reset the connection.
        let mut buf = [0u8; 4096];
        while let Ok(Ok(n)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), tcp.read(&mut buf)).await
        {
            if n == 0 {
                break;
            }
        }
        let _ = tcp.shutdown().await;
        return;
    }
    // Go bounds the handshake with tlsHandshakeTimeout, then the request deadlines apply.
    deadlines.for_handshake();
    let tcp = crate::conn::DeadlineIo::new(tcp, deadlines.clone());
    let start = match tokio_rustls::LazyConfigAcceptor::new(Acceptor::default(), tcp).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("http: TLS handshake error from {addr}: {e}");
            return;
        }
    };
    // `GetConfigForClient`: the file and certificates are read again.
    let cfg = match get_config(path).and_then(|c| rustls_config(&c)) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("http: TLS handshake error from {addr}: {e}");
            return;
        }
    };
    let stream = match start.into_stream(cfg).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("http: TLS handshake error from {addr}: {e}");
            return;
        }
    };
    deadlines.handshake_done();
    let b = auto::Builder::new(TokioExecutor::new());
    let conn = b.serve_connection_with_upgrades(TokioIo::new(stream), svc);
    let _ = watcher.watch(conn.into_owned()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, text: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn config_errors() {
        let dir = std::env::temp_dir().join(format!("ksm-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = write(&dir, "empty.yml", "");
        assert!(get_config(&p).is_ok());
        assert_eq!(configure(&p), Ok(false));
        let p = write(&dir, "bogus.yml", "tls_server_config:\n  bogus_key: 1\n");
        assert_eq!(
            get_config(&p).unwrap_err(),
            "yaml: unmarshal errors:\n  line 2: field bogus_key not found in type web.TLSConfig"
        );
        let p = write(
            &dir,
            "c.yml",
            "tls_server_config:\n  cipher_suites: [FOO]\n",
        );
        assert_eq!(get_config(&p).unwrap_err(), "unknown cipher: FOO");
        let p = write(
            &dir,
            "h.yml",
            "http_server_config:\n  headers:\n    Foo: x\n",
        );
        assert_eq!(
            get_config(&p).unwrap_err(),
            "HTTP header \"Foo\" can not be configured"
        );
        let p = write(
            &dir,
            "f.yml",
            "http_server_config:\n  headers:\n    X-Frame-Options: allow\n",
        );
        assert_eq!(
            get_config(&p).unwrap_err(),
            "invalid value for X-Frame-Options. Expected one of: [\"deny\" \"sameorigin\"], but got: \"allow\""
        );
        let p = write(&dir, "u.yml", "basic_auth_users:\n  a: short\n");
        assert_eq!(
            configure(&p).unwrap_err(),
            "crypto/bcrypt: hashedSecret too short to be a bcrypted password"
        );
        let p = write(&dir, "t.yml", "tls_server_config:\n  cert_file: x.crt\n");
        assert_eq!(configure(&p).unwrap_err(), "missing one of key or key_file");
        let p = write(
            &dir,
            "t2.yml",
            "tls_server_config:\n  cert_file: x.crt\n  key_file: x.key\n",
        );
        let want = format!(
            "failed to read cert_file ({0}/x.crt): open {0}/x.crt: no such file or directory",
            dir.display()
        );
        assert_eq!(configure(&p).unwrap_err(), want);
        assert_eq!(
            get_config("/nonexistent/web.yml").unwrap_err(),
            "open /nonexistent/web.yml: no such file or directory"
        );
    }

    #[test]
    fn limiter() {
        let h = WebHandler::new("");
        let c = RateConfig {
            burst: 2,
            interval: 3_600_000_000_000,
        };
        assert!(h.rate_allows(c));
        assert!(h.rate_allows(c));
        assert!(!h.rate_allows(c));
        assert!(h.rate_allows(RateConfig::default()));
        let zero = RateConfig {
            burst: 0,
            interval: 1_000_000_000,
        };
        assert!(!h.rate_allows(zero));
    }

    #[test]
    fn generated_certs() {
        let mut params = rcgen::CertificateParams::new(vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
            "::ffff:10.0.0.1".to_owned(),
        ])
        .unwrap();
        params
            .subject_alt_names
            .push(rcgen::SanType::URI("spiffe://x/y".try_into().unwrap()));
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let sans = san_values(cert.der()).unwrap();
        assert_eq!(sans, ["localhost", "127.0.0.1", "10.0.0.1", "spiffe://x/y"]);

        let c = WebConfig {
            tls: TlsConfig {
                cert: cert.pem(),
                key: key.serialize_pem(),
                client_auth: "RequireAndVerifyClientCert".to_owned(),
                client_ca_text: cert.pem(),
                min_version: TLS12,
                max_version: TLS13,
                client_allowed_sans: Some(vec!["localhost".to_owned()]),
                cipher_suites: vec!["TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256".to_owned()],
                curve_preferences: vec!["CurveP521".to_owned()],
                ..TlsConfig::default()
            },
            http2: false,
            ..WebConfig::default()
        };
        let cfg = rustls_config(&c).unwrap();
        assert_eq!(cfg.alpn_protocols, vec![b"http/1.1".to_vec()]);
        let mut bad = c.clone();
        bad.tls.key = rcgen::KeyPair::generate().unwrap().serialize_pem();
        assert_eq!(
            rustls_config(&bad).unwrap_err(),
            "failed to load X509KeyPair: tls: private key does not match public key"
        );
        let mut v = c;
        v.tls.max_version = TLS11;
        assert!(rustls_config(&v).is_err());
    }

    #[test]
    fn bcrypt_checks() {
        assert!(bcrypt_cost(FAKE_HASH).is_ok());
        assert!(bcrypt_cost(&format!("x{}", &FAKE_HASH[1..])).is_err());
    }
}
