use crate::config::{config_root_dir, load_https_trust, normalize_sha256_pin, HttpsTrust};
use crate::win_util::executable_dir;
use chrono::NaiveDate;
use flate2::read::GzDecoder;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use rustls_pki_types::{CertificateDer, IpAddr as PkiIpAddr, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

const MAX_HTTP_BODY_BYTES: usize = 64 * 1024 * 1024;
const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1D, 0x11]; // 2.5.29.17

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUrl {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub base_path: String,
    pub full_normalized: String,
}

pub fn parse_api_url(raw: &str) -> Result<ParsedUrl, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("API URL が空です".into());
    }
    let parsed = Url::parse(trimmed).map_err(|_| format!("API URL の形式が不正です: {}", raw))?;
    let secure = match parsed.scheme() {
        "https" => true,
        "http" => false,
        s => return Err(format!("未対応の URL スキームです: {}", s)),
    };
    let host = parsed
        .host_str()
        .ok_or_else(|| "API URL にホスト名がありません".to_string())?
        .trim_matches(|c| c == '[' || c == ']')
        .to_string();
    if !secure && host != "127.0.0.1" && host != "localhost" && host != "::1" {
        return Err("リモート URL には HTTPS を使用してください".into());
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| "API URL にポートがありません".to_string())?;
    let base_path = parsed.path().trim_end_matches('/').to_string();
    let host_part = if host.contains(':') {
        format!("[{}]", host)
    } else {
        host.clone()
    };
    let full_normalized = if base_path.is_empty() {
        format!("{}://{}:{}", parsed.scheme(), host_part, port)
    } else {
        format!("{}://{}:{}{}", parsed.scheme(), host_part, port, base_path)
    };
    Ok(ParsedUrl {
        secure,
        host,
        port,
        base_path,
        full_normalized,
    })
}

