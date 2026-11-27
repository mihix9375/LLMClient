use crate::config::{load_https_trust, normalize_sha256_pin, HttpsTrust};
use crate::tls_http::{
    parse_api_url, parse_reply_bytes, sha256_upper_hex, verify_cert_validity_and_san_at,
};
use crate::win_util::write_file_atomic;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const MAX_PROBE_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct BootstrapCertVerifier {
    algs: WebPkiSupportedAlgorithms,
    captured_der: Arc<Mutex<Option<Vec<u8>>>>,
}

impl ServerCertVerifier for BootstrapCertVerifier {
    fn verify_server_cert(
        &self, end_entity: &CertificateDer<'_>, _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>, _ocsp_response: &[u8], now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        verify_cert_validity_and_san_at(end_entity.as_ref(), server_name, now)?;
        *self.captured_der.lock() = Some(end_entity.as_ref().to_vec());
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(msg, cert, dss, &self.algs)
    }
    fn verify_tls13_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(msg, cert, dss, &self.algs)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> { self.algs.supported_schemes() }
}

pub fn der_to_pem(der: &[u8]) -> String {
    let b64 = B64.encode(der);
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        if let Ok(line) = std::str::from_utf8(chunk) { out.push_str(line); out.push('\n'); }
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

pub fn load_pin_from_ca_file(path: &Path) -> Result<String, String> {
    let raw = fs::read(path).map_err(|e| format!("QWEN_CA_FILE ({}) の読込に失敗しました: {}", path.display(), e))?;
    let der = match std::str::from_utf8(&raw) {
        Ok(text) if text.contains("-----BEGIN CERTIFICATE-----") => {
            let b64: String = text.lines().map(str::trim)
                .skip_while(|&l| l != "-----BEGIN CERTIFICATE-----").skip(1)
                .take_while(|&l| l != "-----END CERTIFICATE-----").collect();
            B64.decode(b64).map_err(|e| format!("QWEN_CA_FILE PEM デコード失敗: {}", e))?
        }
        _ => raw,
    };
    if der.is_empty() { return Err("QWEN_CA_FILE に有効な証明書が含まれていません".into()); }
    normalize_sha256_pin(&sha256_upper_hex(&der))
}

pub fn save_https_trust_entry(
    dir: &Path, normalized_url: &str, sha256_upper: &str, cert_der: Option<&[u8]>,
) -> Result<HttpsTrust, String> {
    let pin = normalize_sha256_pin(sha256_upper)?;
    let _ = fs::create_dir_all(dir);
    let (mut urls, mut pins) = (Vec::<String>::new(), HashMap::<String, String>::new());
    let (mut primary_url, mut primary_sha) = (normalized_url.to_string(), pin.clone());
    if let Some(ex) = load_https_trust(dir).ok().flatten() {
        if let Ok(ex_pin) = normalize_sha256_pin(&ex.sha256) {
            primary_sha = ex_pin.clone();
            for u in &ex.urls {
                if !urls.iter().any(|x| x.eq_ignore_ascii_case(u)) { urls.push(u.clone()); }
                pins.entry(u.clone()).or_insert_with(|| ex_pin.clone());
            }
        }
        for (u, p) in ex.pins {
            if let Ok(np) = normalize_sha256_pin(&p) {
                if !urls.iter().any(|x| x.eq_ignore_ascii_case(&u)) { urls.push(u.clone()); }
                pins.insert(u, np);
            }
        }
        if let Some(first) = urls.first() { primary_url = first.clone(); }
    }
    if !urls.iter().any(|x| x.eq_ignore_ascii_case(normalized_url)) { urls.push(normalized_url.to_string()); }
    pins.insert(normalized_url.to_string(), pin.clone());
    let mut keys: Vec<&String> = pins.keys().collect();
    keys.sort();
    let mut pins_obj = Map::new();
    for k in keys { pins_obj.insert(k.clone(), json!(pins[k])); }
    if let Some(der) = cert_der {
        let cert_path = dir.join("certs").join("server.crt");
        if !cert_path.exists() || primary_sha == pin {
            let _ = fs::create_dir_all(dir.join("certs"));
            let _ = write_file_atomic(&cert_path, der_to_pem(der).as_bytes());
        }
    }
    let payload = json!({
        "url": primary_url, "urls": urls, "sha256": primary_sha,
        "certificate": "certs/server.crt", "pins": Value::Object(pins_obj)
    });
    let body = serde_json::to_string_pretty(&payload).map_err(|e| format!("https-server.json 生成失敗: {}", e))?;
    write_file_atomic(&dir.join("https-server.json"), format!("{}\n", body).as_bytes())
        .map_err(|e| format!("https-server.json 保存失敗: {}", e))?;
    Ok(HttpsTrust { urls, sha256: pin, pins })
}

fn read_probe_response(stream: &mut dyn Read) -> Result<(u16, Vec<u8>), String> {
    let (mut buf, mut tmp, mut hdr_end, mut cl) = (Vec::with_capacity(4096), [0u8; 4096], None, None);
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.len() > MAX_PROBE_BYTES { return Err("TLS 応答が上限を超えました".into()); }
                if hdr_end.is_none() {
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        hdr_end = Some(pos + 4);
                        for line in String::from_utf8_lossy(&buf[..pos]).lines().skip(1) {
                            if let Some((k, v)) = line.split_once(':') {
                                if k.trim().eq_ignore_ascii_case("content-length") { cl = v.trim().parse::<usize>().ok(); }
                            }
                        }
                    }
                }
                if let (Some(he), Some(len)) = (hdr_end, cl) { if buf.len() >= he + len { break; } }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
            Err(e) => return Err(format!("TLS 読取エラー: {}", e)),
        }
    }
    let he = hdr_end.ok_or_else(|| "サーバーから有効な HTTP 応答がありません".to_string())?;
    let status = String::from_utf8_lossy(&buf[..he]).lines().next()
        .and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let body = match cl { Some(len) if buf.len() >= he + len => buf[he..he + len].to_vec(), _ => buf[he..].to_vec() };
    Ok((status, body))
}

