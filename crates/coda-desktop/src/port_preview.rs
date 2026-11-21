//! Zero-exposure `localhost` HTTP port preview (`127.0.0.1:PORT`) and full-method live loopback
//! HTTP/1.1 E2EE proxy tunnel (`127.0.0.1:<ephemeral_port>`) for local and remote Coda hosts.

use std::collections::HashMap;
use std::future::Future;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::RemoteHostSummary;
use gpui::{
    div, prelude::*, px, rgb, rgba, ClickEvent, ClipboardItem, Context, FontWeight, SharedString,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::config::tr;
use crate::{palette, ActiveInputTarget, CodaDesktopState, CodaGpuWorkspaceView};

pub const QUICK_PREVIEW_PORTS: &[u16] = &[3000, 5173, 8000, 8080, 4321];
const MAX_DISPLAY_PREVIEW_CHARS: usize = 64 * 1024;
const MAX_HTTP_REQ_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_HTTP_REQ_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_LOCAL_TUNNEL_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TunnelTargetKey {
    pub device_id: Option<Uuid>,
    pub remote_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTunnelEntry {
    pub key: TunnelTargetKey,
    pub local_port: u16,
    pub url: String,
    pub target_label: String,
}

#[derive(Debug, Clone)]
pub struct PortPreviewState {
    pub port_input: String,
    pub path_input: String,
    pub loading: bool,
    pub status_code: Option<u16>,
    pub content_type: String,
    pub body_preview: String,
    pub tunnel_starting: bool,
    pub active_tunnel_url: Option<String>,
    pub active_tunnel_key: Option<TunnelTargetKey>,
    pub active_tunnels: Vec<ActiveTunnelEntry>,
}

impl Default for PortPreviewState {
    fn default() -> Self {
        Self {
            port_input: "3000".to_string(),
            path_input: "/".to_string(),
            loading: false,
            status_code: None,
            content_type: String::new(),
            body_preview: String::new(),
            tunnel_starting: false,
            active_tunnel_url: None,
            active_tunnel_key: None,
            active_tunnels: Vec::new(),
        }
    }
}

impl PortPreviewState {
    /// Rebuilds `active_tunnels` (and keeps `active_tunnel_key`/`active_tunnel_url` coherent)
    /// from the live `PortPreviewTunnelManager` registry.
    pub fn sync_from_manager(
        &mut self,
        mgr: &PortPreviewTunnelManager,
        remote_hosts: &[RemoteHostSummary],
        preferred_key: Option<TunnelTargetKey>,
    ) {
        let live = mgr.list_active();
        let entries: Vec<ActiveTunnelEntry> = live
            .into_iter()
            .map(|(key, local_port)| {
                let target_label = match key.device_id {
                    Some(did) => {
                        let host_name = remote_hosts
                            .iter()
                            .find(|h| h.id == did)
                            .map(|h| h.name.as_str())
                            .unwrap_or("E2EE");
                        format!("E2EE ({host_name}) -> 127.0.0.1:{}", key.remote_port)
                    }
                    None => format!("Local -> 127.0.0.1:{}", key.remote_port),
                };
                ActiveTunnelEntry {
                    key,
                    local_port,
                    url: format!("http://127.0.0.1:{local_port}/"),
                    target_label,
                }
            })
            .collect();

        let chosen = preferred_key
            .and_then(|pk| entries.iter().find(|e| e.key == pk))
            .or_else(|| {
                self.active_tunnel_key
                    .and_then(|ck| entries.iter().find(|e| e.key == ck))
            })
            .or_else(|| entries.last());

        if let Some(sel) = chosen {
            self.active_tunnel_key = Some(sel.key);
            self.active_tunnel_url = Some(sel.url.clone());
        } else {
            self.active_tunnel_key = None;
            self.active_tunnel_url = None;
        }
        self.active_tunnels = entries;
    }

    pub fn clear_tunnels(&mut self) {
        self.tunnel_starting = false;
        self.active_tunnel_url = None;
        self.active_tunnel_key = None;
        self.active_tunnels.clear();
    }
}

#[derive(Debug, Clone)]
pub struct ProxyHttpRequest {
    pub method: String,
    pub path_and_query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ProxyHttpResponse {
    pub status_code: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

struct ActiveTunnelHandle {
    local_port: u16,
    stop_tx: tokio::sync::watch::Sender<bool>,
}

/// Manages active loopback-only (`127.0.0.1:<ephemeral_port>`) HTTP proxy listeners keyed by `(device_id, remote_port)`.
#[derive(Default)]
pub struct PortPreviewTunnelManager {
    inner: Mutex<HashMap<TunnelTargetKey, ActiveTunnelHandle>>,
}

impl PortPreviewTunnelManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn reusable_local_port(&self, key: TunnelTargetKey) -> Option<u16> {
        let mut map = self.inner.lock().ok()?;
        if let Some(existing) = map.get(&key) {
            if !existing.stop_tx.is_closed() && !*existing.stop_tx.borrow() {
                return Some(existing.local_port);
            }
        }
        map.remove(&key);
        None
    }

    pub fn list_active(&self) -> Vec<(TunnelTargetKey, u16)> {
        let Ok(mut map) = self.inner.lock() else {
            return Vec::new();
        };
        map.retain(|_, handle| !handle.stop_tx.is_closed() && !*handle.stop_tx.borrow());
        let mut items: Vec<(TunnelTargetKey, u16)> =
            map.iter().map(|(&k, h)| (k, h.local_port)).collect();
        items.sort_by_key(|(k, _)| (k.device_id.map(|u| u.as_u128()), k.remote_port));
        items
    }

    pub fn register_tunnel(
        &self,
        key: TunnelTargetKey,
        local_port: u16,
        stop_tx: tokio::sync::watch::Sender<bool>,
    ) {
        if let Ok(mut map) = self.inner.lock() {
            if let Some(prev) = map.insert(
                key,
                ActiveTunnelHandle {
                    local_port,
                    stop_tx,
                },
            ) {
                let _ = prev.stop_tx.send(true);
            }
        }
    }

    pub fn stop_tunnel(&self, key: TunnelTargetKey) -> bool {
        let Ok(mut map) = self.inner.lock() else {
            return false;
        };
        if let Some(handle) = map.remove(&key) {
            let _ = handle.stop_tx.send(true);
            true
        } else {
            false
        }
    }

    pub fn stop_all_for_device(&self, device_id: Uuid) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        map.retain(|k, handle| {
            if k.device_id == Some(device_id) {
                let _ = handle.stop_tx.send(true);
                false
            } else {
                true
            }
        });
    }

    pub fn stop_all(&self) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        for (_, handle) in map.drain() {
            let _ = handle.stop_tx.send(true);
        }
    }
}

impl Drop for PortPreviewTunnelManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Validates a loopback preview `(port, path_and_query)` target, checking `.contains("://")` and
/// path-escape patterns strictly on the path segment before `'?'` so valid query URLs such as
/// `/_next/image?url=http://127.0.0.1:3000/logo.png` work cleanly (`Bug #6`).
pub fn validate_desktop_preview_target(port: u16, path: &str) -> Result<reqwest::Url, String> {
    if port < 1024 {
        return Err(format!(
            "Local preview port {port} blocked (privileged ports < 1024 are disallowed)"
        ));
    }
    if matches!(
        port,
        2375 | 2376 | 3306 | 5432 | 6379 | 9200 | 11211 | 24443 | 24444 | 27017
    ) {
        return Err(format!(
            "Local preview port {port} blocked (internal infrastructure/control port)"
        ));
    }
    let without_fragment = path.split('#').next().unwrap_or(path);
    if without_fragment.chars().any(|c| c.is_control()) {
        return Err("Invalid local preview path".into());
    }
    let path_before_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    if path_before_query.starts_with('@') || path_before_query.contains("://") {
        return Err("Invalid local preview path".into());
    }
    let clean_target = if without_fragment.starts_with('/') {
        without_fragment.to_string()
    } else {
        format!("/{without_fragment}")
    };
    let clean_path_only = clean_target.split('?').next().unwrap_or(&clean_target);
    if clean_path_only.contains('\\')
        || clean_path_only.starts_with("//")
        || clean_path_only.contains("://")
    {
        return Err("Invalid local preview path".into());
    }
    let raw_url = format!("http://127.0.0.1:{port}{clean_target}");
    let parsed = reqwest::Url::parse(&raw_url).map_err(|e| format!("Invalid preview URL: {e}"))?;
    if parsed.scheme() != "http"
        || parsed.host_str() != Some("127.0.0.1")
        || parsed.port() != Some(port)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Local preview target must remain strictly http://127.0.0.1".into());
    }
    Ok(parsed)
}