pub fn resolve_https_trust_for_url(raw_url: &str) -> Result<Option<HttpsTrust>, String> {
    let parsed = parse_api_url(raw_url)?;
    if !parsed.secure {
        return Ok(None);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut push_cand = |p: PathBuf| {
        if !candidates.iter().any(|existing| existing == &p) {
            candidates.push(p);
        }
    };
    push_cand(config_root_dir());
    if let Ok(exe_dir) = executable_dir() {
        push_cand(exe_dir.clone());
        if let Some(p) = exe_dir.parent() {
            push_cand(p.to_path_buf());
            if let Some(gp) = p.parent() {
                push_cand(gp.to_path_buf());
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.trim().is_empty() {
                push_cand(PathBuf::from(xdg.trim()).join("qwen27"));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                push_cand(PathBuf::from(home.trim()).join(".config").join("qwen27"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        push_cand(cwd.clone());
        if let Some(p) = cwd.parent() {
            push_cand(p.to_path_buf());
        }
    }
    if let Ok(ca_raw) = std::env::var("QWEN_CA_FILE") {
        let ca = ca_raw.trim();
        if !ca.is_empty() {
            let pin = crate::tls_bootstrap::load_pin_from_ca_file(std::path::Path::new(ca))?;
            return Ok(Some(HttpsTrust {
                urls: vec![parsed.full_normalized.clone()],
                sha256: pin,
                pins: HashMap::new(),
            }));
        }
    }
    let host_matches = |u: &str| {
        parse_api_url(u)
            .map(|p| p.secure && p.host.eq_ignore_ascii_case(&parsed.host) && p.port == parsed.port)
            .unwrap_or(false)
    };
    for dir in candidates {
        if let Some(trust) = load_https_trust(&dir)? {
            for (u, p) in &trust.pins {
                if host_matches(u) {
                    let pin = normalize_sha256_pin(p)?;
                    return Ok(Some(HttpsTrust {
                        urls: trust.urls,
                        sha256: pin,
                        pins: trust.pins,
                    }));
                }
            }
            if !trust.sha256.is_empty()
                && (trust.urls.is_empty() || trust.urls.iter().any(|u| host_matches(u)))
            {
                let pin = normalize_sha256_pin(&trust.sha256)?;
                return Ok(Some(HttpsTrust {
                    urls: trust.urls,
                    sha256: pin,
                    pins: trust.pins,
                }));
            }
        }
    }
    if std::env::var("QWEN_NO_AUTO_TRUST").ok().as_deref() != Some("1") {
        let generated = crate::tls_bootstrap::probe_and_generate_https_trust(
            &parsed.full_normalized,
            &config_root_dir(),
            5000,
        )?;
        return Ok(Some(generated));
    }
    Err(format!(
        "HTTPS 証明書ピン (https-server.json) が {} 用に見つかりません",
        raw_url
    ))
}

pub(crate) fn sha256_upper_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02X}", b));
    }
    out
}

fn constant_time_eq_ascii(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn dns_name_matches(pattern: &str, host: &str) -> bool {
    let p = pattern.trim_end_matches('.').to_ascii_lowercase();
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if p == h {
        return true;
    }
    if let Some(rest) = p.strip_prefix("*.") {
        if !rest.contains('*') && !rest.is_empty() {
            if let Some(dot) = h.find('.') {
                return &h[dot + 1..] == rest && dot > 0;
            }
        }
    }
    false
}

fn read_der_tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let &tag = input.first()?;
    let &len_byte = input.get(1)?;
    let (len, hdr_len) = if len_byte < 0x80 {
        (len_byte as usize, 2usize)
    } else {
        let count = (len_byte & 0x7F) as usize;
        if count == 0 || count > 4 {
            return None;
        }
        let len_bytes = input.get(2..2 + count)?;
        let mut val = 0usize;
        for &b in len_bytes {
            val = (val << 8) | (b as usize);
        }
        (val, 2 + count)
    };
    let end = hdr_len.checked_add(len)?;
    if end > input.len() {
        return None;
    }
    Some((tag, &input[hdr_len..end], &input[end..]))
}

fn expect_der_tag<'a>(input: &'a [u8], expected: u8) -> Option<(&'a [u8], &'a [u8])> {
    let (tag, val, rest) = read_der_tlv(input)?;
    if tag == expected {
        Some((val, rest))
    } else {
        None
    }
}

fn parse_ascii_digits(bytes: &[u8]) -> Option<u32> {
    let mut val = 0u32;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(val)
}

fn parse_x509_time(tag: u8, raw: &[u8]) -> Option<i64> {
    let (year, rest) = match tag {
        0x17 => {
            // UTCTime: YYMMDDHHMMSSZ
            if raw.len() != 13 || raw[12] != b'Z' {
                return None;
            }
            let yy = parse_ascii_digits(&raw[0..2])? as i32;
            let year = if yy >= 50 { 1900 + yy } else { 2000 + yy };
            (year, &raw[2..12])
        }
        0x18 => {
            // GeneralizedTime: YYYYMMDDHHMMSS(.fff)?Z
            if raw.len() < 15 || *raw.last()? != b'Z' {
                return None;
            }
            let body = &raw[..raw.len() - 1];
            let main = body.split(|&b| b == b'.').next()?;
            if main.len() != 14 {
                return None;
            }
            let year = parse_ascii_digits(&main[0..4])? as i32;
            (year, &main[4..14])
        }
        _ => return None,
    };
    let mon = parse_ascii_digits(&rest[0..2])?;
    let day = parse_ascii_digits(&rest[2..4])?;
    let hour = parse_ascii_digits(&rest[4..6])?;
    let min = parse_ascii_digits(&rest[6..8])?;
    let sec = parse_ascii_digits(&rest[8..10])?;
    let date = NaiveDate::from_ymd_opt(year, mon, day)?;
    let dt = date.and_hms_opt(hour, min, sec)?;
    Some(dt.and_utc().timestamp())
}

pub(crate) fn verify_cert_validity_and_san_at(
    der: &[u8],
    server_name: &ServerName<'_>,
    now: UnixTime,
) -> Result<(), rustls::Error> {
    let bad = || rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding);
    let (cert_seq, cert_tail) = expect_der_tag(der, 0x30).ok_or_else(bad)?;
    if !cert_tail.is_empty() {
        return Err(bad());
    }
    let (mut tbs, _) = expect_der_tag(cert_seq, 0x30).ok_or_else(bad)?;

    // Optional [0] EXPLICIT version
    if let Some((0xA0, _, rest)) = read_der_tlv(tbs) {
        tbs = rest;
    }
    // serialNumber (INTEGER), signature (SEQUENCE), issuer (SEQUENCE)
    let (_, rest) = expect_der_tag(tbs, 0x02).ok_or_else(bad)?;
    let (_, rest) = expect_der_tag(rest, 0x30).ok_or_else(bad)?;
    let (_, rest) = expect_der_tag(rest, 0x30).ok_or_else(bad)?;

    // validity (SEQUENCE { notBefore, notAfter })
    let (validity, rest) = expect_der_tag(rest, 0x30).ok_or_else(bad)?;
    let (nb_tag, nb_val, v_rest) = read_der_tlv(validity).ok_or_else(bad)?;
    let (na_tag, na_val, v_tail) = read_der_tlv(v_rest).ok_or_else(bad)?;
    if !v_tail.is_empty() {
        return Err(bad());
    }
    let not_before = parse_x509_time(nb_tag, nb_val).ok_or_else(bad)?;
    let not_after = parse_x509_time(na_tag, na_val).ok_or_else(bad)?;
    if not_before > not_after {
        return Err(bad());
    }
    let now_secs = now.as_secs() as i64;
    if now_secs < not_before {
        return Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::NotValidYet,
        ));
    }
    if now_secs > not_after {
        return Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::Expired,
        ));
    }

    // subject (SEQUENCE), subjectPublicKeyInfo (SEQUENCE)
    let (_, rest) = expect_der_tag(rest, 0x30).ok_or_else(bad)?;
    let (_, mut tail) = expect_der_tag(rest, 0x30).ok_or_else(bad)?;

    let mut san_blob: Option<&[u8]> = None;
    while !tail.is_empty() {
        let (tag, val, next) = read_der_tlv(tail).ok_or_else(bad)?;
        tail = next;
        if tag == 0xA3 {
            let (mut exts, exts_tail) = expect_der_tag(val, 0x30).ok_or_else(bad)?;
            if !exts_tail.is_empty() {
                return Err(bad());
            }
            while !exts.is_empty() {
                let (ext_seq, ext_next) = expect_der_tag(exts, 0x30).ok_or_else(bad)?;
                exts = ext_next;
                let (oid, mut ext_rest) = expect_der_tag(ext_seq, 0x06).ok_or_else(bad)?;
                if let Some((0x01, _, after_crit)) = read_der_tlv(ext_rest) {
                    ext_rest = after_crit;
                }
                let (ext_val, ext_tail) = expect_der_tag(ext_rest, 0x04).ok_or_else(bad)?;
                if !ext_tail.is_empty() {
                    return Err(bad());
                }
                if oid == OID_SUBJECT_ALT_NAME {
                    san_blob = Some(ext_val);
                    break;
                }
            }
            break;
        }
    }

    let Some(raw_san) = san_blob else {
        return Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::NotValidForName,
        ));
    };

    let (mut names, names_tail) = expect_der_tag(raw_san, 0x30).ok_or_else(bad)?;
    if !names_tail.is_empty() {
        return Err(bad());
    }
    let mut matched = false;
    while !names.is_empty() {
        let (tag, val, next) = read_der_tlv(names).ok_or_else(bad)?;
        names = next;
        match (server_name, tag) {
            (ServerName::DnsName(dns), 0x82) => {
                if let Ok(name) = std::str::from_utf8(val) {
                    if dns_name_matches(name, dns.as_ref()) {
                        matched = true;
                        break;
                    }
                }
            }
            (ServerName::IpAddress(ip), 0x87) => {
                let eq = match ip {
                    PkiIpAddr::V4(v4) => val == v4.as_ref(),
                    PkiIpAddr::V6(v6) => val == v6.as_ref(),
                };
                if eq {
                    matched = true;
                    break;
                }
            }
            _ => {}
        }
    }
    if matched {
        Ok(())
    } else {
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::NotValidForName,
        ))
    }
}