pub fn probe_and_generate_https_trust(raw_url: &str, trust_dir: &Path, timeout_ms: u64) -> Result<HttpsTrust, String> {
    let parsed = parse_api_url(raw_url)?;
    if !parsed.secure { return Err("HTTPS URL が必要です".into()); }
    let addrs: Vec<SocketAddr> = (parsed.host.as_str(), parsed.port)
        .to_socket_addrs().map_err(|e| format!("ホスト解決失敗 ({}): {}", raw_url, e))?.collect();
    let timeout = Duration::from_millis(timeout_ms.clamp(1000, 10000));
    let mut last_err = String::new();
    let tcp = addrs.into_iter().find_map(|a| match TcpStream::connect_timeout(&a, timeout) {
        Ok(s) => Some(s), Err(e) => { last_err = e.to_string(); None }
    }).ok_or_else(|| format!("API 接続失敗 ({}): {}", raw_url, last_err))?;
    let _ = tcp.set_read_timeout(Some(timeout));
    let _ = tcp.set_write_timeout(Some(timeout));
    let provider = rustls::crypto::ring::default_provider();
    let captured = Arc::new(Mutex::new(None));
    let verifier = Arc::new(BootstrapCertVerifier {
        algs: provider.signature_verification_algorithms, captured_der: Arc::clone(&captured),
    });
    let cfg = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions().map_err(|e| format!("TLS 初期化失敗: {}", e))?
        .dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth();
    let sname = ServerName::try_from(parsed.host.clone()).map_err(|_| format!("不正なホスト名: {}", parsed.host))?;
    let mut tls = StreamOwned::new(ClientConnection::new(Arc::new(cfg), sname).map_err(|e| format!("TLS 開始失敗: {}", e))?, tcp);
    let host_hdr = if parsed.host.contains(':') { format!("[{}]:{}", parsed.host, parsed.port) } else { format!("{}:{}", parsed.host, parsed.port) };
    let path = if parsed.base_path.is_empty() { "/health".to_string() } else { format!("{}/health", parsed.base_path) };
    let req = format!("GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Qwen27-Rust/1.0\r\nAccept: application/json\r\nConnection: close\r\n\r\n", path, host_hdr);
    tls.write_all(req.as_bytes()).map_err(|e| format!("TLS ハンドシェイク/送信失敗 ({}): {}", raw_url, e))?;
    let _ = tls.flush();
    let (status, body) = read_probe_response(&mut tls)?;
    let val = parse_reply_bytes(&body).unwrap_or_else(|_| json!({}));
    if status != 200 && status != 503 {
        let msg = val.get("error").and_then(|v| v.as_str()).unwrap_or("接続が拒否されました");
        return Err(format!("証明書自動取得に失敗 (HTTP {}): {}", status, msg));
    }
    if val.get("service").and_then(|v| v.as_str()) != Some("qwen27-framework") {
        return Err("接続先が Qwen27 Framework API ではないため https-server.json を生成しませんでした".into());
    }
    let der = captured.lock().clone().ok_or_else(|| "サーバー証明書を取得できませんでした".to_string())?;
    save_https_trust_entry(trust_dir, &parsed.full_normalized, &sha256_upper_hex(&der), Some(&der))
}