pub fn parse_preview_port_and_path(
    port_raw: &str,
    path_raw: &str,
) -> Result<(u16, String), String> {
    let port: u16 = port_raw
        .trim()
        .parse()
        .map_err(|_| "Port must be an integer between 1024 and 65535".to_string())?;
    let clean_path = if path_raw.trim().is_empty() {
        "/".to_string()
    } else if path_raw.trim().starts_with('/') {
        path_raw.trim().to_string()
    } else {
        format!("/{}", path_raw.trim())
    };
    validate_desktop_preview_target(port, &clean_path)?;
    Ok((port, clean_path))
}

pub fn format_preview_body_bytes(raw: &[u8], content_type: &str) -> String {
    let ct_lower = content_type.to_ascii_lowercase();
    if ct_lower.starts_with("image/")
        || ct_lower.starts_with("audio/")
        || ct_lower.starts_with("video/")
        || ct_lower.contains("wasm")
        || ct_lower.contains("octet-stream")
    {
        return format!(
            "(Binary payload: {} bytes, Content-Type: {content_type})",
            raw.len()
        );
    }
    let text = String::from_utf8_lossy(raw);
    let formatted = if ct_lower.contains("json") {
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| text.into_owned())
    } else {
        text.into_owned()
    };
    if formatted.chars().count() > MAX_DISPLAY_PREVIEW_CHARS {
        let mut s: String = formatted.chars().take(MAX_DISPLAY_PREVIEW_CHARS).collect();
        s.push_str("\n… [truncated at 64 KB]");
        s
    } else {
        formatted
    }
}

pub fn decode_remote_preview_raw_bytes(body_b64: &str) -> Result<Vec<u8>, String> {
    B64.decode(body_b64.trim())
        .map_err(|e| format!("Invalid base64 preview payload: {e}"))
}

pub fn decode_remote_preview_response(
    status_code: u16,
    content_type: &str,
    body_b64: &str,
) -> (u16, String, String) {
    let bytes = decode_remote_preview_raw_bytes(body_b64).unwrap_or_default();
    let formatted = format_preview_body_bytes(&bytes, content_type);
    (status_code, content_type.to_string(), formatted)
}

pub fn infer_content_type_from_path(path_and_query: &str) -> &'static str {
    let path_only = path_and_query
        .split('?')
        .next()
        .unwrap_or(path_and_query)
        .to_ascii_lowercase();
    if path_only.ends_with(".js") || path_only.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path_only.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path_only.ends_with(".json") || path_only.ends_with(".map") {
        "application/json; charset=utf-8"
    } else if path_only.ends_with(".svg") {
        "image/svg+xml"
    } else if path_only.ends_with(".png") {
        "image/png"
    } else if path_only.ends_with(".jpg") || path_only.ends_with(".jpeg") {
        "image/jpeg"
    } else if path_only.ends_with(".gif") {
        "image/gif"
    } else if path_only.ends_with(".webp") {
        "image/webp"
    } else if path_only.ends_with(".ico") {
        "image/x-icon"
    } else if path_only.ends_with(".wasm") {
        "application/wasm"
    } else if path_only.ends_with(".woff2") {
        "font/woff2"
    } else if path_only.ends_with(".woff") {
        "font/woff"
    } else if path_only.ends_with(".txt") {
        "text/plain; charset=utf-8"
    } else {
        "text/html; charset=utf-8"
    }
}

fn is_safe_forwarded_request_header(lower_name: &str) -> bool {
    if matches!(
        lower_name,
        "accept"
            | "accept-language"
            | "content-type"
            | "authorization"
            | "cookie"
            | "origin"
            | "referer"
            | "range"
            | "if-none-match"
            | "if-modified-since"
            | "cache-control"
            | "pragma"
            | "user-agent"
            | "sec-fetch-dest"
            | "sec-fetch-mode"
            | "sec-fetch-site"
            | "hx-request"
            | "hx-target"
            | "hx-trigger"
            | "hx-current-url"
    ) {
        return true;
    }
    if lower_name.starts_with("x-")
        && !lower_name.starts_with("x-forwarded-")
        && lower_name != "x-real-ip"
    {
        return lower_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    }
    false
}