#[cfg(test)]
fn verify_cert_validity_and_san(
    der: &[u8],
    server_name: &ServerName<'_>,
) -> Result<(), rustls::Error> {
    verify_cert_validity_and_san_at(der, server_name, UnixTime::now())
}

#[derive(Debug)]
struct PinnedServerCertVerifier {
    expected_sha256_upper: String,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual_hex = sha256_upper_hex(end_entity.as_ref());
        if !constant_time_eq_ascii(&actual_hex, &self.expected_sha256_upper) {
            return Err(rustls::Error::General(format!(
                "TLS certificate fingerprint mismatch (got {}, expected pinned cert)",
                actual_hex
            )));
        }
        verify_cert_validity_and_san_at(end_entity.as_ref(), server_name, now)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

static TLS_CONFIG_CACHE: OnceCell<Mutex<HashMap<String, Arc<ClientConfig>>>> = OnceCell::new();

fn tls_config_for_pin(sha256_upper: &str) -> Result<Arc<ClientConfig>, String> {
    let cache = TLS_CONFIG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock();
    if let Some(existing) = guard.get(sha256_upper) {
        return Ok(Arc::clone(existing));
    }
    let provider = rustls::crypto::ring::default_provider();
    let algs = provider.signature_verification_algorithms;
    let verifier = Arc::new(PinnedServerCertVerifier {
        expected_sha256_upper: sha256_upper.to_string(),
        algs,
    });
    let cfg = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS バージョン設定に失敗しました: {}", e))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let arc = Arc::new(cfg);
    guard.insert(sha256_upper.to_string(), Arc::clone(&arc));
    Ok(arc)
}

enum ConnStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl ConnStream {
    fn set_timeout(&mut self, timeout: Duration) {
        let tcp = match self {
            ConnStream::Plain(s) => s,
            ConnStream::Tls(s) => s.get_mut(),
        };
        let slice_timeout = Some(timeout.min(Duration::from_millis(500)));
        let _ = tcp.set_read_timeout(slice_timeout);
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(10).max(timeout)));
    }

    fn raw_clone(&self) -> Option<TcpStream> {
        let tcp = match self {
            ConnStream::Plain(s) => s,
            ConnStream::Tls(s) => s.get_ref(),
        };
        tcp.try_clone().ok()
    }
}