fn is_safe_forwarded_response_header(lower_name: &str) -> bool {
    if matches!(
        lower_name,
        "location"
            | "set-cookie"
            | "cache-control"
            | "etag"
            | "last-modified"
            | "expires"
            | "vary"
            | "accept-ranges"
            | "content-range"
            | "content-disposition"
            | "www-authenticate"
            | "retry-after"
    ) {
        return true;
    }
    lower_name.starts_with("access-control-")
        && lower_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn sanitize_header_value(raw: &str) -> String {
    raw.split(['\r', '\n'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|&c| c == '\t' || !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

#[derive(Debug, Clone)]
pub struct ParsedHttpHead {
    pub method: String,
    pub path_and_query: String,
    pub headers: Vec<(String, String)>,
    pub content_length: usize,
    pub chunked: bool,
    pub body_offset: usize,
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(pos + 4)
    } else {
        buf.windows(2).position(|w| w == b"\n\n").map(|pos| pos + 2)
    }
}

/// Parses an HTTP/1.1 request head (`GET`/`HEAD`/`POST`/`PUT`/`PATCH`/`DELETE`/`OPTIONS`),
/// safe request headers, and `Content-Length` / `Transfer-Encoding` framing.
pub fn parse_http_proxy_request_head(raw: &[u8]) -> Result<ParsedHttpHead, (u16, &'static str)> {
    let body_offset = find_header_end(raw).unwrap_or(raw.len());
    let header_block = std::str::from_utf8(&raw[..body_offset])
        .map_err(|_| (400, "Malformed HTTP request headers"))?;
    let mut lines = header_block.lines();
    let first_line = lines
        .next()
        .ok_or((400, "Empty HTTP request line"))?
        .trim_end_matches('\r');
    let mut parts = first_line.split_whitespace();
    let method_raw = parts.next().ok_or((400, "Empty HTTP request line"))?;
    let raw_target = parts.next().ok_or((400, "Missing HTTP request target"))?;
    let _http_ver = parts.next().ok_or((400, "Missing HTTP version"))?;

    let method = match method_raw {
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS" => method_raw.to_string(),
        _ => {
            return Err((
                405,
                "Method not allowed by Coda Live Loopback HTTP/1.1 Tunnel",
            ));
        }
    };

    let without_fragment = raw_target.split('#').next().unwrap_or(raw_target);
    let path_only = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    if without_fragment.is_empty()
        || !without_fragment.starts_with('/')
        || path_only.starts_with("//")
        || path_only.contains('\\')
        || path_only.contains("://")
        || without_fragment.chars().any(|c| c.is_control())
    {
        return Err((400, "Invalid loopback proxy request path"));
    }

    let mut headers = Vec::new();
    let mut content_length: usize = 0;
    let mut chunked = false;

    for raw_line in lines {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((name_raw, val_raw)) = line.split_once(':') else {
            continue;
        };
        let lower = name_raw.trim().to_ascii_lowercase();
        let clean_val = sanitize_header_value(val_raw);
        if lower == "content-length" {
            let parsed_len = clean_val
                .parse::<usize>()
                .map_err(|_| (400, "Invalid Content-Length header"))?;
            if parsed_len > MAX_HTTP_REQ_BODY_BYTES {
                return Err((413, "Request body exceeds 2 MiB loopback tunnel limit"));
            }
            content_length = parsed_len;
            continue;
        }
        if lower == "transfer-encoding" {
            if clean_val.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
            continue;
        }
        if lower == "upgrade" && clean_val.eq_ignore_ascii_case("websocket") {
            return Err((501, "WebSocket upgrade is not supported over HTTP preview"));
        }
        if is_safe_forwarded_request_header(&lower) && !clean_val.is_empty() {
            headers.push((name_raw.trim().to_string(), clean_val));
        }
    }

    Ok(ParsedHttpHead {
        method,
        path_and_query: without_fragment.to_string(),
        headers,
        content_length,
        chunked,
        body_offset,
    })
}

/// Compatibility helper returning `(is_head, path_and_query)` for parsed HTTP/1.1 requests.
pub fn parse_http_proxy_request_target(
    raw_request: &[u8],
) -> Result<(bool, String), (u16, &'static str)> {
    let parsed = parse_http_proxy_request_head(raw_request)?;
    Ok((parsed.method == "HEAD", parsed.path_and_query))
}

/// Rewrites upstream `Location: http://127.0.0.1:<remote_port>/...` or `http://localhost:<remote_port>/...`
/// to `http://127.0.0.1:<local_port>/...` so `301`/`302`/`307`/`308` redirects stay inside the tunnel (`Bug #6`).
pub fn rewrite_proxy_location_header(location: &str, remote_port: u16, local_port: u16) -> String {
    let clean = sanitize_header_value(location);
    if remote_port == 0 || local_port == 0 {
        return clean;
    }
    for host in ["127.0.0.1", "localhost", "[::1]"] {
        let prefix = format!("http://{host}:{remote_port}");
        if clean.len() >= prefix.len() && clean[..prefix.len()].eq_ignore_ascii_case(&prefix) {
            let rest = &clean[prefix.len()..];
            if rest.is_empty()
                || rest.starts_with('/')
                || rest.starts_with('?')
                || rest.starts_with('#')
            {
                return format!("http://127.0.0.1:{local_port}{rest}");
            }
        }
    }
    clean
}

fn http_status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Response",
    }
}

/// Builds a wire-valid HTTP/1.1 response preserving safe response headers (`Location`, `Set-Cookie`,
/// `Cache-Control`, `Access-Control-*`, etc.) and rewriting loopback redirect ports (`Bug #6` / `Gap #1`).
pub fn build_http_11_response_with_headers(
    status_code: u16,
    content_type: &str,
    resp_headers: &[(String, String)],
    body: &[u8],
    head_only: bool,
    remote_port: u16,
    local_port: u16,
) -> Vec<u8> {
    let clean_ct = sanitize_header_value(content_type);
    let eff_ct = if clean_ct.is_empty() {
        "application/octet-stream"
    } else {
        clean_ct.as_str()
    };
    let reason = http_status_reason(status_code);
    let no_body_status = matches!(status_code, 100..=199 | 204 | 304);
    let content_len = if no_body_status {
        0
    } else if head_only && body.is_empty() {
        resp_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
            .unwrap_or(0)
    } else {
        body.len()
    };

    let mut header_str =
        format!("HTTP/1.1 {status_code} {reason}\r\nContent-Type: {eff_ct}\r\nContent-Length: {content_len}\r\n");

    let mut saw_cache_control = false;
    for (raw_name, raw_val) in resp_headers {
        let lower = raw_name.trim().to_ascii_lowercase();
        if !is_safe_forwarded_response_header(&lower) {
            continue;
        }
        let val = if lower == "location" {
            rewrite_proxy_location_header(raw_val, remote_port, local_port)
        } else {
            sanitize_header_value(raw_val)
        };
        if val.is_empty() {
            continue;
        }
        if lower == "cache-control" {
            saw_cache_control = true;
        }
        let clean_name: String = raw_name
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !clean_name.is_empty() {
            header_str.push_str(&format!("{clean_name}: {val}\r\n"));
        }
    }
    if !saw_cache_control {
        header_str.push_str("Cache-Control: no-store\r\n");
    }
    header_str.push_str("X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n");

    let emit_body = !head_only && !no_body_status;
    let mut out = Vec::with_capacity(header_str.len() + if emit_body { body.len() } else { 0 });
    out.extend_from_slice(header_str.as_bytes());
    if emit_body {
        out.extend_from_slice(body);
    }
    out
}

/// Builds a wire-valid HTTP/1.1 response with sanitized `Content-Type`, exact `Content-Length`, and `Connection: close`.
pub fn build_http_11_response(
    status_code: u16,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> Vec<u8> {
    build_http_11_response_with_headers(status_code, content_type, &[], body, head_only, 0, 0)
}

/// Executes a full-method local loopback HTTP request (`127.0.0.1:port`), returning `413` if the
/// response body exceeds `max_bytes` rather than silently truncating a `200 OK` bundle (`Bug #6`).
pub async fn fetch_local_port_preview_full(
    port: u16,
    req: ProxyHttpRequest,
    max_bytes: usize,
) -> Result<ProxyHttpResponse, String> {
    let url = validate_desktop_preview_target(port, &req.path_and_query)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "Failed building HTTP client".to_string())?;

    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| format!("Unsupported HTTP method: {}", req.method))?;
    let mut builder = client.request(method.clone(), url);
    for (k, v) in &req.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if !req.body.is_empty() {
        builder = builder.body(req.body);
    }

    let mut resp = builder
        .send()
        .await
        .map_err(|err| format!("Local port {port} unreachable: {err}"))?;
    let status = resp.status().as_u16();

    let mut resp_headers = Vec::new();
    let mut ct = String::new();
    for (name, value) in resp.headers().iter() {
        let Ok(val_str) = value.to_str() else {
            continue;
        };
        let lower = name.as_str().to_ascii_lowercase();
        if lower == "content-type" {
            ct = val_str.to_string();
        } else if lower == "content-length" {
            if let Ok(declared_len) = val_str.trim().parse::<usize>() {
                if declared_len > max_bytes && method != reqwest::Method::HEAD {
                    return Ok(ProxyHttpResponse {
                        status_code: 413,
                        content_type: "text/plain; charset=utf-8".to_string(),
                        headers: Vec::new(),
                        body: format!(
                            "Payload Too Large: upstream Content-Length ({declared_len} bytes) exceeds {max_bytes} bytes loopback limit"
                        )
                        .into_bytes(),
                    });
                }
            }
            resp_headers.push((name.as_str().to_string(), val_str.to_string()));
        } else if is_safe_forwarded_response_header(&lower) {
            resp_headers.push((name.as_str().to_string(), val_str.to_string()));
        }
    }

    let eff_ct = if ct.trim().is_empty() {
        infer_content_type_from_path(&req.path_and_query).to_string()
    } else {
        ct
    };

    if method == reqwest::Method::HEAD || matches!(status, 100..=199 | 204 | 304) {
        return Ok(ProxyHttpResponse {
            status_code: status,
            content_type: eff_ct,
            headers: resp_headers,
            body: Vec::new(),
        });
    }

    // Avoid hanging 8s on open-ended Server-Sent Events (`text/event-stream`) streams.
    if eff_ct
        .to_ascii_lowercase()
        .starts_with("text/event-stream")
    {
        let mut sse_buf = Vec::new();
        if let Ok(Ok(Some(chunk))) =
            tokio::time::timeout(Duration::from_millis(250), resp.chunk()).await
        {
            let cap = chunk.len().min(max_bytes);
            sse_buf.extend_from_slice(&chunk[..cap]);
        }
        return Ok(ProxyHttpResponse {
            status_code: status,
            content_type: eff_ct,
            headers: resp_headers,
            body: sse_buf,
        });
    }

    let mut buf = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if buf.len().saturating_add(chunk.len()) > max_bytes {
                    return Ok(ProxyHttpResponse {
                        status_code: 413,
                        content_type: "text/plain; charset=utf-8".to_string(),
                        headers: Vec::new(),
                        body: format!(
                            "Payload Too Large: response exceeds {max_bytes} bytes loopback preview limit"
                        )
                        .into_bytes(),
                    });
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(format!("Failed reading local port {port} body: {e}"));
            }
        }
    }

    Ok(ProxyHttpResponse {
        status_code: status,
        content_type: eff_ct,
        headers: resp_headers,
        body: buf,
    })
}

pub async fn fetch_local_port_preview_raw(
    port: u16,
    path_and_query: &str,
    max_bytes: usize,
) -> Result<(u16, String, Vec<u8>), String> {
    let resp = fetch_local_port_preview_full(
        port,
        ProxyHttpRequest {
            method: "GET".to_string(),
            path_and_query: path_and_query.to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        },
        max_bytes,
    )
    .await?;
    Ok((resp.status_code, resp.content_type, resp.body))
}

pub async fn fetch_local_port_preview(port: u16, path: &str) -> (u16, String, String) {
    match fetch_local_port_preview_raw(port, path, MAX_LOCAL_TUNNEL_BODY_BYTES).await {
        Ok((status, ct, buf)) => (status, ct.clone(), format_preview_body_bytes(&buf, &ct)),
        Err(msg) => {
            let code = if msg.contains("blocked") || msg.contains("Invalid") {
                403
            } else {
                502
            };
            (code, "text/plain; charset=utf-8".to_string(), msg)
        }
    }
}

async fn read_http_request_head_and_initial(
    stream: &mut tokio::net::TcpStream,
) -> Result<Vec<u8>, (u16, &'static str)> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        if find_header_end(&buf).is_some() {
            return Ok(buf);
        }
        if buf.len() >= MAX_HTTP_REQ_HEADER_BYTES {
            return Err((431, "Request Header Fields Too Large"));
        }
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|_| (400, "Failed reading HTTP request"))?;
        if n == 0 {
            return Ok(buf);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn read_chunked_body_bounded(
    stream: &mut tokio::net::TcpStream,
    initial: &[u8],
    max_body: usize,
) -> Result<Vec<u8>, (u16, &'static str)> {
    let mut wire = initial.to_vec();
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut scratch = [0u8; 4096];

    loop {
        while wire[cursor..].windows(2).position(|w| w == b"\r\n").is_none() {
            if wire.len().saturating_sub(cursor) > 128 {
                return Err((400, "Malformed chunked header"));
            }
            let n = stream
                .read(&mut scratch)
                .await
                .map_err(|_| (400, "Truncated chunked body"))?;
            if n == 0 {
                return Err((400, "Truncated chunked body"));
            }
            wire.extend_from_slice(&scratch[..n]);
        }
        let rel_end = wire[cursor..].windows(2).position(|w| w == b"\r\n").unwrap();
        let size_line = std::str::from_utf8(&wire[cursor..cursor + rel_end])
            .map_err(|_| (400, "Malformed chunk size"))?;
        let hex_part = size_line.split(';').next().unwrap_or("").trim();
        let chunk_len =
            usize::from_str_radix(hex_part, 16).map_err(|_| (400, "Invalid chunk size"))?;
        cursor += rel_end + 2;
        if chunk_len == 0 {
            return Ok(out);
        }
        if out.len().saturating_add(chunk_len) > max_body {
            return Err((413, "Request body exceeds 2 MiB loopback tunnel limit"));
        }
        let needed = cursor + chunk_len + 2;
        while wire.len() < needed {
            let n = stream
                .read(&mut scratch)
                .await
                .map_err(|_| (400, "Truncated chunked payload"))?;
            if n == 0 {
                return Err((400, "Truncated chunked payload"));
            }
            wire.extend_from_slice(&scratch[..n]);
        }
        if &wire[cursor + chunk_len..cursor + chunk_len + 2] != b"\r\n" {
            return Err((400, "Malformed chunked terminator"));
        }
        out.extend_from_slice(&wire[cursor..cursor + chunk_len]);
        cursor += chunk_len + 2;
    }
}

async fn read_full_http_proxy_request(
    stream: &mut tokio::net::TcpStream,
) -> Result<Option<ProxyHttpRequest>, (u16, &'static str)> {
    let raw = tokio::time::timeout(
        Duration::from_secs(5),
        read_http_request_head_and_initial(stream),
    )
    .await
    .map_err(|_| (408, "Request Timeout"))??;

    if raw.is_empty() {
        return Ok(None);
    }

    let head = parse_http_proxy_request_head(&raw)?;
    let initial_body = &raw[head.body_offset..];

    let body = if head.chunked {
        tokio::time::timeout(
            Duration::from_secs(10),
            read_chunked_body_bounded(stream, initial_body, MAX_HTTP_REQ_BODY_BYTES),
        )
        .await
        .map_err(|_| (408, "Request Body Timeout"))??
    } else if head.content_length > 0 {
        let mut body_buf = Vec::with_capacity(head.content_length);
        let copy_n = initial_body.len().min(head.content_length);
        body_buf.extend_from_slice(&initial_body[..copy_n]);
        let remaining = head.content_length - copy_n;
        if remaining > 0 {
            let start_pos = body_buf.len();
            body_buf.resize(head.content_length, 0);
            tokio::time::timeout(
                Duration::from_secs(10),
                stream.read_exact(&mut body_buf[start_pos..]),
            )
            .await
            .map_err(|_| (408, "Request Body Timeout"))?
            .map_err(|_| (400, "Incomplete HTTP request body"))?;
        }
        body_buf
    } else {
        Vec::new()
    };

    Ok(Some(ProxyHttpRequest {
        method: head.method,
        path_and_query: head.path_and_query,
        headers: head.headers,
        body,
    }))
}

/// Spawns a strictly loopback (`127.0.0.1:0`) full-method HTTP/1.1 proxy listener forwarding
/// `ProxyHttpRequest`s via `fetcher`, rewriting loopback redirect `Location` headers, and
/// acquiring concurrency permits outside `accept()` so `stop_tx` cancels instantaneously (`Bug #6` / `Gap #1`).
pub async fn spawn_loopback_http_proxy_with_full_fetcher<F, Fut>(
    remote_port: u16,
    fetcher: F,
) -> Result<(u16, tokio::sync::watch::Sender<bool>), String>
where
    F: Fn(u16, ProxyHttpRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ProxyHttpResponse, String>> + Send + 'static,
{
    validate_desktop_preview_target(remote_port, "/")?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| format!("Failed binding loopback tunnel on 127.0.0.1:0: {e}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| format!("Failed querying loopback tunnel port: {e}"))?;
    if !local_addr.ip().is_loopback() {
        return Err("Refusing non-loopback tunnel socket".into());
    }
    let local_port = local_addr.port();
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let fetcher = Arc::new(fetcher);
    let sem = Arc::new(tokio::sync::Semaphore::new(16));

    tokio::spawn(async move {
        loop {
            if *stop_rx.borrow() {
                break;
            }
            let permit = tokio::select! {
                biased;
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                    continue;
                }
                acquired = Arc::clone(&sem).acquire_owned() => {
                    match acquired {
                        Ok(p) => p,
                        Err(_) => break,
                    }
                }
            };

            let accepted = tokio::select! {
                biased;
                changed = stop_rx.changed() => {
                    drop(permit);
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                    continue;
                }
                res = listener.accept() => res,
            };

            let Ok((mut stream, peer_addr)) = accepted else {
                break;
            };
            if !peer_addr.ip().is_loopback() {
                continue;
            }

            let fetcher_conn = Arc::clone(&fetcher);
            tokio::spawn(async move {
                let _permit = permit;
                let proxy_req = match read_full_http_proxy_request(&mut stream).await {
                    Ok(Some(r)) => r,
                    Ok(None) => return,
                    Err((code, msg)) => {
                        let resp = build_http_11_response(
                            code,
                            "text/plain; charset=utf-8",
                            msg.as_bytes(),
                            false,
                        );
                        let _ = stream.write_all(&resp).await;
                        let _ = stream.shutdown().await;
                        return;
                    }
                };

                let is_head = proxy_req.method == "HEAD";
                let path_for_ct = proxy_req.path_and_query.clone();

                if let Err(msg) = validate_desktop_preview_target(remote_port, &path_for_ct) {
                    let resp = build_http_11_response(
                        403,
                        "text/plain; charset=utf-8",
                        msg.as_bytes(),
                        is_head,
                    );
                    let _ = stream.write_all(&resp).await;
                    let _ = stream.shutdown().await;
                    return;
                }

                let resp_bytes = match fetcher_conn(remote_port, proxy_req).await {
                    Ok(upstream) => {
                        let eff_ct = if upstream.content_type.trim().is_empty() {
                            infer_content_type_from_path(&path_for_ct).to_string()
                        } else {
                            upstream.content_type
                        };
                        build_http_11_response_with_headers(
                            upstream.status_code,
                            &eff_ct,
                            &upstream.headers,
                            &upstream.body,
                            is_head,
                            remote_port,
                            local_port,
                        )
                    }
                    Err(err) => build_http_11_response(
                        502,
                        "text/plain; charset=utf-8",
                        err.as_bytes(),
                        is_head,
                    ),
                };
                let _ = stream.write_all(&resp_bytes).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Ok((local_port, stop_tx))
}

/// Spawns a strictly loopback (`127.0.0.1:0`) HTTP/1.1 proxy listener forwarding `(port, path)` via `fetcher`.
pub async fn spawn_loopback_http_proxy_with_fetcher<F, Fut>(
    remote_port: u16,
    fetcher: F,
) -> Result<(u16, tokio::sync::watch::Sender<bool>), String>
where
    F: Fn(u16, String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(u16, String, Vec<u8>), String>> + Send + 'static,
{
    let fetcher_arc = Arc::new(fetcher);
    spawn_loopback_http_proxy_with_full_fetcher(remote_port, move |port, req| {
        let f = Arc::clone(&fetcher_arc);
        async move {
            let (status_code, content_type, body) = f(port, req.path_and_query).await?;
            Ok(ProxyHttpResponse {
                status_code,
                content_type,
                headers: Vec::new(),
                body,
            })
        }
    })
    .await
}

/// Opens a verified `http://127.0.0.1:<port>/...` URL in the OS default browser.
pub fn open_loopback_tunnel_in_browser(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "http"
        || parsed.host_str() != Some("127.0.0.1")
        || parsed.port().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(parsed.as_str())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(parsed.as_str())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

pub fn max_local_tunnel_body_bytes() -> usize {
    MAX_LOCAL_TUNNEL_BODY_BYTES
}

impl CodaGpuWorkspaceView {
    pub(crate) fn render_port_preview_drawer_body(
        &self,
        st: &CodaDesktopState,
        el: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let port_focus = st.input_target == ActiveInputTarget::PreviewPort;
        let path_focus = st.input_target == ActiveInputTarget::PreviewPath;
        let pv = &st.port_preview;
        let status_color = match pv.status_code {
            Some(200..=299) => palette::DONE_GREEN,
            Some(300..=399) => palette::WORKING_SKY,
            Some(400..=499) => palette::NEEDS_YOU_AMBER,
            Some(_) => palette::ERROR_RED,
            None => palette::TEXT_DIM,
        };
        let active_tunnels = pv.active_tunnels.clone();

        el.child(
            div()
                .p(px(9.0))
                .rounded(px(6.0))
                .bg(rgb(palette::SURFACE_BG))
                .border_1()
                .border_color(rgb(palette::BORDER))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(palette::TEXT_DIM))
                        .child(tr(
                            loc,
                            "Inspect 127.0.0.1:PORT or open full-method live loopback tunnels (zero public exposure)",
                            "通过加密通道探测 127.0.0.1:端口，或开启全方法本地回环实时隧道（零公网暴露）",
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("port-preview-port-input")
                                .occlude()
                                .w(px(74.0))
                                .px(px(7.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(rgb(palette::CHROME_BG))
                                .border_1()
                                .border_color(rgb(if port_focus { accent } else { palette::BORDER }))
                                .cursor_pointer()
                                .font_family("Menlo")
                                .text_size(px(11.5))
                                .text_color(rgb(palette::TEXT_PRIMARY))
                                .child(format!(
                                    ":{}",
                                    if pv.port_input.is_empty() {
                                        "3000".to_string()
                                    } else {
                                        format!(
                                            "{}{}",
                                            pv.port_input,
                                            if port_focus { "▏" } else { "" }
                                        )
                                    }
                                ))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.input_selected_all = true;
                                        g.input_target = ActiveInputTarget::PreviewPort;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("port-preview-path-input")
                                .occlude()
                                .flex_1()
                                .min_w_0()
                                .px(px(7.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(rgb(palette::CHROME_BG))
                                .border_1()
                                .border_color(rgb(if path_focus { accent } else { palette::BORDER }))
                                .cursor_pointer()
                                .font_family("Menlo")
                                .text_size(px(11.5))
                                .text_color(rgb(palette::TEXT_PRIMARY))
                                .truncate()
                                .child(format!(
                                    "{}{}",
                                    if pv.path_input.is_empty() { "/" } else { &pv.path_input },
                                    if path_focus { "▏" } else { "" }
                                ))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.input_selected_all = false;
                                        g.input_target = ActiveInputTarget::PreviewPath;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("btn-port-preview-fetch")
                                .occlude()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(rgb(accent))
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0x09090b))
                                .child(if pv.loading {
                                    "…"
                                } else {
                                    tr(loc, "Fetch", "请求")
                                })
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.trigger_active_port_preview();
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(5.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .children(QUICK_PREVIEW_PORTS.iter().copied().map(|p| {
                                    let is_cur = pv.port_input.trim() == p.to_string();
                                    div()
                                        .id(SharedString::from(format!("quick-port-{p}")))
                                        .occlude()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(if is_cur {
                                            palette::HOVER_BG
                                        } else {
                                            palette::CHROME_BG
                                        }))
                                        .border_1()
                                        .border_color(rgb(if is_cur {
                                            accent
                                        } else {
                                            palette::BORDER
                                        }))
                                        .cursor_pointer()
                                        .font_family("Menlo")
                                        .text_size(px(10.0))
                                        .text_color(rgb(if is_cur {
                                            accent
                                        } else {
                                            palette::TEXT_MUTED
                                        }))
                                        .child(format!(":{p}"))
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.port_preview.port_input = p.to_string();
                                            }
                                            this.trigger_active_port_preview();
                                            cx.notify();
                                        }))
                                })),
                        )
                        .child(
                            div()
                                .id("btn-port-open-tunnel")
                                .occlude()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .bg(rgb(palette::HOVER_BG))
                                .border_1()
                                .border_color(rgb(accent))
                                .hover(|h| h.bg(rgb(palette::CHROME_BG)))
                                .cursor_pointer()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(accent))
                                .child(if pv.tunnel_starting {
                                    tr(loc, "Starting…", "启动中…")
                                } else {
                                    tr(loc, "Open Live Tunnel", "开启实时隧道")
                                })
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.open_live_port_tunnel(true);
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .when(!active_tunnels.is_empty(), |el| {
            let count = active_tunnels.len();
            el.child(
                div()
                    .p(px(9.0))
                    .rounded(px(6.0))
                    .bg(rgba(0x22c55e14))
                    .border_1()
                    .border_color(rgba(0x22c55e66))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded(px(99.0))
                                            .bg(rgb(palette::DONE_GREEN)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::DONE_GREEN))
                                            .child(format!(
                                                "{} ({count})",
                                                tr(loc, "Live Loopback Tunnels", "本地实时回环隧道")
                                            )),
                                    ),
                            )
                            .when(count > 1, |row| {
                                row.child(
                                    div()
                                        .id("btn-port-stop-all-tunnels")
                                        .occlude()
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::CHROME_BG))
                                        .border_1()
                                        .border_color(rgb(palette::ERROR_RED))
                                        .cursor_pointer()
                                        .text_size(px(10.0))
                                        .text_color(rgb(palette::ERROR_RED))
                                        .child(tr(loc, "Stop All", "全部停止"))
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.stop_port_tunnel(None);
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .children(active_tunnels.into_iter().map(|entry| {
                        let key = entry.key;
                        let url_open = entry.url.clone();
                        let url_btn_open = entry.url.clone();
                        let url_copy = entry.url.clone();
                        let slug = format!(
                            "{}-{}",
                            key.device_id
                                .map(|u| u.to_string())
                                .unwrap_or_else(|| "local".to_string()),
                            key.remote_port
                        );
                        div()
                            .p(px(7.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::CHROME_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .font_family("Menlo")
                                            .text_size(px(10.0))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .truncate()
                                            .child(entry.target_label),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "btn-tunnel-open-{slug}"
                                                    )))
                                                    .occlude()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::SURFACE_BG))
                                                    .border_1()
                                                    .border_color(rgb(accent))
                                                    .cursor_pointer()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(accent))
                                                    .child(tr(loc, "Open", "打开"))
                                                    .on_click(cx.listener(
                                                        move |_, _: &ClickEvent, _, _| {
                                                            let _ = open_loopback_tunnel_in_browser(
                                                                &url_btn_open,
                                                            );
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "btn-tunnel-copy-{slug}"
                                                    )))
                                                    .occlude()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::SURFACE_BG))
                                                    .border_1()
                                                    .border_color(rgb(palette::BORDER))
                                                    .cursor_pointer()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(palette::TEXT_MUTED))
                                                    .child(tr(loc, "Copy", "复制"))
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            cx.write_to_clipboard(
                                                                ClipboardItem::new_string(
                                                                    url_copy.clone(),
                                                                ),
                                                            );
                                                            if let Ok(mut g) = this.state.lock() {
                                                                g.status_notice = Some(
                                                                    tr(
                                                                        g.locale,
                                                                        "Copied tunnel URL",
                                                                        "已复制本地隧道链接",
                                                                    )
                                                                    .to_string(),
                                                                );
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "btn-tunnel-stop-{slug}"
                                                    )))
                                                    .occlude()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::SURFACE_BG))
                                                    .border_1()
                                                    .border_color(rgb(palette::ERROR_RED))
                                                    .cursor_pointer()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(palette::ERROR_RED))
                                                    .child(tr(loc, "Stop", "停止"))
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            this.stop_port_tunnel(Some(key));
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("btn-tunnel-url-{slug}")))
                                    .occlude()
                                    .cursor_pointer()
                                    .font_family("Menlo")
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .truncate()
                                    .child(entry.url)
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                                        let _ = open_loopback_tunnel_in_browser(&url_open);
                                    })),
                            )
                    })),
            )
        })
        .when_some(pv.status_code, |el, code| {
            let lines: Vec<String> = pv
                .body_preview
                .lines()
                .take(260)
                .map(str::to_string)
                .collect();
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(status_color))
                            .child(format!("HTTP {code}")),
                    )
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(10.5))
                            .text_color(rgb(palette::TEXT_DIM))
                            .truncate()
                            .child(pv.content_type.clone()),
                    ),
            )
            .child(
                div()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .font_family("Menlo")
                    .text_size(px(11.0))
                    .flex()
                    .flex_col()
                    .gap(px(1.5))
                    .children(lines.into_iter().map(|line| {
                        div()
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child(if line.is_empty() { " ".to_string() } else { line })
                    })),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_preview_port_and_query_urls_with_scheme() {
        assert_eq!(
            parse_preview_port_and_path("5173", "api/health").unwrap(),
            (5173, "/api/health".to_string())
        );
        // Query strings containing `://` MUST be allowed (`/_next/image?url=http://...`)
        assert_eq!(
            parse_preview_port_and_path(
                "3000",
                "/_next/image?url=http://127.0.0.1:3000/logo.png&w=256"
            )
            .unwrap(),
            (
                3000,
                "/_next/image?url=http://127.0.0.1:3000/logo.png&w=256".to_string()
            )
        );
        assert!(parse_preview_port_and_path("3000", "//evil.com/path").is_err());
        assert!(parse_preview_port_and_path("3000", "/http://evil.com").is_err());
        assert!(parse_preview_port_and_path("80", "/").is_err());
        assert!(parse_preview_port_and_path("24444", "/").is_err());

        let (code, _, body) = decode_remote_preview_response(
            200,
            "application/json",
            &B64.encode(br#"{"ok":true,"port":5173}"#),
        );
        assert_eq!(code, 200);
        assert!(body.contains("\"ok\": true"));
    }

    #[test]
    fn test_http_proxy_request_parser_methods_headers_and_location_rewrite() {
        let (head, target) = parse_http_proxy_request_target(
            b"GET /_next/image?url=http://127.0.0.1:3000/a.png#ignored HTTP/1.1\r\nHost: 127.0.0.1:50000\r\n\r\n",
        )
        .unwrap();
        assert!(!head);
        assert_eq!(target, "/_next/image?url=http://127.0.0.1:3000/a.png");

        let post_head = parse_http_proxy_request_head(
            b"POST /api/login HTTP/1.1\r\nHost: 127.0.0.1:50000\r\nContent-Type: application/json\r\nAuthorization: Bearer secret\r\nCookie: sid=abc\r\nX-Forwarded-For: 1.2.3.4\r\nContent-Length: 15\r\n\r\n{\"user\":\"coda\"}",
        )
        .unwrap();
        assert_eq!(post_head.method, "POST");
        assert_eq!(post_head.content_length, 15);
        assert!(post_head
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer secret"));
        assert!(post_head
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("cookie") && v == "sid=abc"));
        assert!(!post_head
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("x-forwarded-for")));

        assert_eq!(
            parse_http_proxy_request_target(b"CONNECT evil.com:443 HTTP/1.1\r\n\r\n")
                .unwrap_err()
                .0,
            405
        );
        assert_eq!(
            parse_http_proxy_request_target(b"GET //evil.com/path HTTP/1.1\r\n\r\n")
                .unwrap_err()
                .0,
            400
        );

        assert_eq!(
            rewrite_proxy_location_header("http://127.0.0.1:3000/dashboard?ok=1", 3000, 54321),
            "http://127.0.0.1:54321/dashboard?ok=1"
        );
        assert_eq!(
            rewrite_proxy_location_header("http://localhost:3000/auth", 3000, 54321),
            "http://127.0.0.1:54321/auth"
        );
        // Port 30000 must NOT match remote_port 3000!
        assert_eq!(
            rewrite_proxy_location_header("http://127.0.0.1:30000/other", 3000, 54321),
            "http://127.0.0.1:30000/other"
        );

        let resp = build_http_11_response_with_headers(
            302,
            "text/html",
            &[
                (
                    "Location".to_string(),
                    "http://localhost:3000/welcome\r\nEvil: 1".to_string(),
                ),
                ("Set-Cookie".to_string(), "a=1; Path=/".to_string()),
                ("Set-Cookie".to_string(), "b=2; HttpOnly".to_string()),
                (
                    "Access-Control-Allow-Origin".to_string(),
                    "http://127.0.0.1:54321".to_string(),
                ),
            ],
            b"",
            false,
            3000,
            54321,
        );
        let wire = std::str::from_utf8(&resp).unwrap();
        assert!(wire.starts_with("HTTP/1.1 302 Found\r\n"));
        assert!(wire.contains("Location: http://127.0.0.1:54321/welcome\r\n"));
        assert!(wire.contains("Set-Cookie: a=1; Path=/\r\n"));
        assert!(wire.contains("Set-Cookie: b=2; HttpOnly\r\n"));
        assert!(wire.contains("Access-Control-Allow-Origin: http://127.0.0.1:54321\r\n"));
        assert!(!wire.contains("Evil:"));
    }

    #[tokio::test]
    async fn test_live_loopback_http_proxy_full_methods_redirects_413_and_multi_tunnel_stop() {
        let png_payload: Vec<u8> =
            vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff, 0x42];
        let png_clone = png_payload.clone();

        let (local_port, stop_tx) =
            spawn_loopback_http_proxy_with_full_fetcher(5173, move |port, req| {
                let png = png_clone.clone();
                async move {
                    assert_eq!(port, 5173);
                    match (req.method.as_str(), req.path_and_query.as_str()) {
                        ("POST", "/api/echo") => {
                            let auth = req
                                .headers
                                .iter()
                                .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                                .map(|(_, v)| v.clone())
                                .unwrap_or_default();
                            let mut out = req.body.clone();
                            out.extend_from_slice(format!("|{auth}").as_bytes());
                            Ok(ProxyHttpResponse {
                                status_code: 201,
                                content_type: "application/json".to_string(),
                                headers: vec![(
                                    "Set-Cookie".to_string(),
                                    "sess=live123; Path=/".to_string(),
                                )],
                                body: out,
                            })
                        }
                        ("GET", "/old") => Ok(ProxyHttpResponse {
                            status_code: 302,
                            content_type: "text/plain".to_string(),
                            headers: vec![(
                                "Location".to_string(),
                                "http://127.0.0.1:5173/logo.png".to_string(),
                            )],
                            body: Vec::new(),
                        }),
                        ("GET", "/logo.png") => Ok(ProxyHttpResponse {
                            status_code: 200,
                            content_type: "image/png".to_string(),
                            headers: Vec::new(),
                            body: png,
                        }),
                        _ => Ok(ProxyHttpResponse {
                            status_code: 404,
                            content_type: "text/plain".to_string(),
                            headers: Vec::new(),
                            body: b"missing".to_vec(),
                        }),
                    }
                }
            })
            .await
            .expect("spawn full-method loopback tunnel");

        let (local_port_b, stop_tx_b) =
            spawn_loopback_http_proxy_with_fetcher(8080, |_port, _path| async {
                Ok((200, "text/plain".to_string(), b"port-b".to_vec()))
            })
            .await
            .expect("spawn second tunnel");

        let mgr = PortPreviewTunnelManager::new();
        let key_a = TunnelTargetKey {
            device_id: Some(Uuid::nil()),
            remote_port: 5173,
        };
        let key_b = TunnelTargetKey {
            device_id: None,
            remote_port: 8080,
        };
        mgr.register_tunnel(key_a, local_port, stop_tx);
        mgr.register_tunnel(key_b, local_port_b, stop_tx_b);

        let mut pv_state = PortPreviewState::default();
        pv_state.sync_from_manager(&mgr, &[], Some(key_b));
        assert_eq!(pv_state.active_tunnels.len(), 2);

        let client = reqwest::Client::new();
        // POST with body + Authorization header
        let post_resp = client
            .post(format!("http://127.0.0.1:{local_port}/api/echo"))
            .header("Authorization", "Bearer tok-xyz")
            .body("{\"hello\":1}")
            .send()
            .await
            .unwrap();
        assert_eq!(post_resp.status().as_u16(), 201);
        assert_eq!(
            post_resp
                .headers()
                .get("set-cookie")
                .unwrap()
                .to_str()
                .unwrap(),
            "sess=live123; Path=/"
        );
        assert_eq!(post_resp.text().await.unwrap(), "{\"hello\":1}|Bearer tok-xyz");

        // 302 redirect from http://127.0.0.1:5173/old -> rewritten to http://127.0.0.1:<local_port>/logo.png!
        let redir_resp = client
            .get(format!("http://127.0.0.1:{local_port}/old"))
            .send()
            .await
            .unwrap();
        assert_eq!(redir_resp.status().as_u16(), 200);
        assert_eq!(redir_resp.bytes().await.unwrap().as_ref(), png_payload.as_slice());

        // Oversized > 2 MiB POST body rejected with 413
        let huge_req_resp = client
            .post(format!("http://127.0.0.1:{local_port}/api/echo"))
            .body(vec![b'x'; MAX_HTTP_REQ_BODY_BYTES + 64])
            .send()
            .await
            .unwrap();
        assert_eq!(huge_req_resp.status().as_u16(), 413);

        // Stopping Port B leaves Port A alive, and stopping Port A closes Port A!
        assert!(mgr.stop_tunnel(key_b));
        pv_state.sync_from_manager(&mgr, &[], None);
        assert_eq!(pv_state.active_tunnels.len(), 1);
        assert_eq!(pv_state.active_tunnel_key, Some(key_a));

        assert!(mgr.stop_tunnel(key_a));
        pv_state.sync_from_manager(&mgr, &[], None);
        assert!(pv_state.active_tunnels.is_empty());
        assert!(pv_state.active_tunnel_url.is_none());
    }

    #[tokio::test]
    async fn test_local_port_preview_returns_413_above_limit_instead_of_truncating() {
        let upstream = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let up_port = upstream.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = upstream.accept().await {
                let mut req_buf = [0u8; 512];
                let _ = s.read(&mut req_buf).await;
                let body = vec![b'A'; 2048];
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
            }
        });

        let (status, _ct, body) = fetch_local_port_preview_raw(up_port, "/bundle.js", 1024)
            .await
            .unwrap();
        assert_eq!(status, 413, "response > max_bytes must return 413, not truncated 200");
        assert!(String::from_utf8_lossy(&body).contains("Payload Too Large"));
    }
}