impl Read for ConnStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            ConnStream::Plain(s) => s.read(buf),
            ConnStream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for ConnStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            ConnStream::Plain(s) => s.write(buf),
            ConnStream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            ConnStream::Plain(s) => s.flush(),
            ConnStream::Tls(s) => s.flush(),
        }
    }
}

pub struct ApiClient {
    base_url: String,
    parsed: ParsedUrl,
    explicit_pin: Option<String>,
    conn: Mutex<Option<ConnStream>>,
    active_tcp: Mutex<Option<TcpStream>>,
    halted: AtomicBool,
}

impl ApiClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, String> {
        let raw = base_url.into();
        let parsed = parse_api_url(&raw)?;
        let pin = if parsed.secure {
            resolve_https_trust_for_url(&raw)?.map(|t| t.sha256)
        } else {
            None
        };
        Ok(Self {
            base_url: raw,
            parsed,
            explicit_pin: pin,
            conn: Mutex::new(None),
            active_tcp: Mutex::new(None),
            halted: AtomicBool::new(false),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn cancel_io(&self) {
        self.halted.store(true, Ordering::SeqCst);
        if let Some(tcp) = self.active_tcp.lock().take() {
            let _ = tcp.shutdown(std::net::Shutdown::Both);
        }
    }

    fn connect_new(&self, timeout: Duration) -> Result<ConnStream, String> {
        if self.halted.load(Ordering::SeqCst) {
            return Err("API 通信は中断されました".into());
        }
        let addrs: Vec<SocketAddr> = format!("{}:{}", self.parsed.host, self.parsed.port)
            .to_socket_addrs()
            .map_err(|e| {
                format!(
                    "API への接続に失敗しました ({}): {}。接続先とサーバーの状態を確認してください",
                    self.base_url, e
                )
            })?
            .collect();
        if addrs.is_empty() {
            return Err(format!("ホストを解決できません: {}", self.parsed.host));
        }
        let mut last_err = String::new();
        let mut tcp_opt = None;
        let conn_timeout = timeout.min(Duration::from_secs(5));
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, conn_timeout) {
                Ok(s) => {
                    tcp_opt = Some(s);
                    break;
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        let tcp = tcp_opt.ok_or_else(|| {
            format!(
                "API への接続に失敗しました: {} ({})。接続先とサーバーの状態を確認してください。",
                self.base_url, last_err
            )
        })?;
        let _ = tcp.set_nodelay(true);
        let slice_timeout = Some(timeout.min(Duration::from_millis(500)));
        let _ = tcp.set_read_timeout(slice_timeout);
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(10)));

        if self.parsed.secure {
            let pin = self.explicit_pin.as_deref().ok_or_else(|| {
                "HTTPS 証明書ピンが設定されていません".to_string()
            })?;
            let cfg = tls_config_for_pin(pin)?;
            let server_name = ServerName::try_from(self.parsed.host.clone())
                .map_err(|_| format!("不正なホスト名です: {}", self.parsed.host))?;
            let conn = ClientConnection::new(cfg, server_name)
                .map_err(|e| format!("TLS セッションを初期化できません: {}", e))?;
            let tls = StreamOwned::new(conn, tcp);
            Ok(ConnStream::Tls(Box::new(tls)))
        } else {
            Ok(ConnStream::Plain(tcp))
        }
    }

    pub fn request(
        &self,
        path: &str,
        body: Option<&Value>,
        timeout_ms: u64,
        method: Option<&str>,
    ) -> Result<Value, String> {
        let timeout = Duration::from_millis(timeout_ms.max(1000));
        let deadline = Instant::now() + timeout;
        let verb = match method {
            Some(m) if !m.trim().is_empty() => m.trim(),
            _ if body.is_some() => "POST",
            _ => "GET",
        };
        let req_path = if self.parsed.base_path.is_empty() {
            path.to_string()
        } else if path.starts_with('/') {
            format!("{}{}", self.parsed.base_path, path)
        } else {
            format!("{}/{}", self.parsed.base_path, path)
        };
        let body_bytes = match body {
            Some(v) => serde_json::to_vec(v).map_err(|e| format!("JSON 化に失敗しました: {}", e))?,
            None => Vec::new(),
        };
        let host_hdr = if self.parsed.host.contains(':') {
            format!("[{}]:{}", self.parsed.host, self.parsed.port)
        } else {
            format!("{}:{}", self.parsed.host, self.parsed.port)
        };
        let mut header = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Qwen27-Rust/1.0\r\nAccept: application/json\r\nAccept-Encoding: gzip\r\nConnection: keep-alive\r\n",
            verb, req_path, host_hdr
        );
        if !body_bytes.is_empty() || verb == "POST" || verb == "PUT" || verb == "PATCH" {
            header.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body_bytes.len()
            ));
        }
        header.push_str("\r\n");

        for attempt in 0..2 {
            if self.halted.load(Ordering::SeqCst) {
                return Err("API 通信は中断されました".into());
            }
            let mut stream = match self.conn.lock().take() {
                Some(mut s) => {
                    s.set_timeout(timeout);
                    s
                }
                None => self.connect_new(timeout)?,
            };
            *self.active_tcp.lock() = stream.raw_clone();

            let io_res = self.exchange_once(&mut stream, header.as_bytes(), &body_bytes, deadline);
            *self.active_tcp.lock() = None;

            match io_res {
                Ok((status, keep_alive, parsed_json)) => {
                    if keep_alive && !self.halted.load(Ordering::SeqCst) {
                        *self.conn.lock() = Some(stream);
                    }
                    if status < 200 || status >= 300 {
                        let msg = parsed_json
                            .get("error")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("HTTP {}", status));
                        return Err(msg);
                    }
                    return Ok(parsed_json);
                }
                Err(ExchangeErr::Retryable(msg)) if attempt == 0 && !self.halted.load(Ordering::SeqCst) => {
                    let _ = msg;
                    continue;
                }
                Err(ExchangeErr::Retryable(msg)) | Err(ExchangeErr::Fatal(msg)) => {
                    return Err(msg);
                }
            }
        }
        Err("API connection failed".into())
    }

    pub fn get(&self, path: &str, timeout_ms: u64) -> Result<Value, String> {
        self.request(path, None, timeout_ms, Some("GET"))
    }

    pub fn post(&self, path: &str, body: &Value, timeout_ms: u64) -> Result<Value, String> {
        self.request(path, Some(body), timeout_ms, Some("POST"))
    }

    pub fn delete(&self, path: &str, timeout_ms: u64) -> Result<Value, String> {
        self.request(path, None, timeout_ms, Some("DELETE"))
    }

    fn exchange_once(
        &self,
        stream: &mut ConnStream,
        header_bytes: &[u8],
        body_bytes: &[u8],
        deadline: Instant,
    ) -> Result<(u16, bool, Value), ExchangeErr> {
        write_all_with_cancel(stream, header_bytes, &self.halted, deadline)?;
        if !body_bytes.is_empty() {
            write_all_with_cancel(stream, body_bytes, &self.halted, deadline)?;
        }
        stream
            .flush()
            .map_err(|e| ExchangeErr::Retryable(format!("API 送信に失敗しました: {}", e)))?;

        let (status, headers, mut buf) = read_http_head(stream, &self.halted, deadline)?;
        let content_encoding = headers.get("content-encoding").cloned().unwrap_or_default();
        let connection_hdr = headers.get("connection").cloned().unwrap_or_default();
        let transfer_enc = headers.get("transfer-encoding").cloned().unwrap_or_default();
        let content_length = headers
            .get("content-length")
            .and_then(|s| s.parse::<usize>().ok());

        let mut keep_alive = !connection_hdr.eq_ignore_ascii_case("close");
        let raw_body = if transfer_enc.to_ascii_lowercase().contains("chunked") {
            read_chunked_body(stream, &mut buf, &self.halted, deadline)?
        } else if let Some(len) = content_length {
            if len > MAX_HTTP_BODY_BYTES {
                return Err(ExchangeErr::Fatal("API response exceeds 64 MiB".into()));
            }
            while buf.len() < len {
                let before = buf.len();
                read_more(stream, &mut buf, &self.halted, deadline)?;
                if buf.len() == before {
                    return Err(ExchangeErr::Retryable("Unexpected EOF reading API body".into()));
                }
            }
            buf.truncate(len);
            buf
        } else if matches!(status, 204 | 304) {
            Vec::new()
        } else {
            keep_alive = false;
            loop {
                let before = buf.len();
                match read_more_eof_ok(stream, &mut buf, &self.halted, deadline) {
                    Ok(true) => break,
                    Ok(false) => {
                        if buf.len() > MAX_HTTP_BODY_BYTES {
                            return Err(ExchangeErr::Fatal("API response exceeds 64 MiB".into()));
                        }
                        if buf.len() == before {
                            continue;
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            buf
        };

        let decoded = if content_encoding.to_ascii_lowercase().contains("gzip") && !raw_body.is_empty() {
            let mut gz = GzDecoder::new(&raw_body[..]);
            let mut out = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = gz
                    .read(&mut chunk)
                    .map_err(|e| ExchangeErr::Fatal(format!("gzip 解凍に失敗しました: {}", e)))?;
                if n == 0 {
                    break;
                }
                if out.len() + n > MAX_HTTP_BODY_BYTES {
                    return Err(ExchangeErr::Fatal("API response exceeds 64 MiB".into()));
                }
                out.extend_from_slice(&chunk[..n]);
            }
            out
        } else {
            raw_body
        };

        let json = parse_reply_bytes(&decoded).map_err(ExchangeErr::Fatal)?;
        Ok((status, keep_alive, json))
    }
}

enum ExchangeErr {
    Retryable(String),
    Fatal(String),
}

fn is_would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
    )
}

fn write_all_with_cancel(
    stream: &mut ConnStream,
    data: &[u8],
    halted: &AtomicBool,
    deadline: Instant,
) -> Result<(), ExchangeErr> {
    let mut off = 0;
    while off < data.len() {
        if halted.load(Ordering::SeqCst) {
            return Err(ExchangeErr::Fatal("API 通信は中断されました".into()));
        }
        if Instant::now() >= deadline {
            return Err(ExchangeErr::Fatal("API 送信がタイムアウトしました".into()));
        }
        match stream.write(&data[off..]) {
            Ok(0) => return Err(ExchangeErr::Retryable("Connection closed during write".into())),
            Ok(n) => off += n,
            Err(e) if is_would_block(&e) => continue,
            Err(e) => return Err(ExchangeErr::Retryable(format!("API 送信エラー: {}", e))),
        }
    }
    Ok(())
}

fn read_more(
    stream: &mut ConnStream,
    buf: &mut Vec<u8>,
    halted: &AtomicBool,
    deadline: Instant,
) -> Result<(), ExchangeErr> {
    let mut tmp = [0u8; 16384];
    loop {
        if halted.load(Ordering::SeqCst) {
            return Err(ExchangeErr::Fatal("API 通信は中断されました".into()));
        }
        if Instant::now() >= deadline {
            return Err(ExchangeErr::Fatal("API 応答がタイムアウトしました".into()));
        }
        match stream.read(&mut tmp) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if buf.len() + n > MAX_HTTP_BODY_BYTES {
                    return Err(ExchangeErr::Fatal("API response exceeds 64 MiB".into()));
                }
                buf.extend_from_slice(&tmp[..n]);
                return Ok(());
            }
            Err(e) if is_would_block(&e) => continue,
            Err(e) => return Err(ExchangeErr::Retryable(format!("API 受信エラー: {}", e))),
        }
    }
}

fn read_more_eof_ok(
    stream: &mut ConnStream,
    buf: &mut Vec<u8>,
    halted: &AtomicBool,
    deadline: Instant,
) -> Result<bool, ExchangeErr> {
    let mut tmp = [0u8; 16384];
    loop {
        if halted.load(Ordering::SeqCst) {
            return Err(ExchangeErr::Fatal("API 通信は中断されました".into()));
        }
        if Instant::now() >= deadline {
            return Err(ExchangeErr::Fatal("API 応答がタイムアウトしました".into()));
        }
        match stream.read(&mut tmp) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                if buf.len() + n > MAX_HTTP_BODY_BYTES {
                    return Err(ExchangeErr::Fatal("API response exceeds 64 MiB".into()));
                }
                buf.extend_from_slice(&tmp[..n]);
                return Ok(false);
            }
            Err(e) if is_would_block(&e) => continue,
            Err(e) => return Err(ExchangeErr::Fatal(format!("API 受信エラー: {}", e))),
        }
    }
}

fn read_http_head(
    stream: &mut ConnStream,
    halted: &AtomicBool,
    deadline: Instant,
) -> Result<(u16, HashMap<String, String>, Vec<u8>), ExchangeErr> {
    let mut raw = Vec::<u8>::with_capacity(4096);
    loop {
        if let Some(pos) = find_header_end(&raw) {
            let mut hdr_slots = [httparse::EMPTY_HEADER; 64];
            let mut resp = httparse::Response::new(&mut hdr_slots);
            match resp.parse(&raw[..pos]) {
                Ok(httparse::Status::Complete(_)) => {
                    let status = resp
                        .code
                        .ok_or_else(|| ExchangeErr::Fatal("HTTP ステータスがありません".into()))?;
                    let mut map = HashMap::new();
                    for h in resp.headers.iter() {
                        map.insert(
                            h.name.to_ascii_lowercase(),
                            String::from_utf8_lossy(h.value).trim().to_string(),
                        );
                    }
                    let rest = raw[pos..].to_vec();
                    return Ok((status, map, rest));
                }
                Ok(httparse::Status::Partial) => {}
                Err(e) => {
                    return Err(ExchangeErr::Fatal(format!("HTTP ヘッダー解析エラー: {}", e)));
                }
            }
        }
        if raw.len() > 256 * 1024 {
            return Err(ExchangeErr::Fatal("HTTP ヘッダーが大きすぎます".into()));
        }
        let before = raw.len();
        read_more(stream, &mut raw, halted, deadline)?;
        if raw.len() == before {
            return Err(ExchangeErr::Retryable(
                "サーバーから応答がありません (接続が閉じられました)".into(),
            ));
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

fn read_chunked_body(
    stream: &mut ConnStream,
    buf: &mut Vec<u8>,
    halted: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>, ExchangeErr> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    loop {
        let line_end = loop {
            if let Some(rel) = buf[cursor..].windows(2).position(|w| w == b"\r\n") {
                break cursor + rel;
            }
            let before = buf.len();
            read_more(stream, buf, halted, deadline)?;
            if buf.len() == before {
                return Err(ExchangeErr::Fatal("chunked 応答が途中で終了しました".into()));
            }
        };
        let size_line = String::from_utf8_lossy(&buf[cursor..line_end]);
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let chunk_len = usize::from_str_radix(size_hex, 16)
            .map_err(|_| ExchangeErr::Fatal("不正な chunked サイズです".into()))?;
        cursor = line_end + 2;
        if chunk_len == 0 {
            break;
        }
        if out.len() + chunk_len > MAX_HTTP_BODY_BYTES {
            return Err(ExchangeErr::Fatal("API response exceeds 64 MiB".into()));
        }
        while buf.len() < cursor + chunk_len + 2 {
            let before = buf.len();
            read_more(stream, buf, halted, deadline)?;
            if buf.len() == before {
                return Err(ExchangeErr::Fatal("chunked 本体が途中で終了しました".into()));
            }
        }
        out.extend_from_slice(&buf[cursor..cursor + chunk_len]);
        cursor += chunk_len + 2;
        if cursor > 65536 {
            buf.drain(..cursor);
            cursor = 0;
        }
    }
    Ok(out)
}

pub fn sanitize_json_lone_surrogates(input: &str) -> String {
    if !input.contains("\\uD") && !input.contains("\\ud") {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if i + 6 <= bytes.len() && bytes[i] == b'\\' && bytes[i + 1] == b'u' {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 2..i + 6]) {
                if let Ok(code) = u16::from_str_radix(hex_str, 16) {
                    if (0xD800..=0xDBFF).contains(&code) {
                        if i + 12 <= bytes.len() && bytes[i + 6] == b'\\' && bytes[i + 7] == b'u' {
                            if let Ok(low_str) = std::str::from_utf8(&bytes[i + 8..i + 12]) {
                                if let Ok(low) = u16::from_str_radix(low_str, 16) {
                                    if (0xDC00..=0xDFFF).contains(&low) {
                                        out.push_str(&input[i..i + 12]);
                                        i += 12;
                                        continue;
                                    }
                                }
                            }
                        }
                        out.push_str("\\uFFFD");
                        i += 6;
                        continue;
                    } else if (0xDC00..=0xDFFF).contains(&code) {
                        out.push_str("\\uFFFD");
                        i += 6;
                        continue;
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

pub fn parse_reply_bytes(bytes: &[u8]) -> Result<Value, String> {
    if bytes.is_empty() {
        return Ok(serde_json::json!({}));
    }
    let text = String::from_utf8_lossy(bytes);
    let clean = sanitize_json_lone_surrogates(text.trim());
    if clean.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&clean).map_err(|e| format!("API 応答が JSON ではありません: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_https_and_ipv6_urls() {
        let p = parse_api_url("http://127.0.0.1:8090/v1/").unwrap();
        assert!(!p.secure);
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 8090);
        assert_eq!(p.base_path, "/v1");

        let p2 = parse_api_url("https://[::1]:8443").unwrap();
        assert!(p2.secure);
        assert_eq!(p2.host, "::1");
        assert_eq!(p2.port, 8443);
        assert_eq!(p2.base_path, "");
    }

    #[test]
    fn sanitizes_lone_surrogates_in_json() {
        let valid = r#"{"ok":"\uD83D\uDE00","bad":"\uD800x\uDC01"}"#;
        let sanitized = sanitize_json_lone_surrogates(valid);
        let v: Value = serde_json::from_str(&sanitized).unwrap();
        assert_eq!(v["ok"].as_str(), Some("😀"));
        assert_eq!(v["bad"].as_str(), Some("\u{FFFD}x\u{FFFD}"));
    }

    #[test]
    fn wildcard_dns_matching_single_label() {
        assert!(dns_name_matches("*.example.com", "api.example.com"));
        assert!(!dns_name_matches("*.example.com", "a.b.example.com"));
        assert!(dns_name_matches("localhost", "LOCALHOST."));
    }

    #[test]
    fn verifies_server_crt_san_ips_and_dns() {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;

        let crt_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("certs")
            .join("server.crt");
        if !crt_path.is_file() {
            return;
        }
        let pem = std::fs::read_to_string(&crt_path).unwrap();
        let b64: String = pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .map(str::trim)
            .collect();
        let der = B64.decode(b64).unwrap();
        assert!(verify_cert_validity_and_san(&der, &ServerName::try_from("192.0.2.10").unwrap()).is_ok());
        assert!(verify_cert_validity_and_san(&der, &ServerName::try_from("198.51.100.10").unwrap()).is_ok());
        assert!(verify_cert_validity_and_san(&der, &ServerName::try_from("localhost").unwrap()).is_ok());
        assert!(verify_cert_validity_and_san(&der, &ServerName::try_from("192.0.2.99").unwrap()).is_err());

        let too_early = UnixTime::since_unix_epoch(Duration::from_secs(1_500_000_000));
        let too_late = UnixTime::since_unix_epoch(Duration::from_secs(2_500_000_000));
        assert!(matches!(
            verify_cert_validity_and_san_at(&der, &ServerName::try_from("localhost").unwrap(), too_early),
            Err(rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidYet))
        ));
        assert!(matches!(
            verify_cert_validity_and_san_at(&der, &ServerName::try_from("localhost").unwrap(), too_late),
            Err(rustls::Error::InvalidCertificate(rustls::CertificateError::Expired))
        ));
        assert!(parse_x509_time(0x17, b"261019193518Z").is_some());
        assert!(parse_x509_time(0x18, b"20510101000000Z").is_some());
    }

    #[test]
    fn bootstrap_saves_and_merges_https_server_json_and_reads_pem_ca() {
        let dir = std::env::temp_dir().join(format!("qwen_boot_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let der_a = b"mock-cert-bytes-server-a";
        let pin_a = sha256_upper_hex(der_a);
        let t1 = crate::tls_bootstrap::save_https_trust_entry(
            &dir,
            "https://192.0.2.10:8443",
            &pin_a,
            Some(der_a),
        )
        .unwrap();
        assert_eq!(t1.sha256, pin_a);
        assert!(dir.join("https-server.json").is_file());
        assert!(dir.join("certs").join("server.crt").is_file());
        let ca_pin = crate::tls_bootstrap::load_pin_from_ca_file(&dir.join("certs").join("server.crt")).unwrap();
        assert_eq!(ca_pin, pin_a);

        // Same server on second URL (e.g., Tailscale) merges cleanly
        let t2 = crate::tls_bootstrap::save_https_trust_entry(
            &dir,
            "https://198.51.100.10:8443",
            &pin_a,
            None,
        )
        .unwrap();
        assert_eq!(t2.urls.len(), 2);

        // Different server preserves previous pin and adds its own per-URL pin
        let pin_b = sha256_upper_hex(b"mock-cert-bytes-server-b");
        let _ = crate::tls_bootstrap::save_https_trust_entry(
            &dir,
            "https://203.0.113.5:8443",
            &pin_b,
            None,
        )
        .unwrap();
        let loaded = load_https_trust(&dir).unwrap().unwrap();
        assert_eq!(loaded.pins.get("https://192.0.2.10:8443").map(String::as_str), Some(pin_a.as_str()));
        assert_eq!(loaded.pins.get("https://198.51.100.10:8443").map(String::as_str), Some(pin_a.as_str()));
        assert_eq!(loaded.pins.get("https://203.0.113.5:8443").map(String::as_str), Some(pin_b.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
