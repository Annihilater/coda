//! Zero-exposure `localhost` HTTP port preview (`127.0.0.1:PORT`) and live loopback HTTP/1.1
//! E2EE proxy tunnel (`127.0.0.1:<ephemeral_port>`) for local and remote Coda hosts.

use std::collections::HashMap;
use std::future::Future;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::fs_sandbox::validate_local_preview_target;
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
const MAX_LOCAL_TUNNEL_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TunnelTargetKey {
    pub device_id: Option<Uuid>,
    pub remote_port: u16,
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
        }
    }
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

pub fn parse_preview_port_and_path(port_raw: &str, path_raw: &str) -> Result<(u16, String), String> {
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
    validate_local_preview_target(port, &clean_path)?;
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
        return format!("(Binary payload: {} bytes, Content-Type: {content_type})", raw.len());
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

/// Parses an incoming HTTP/1.x request line (`GET /path?query HTTP/1.1`), returning `(is_head, path_and_query)`.
pub fn parse_http_proxy_request_target(raw_request: &[u8]) -> Result<(bool, String), (u16, &'static str)> {
    let line_end = raw_request
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(raw_request.len());
    let first_line = std::str::from_utf8(&raw_request[..line_end])
        .map_err(|_| (400, "Malformed HTTP request line"))?
        .trim_end_matches('\r');
    let mut parts = first_line.split_whitespace();
    let method = parts.next().ok_or((400, "Empty HTTP request line"))?;
    let raw_target = parts.next().ok_or((400, "Missing HTTP request target"))?;
    let _http_ver = parts.next().ok_or((400, "Missing HTTP version"))?;

    let is_head = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return Err((405, "Only GET and HEAD are supported by Coda Live Tunnel")),
    };

    let without_fragment = raw_target.split('#').next().unwrap_or(raw_target);
    if without_fragment.is_empty()
        || !without_fragment.starts_with('/')
        || without_fragment.starts_with("//")
        || without_fragment.contains('\\')
        || without_fragment.contains("://")
        || without_fragment.chars().any(|c| c.is_control())
    {
        return Err((400, "Invalid loopback proxy request path"));
    }
    Ok((is_head, without_fragment.to_string()))
}

fn http_status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Response",
    }
}

/// Builds a wire-valid HTTP/1.1 response with sanitized `Content-Type`, exact `Content-Length`, and `Connection: close`.
pub fn build_http_11_response(
    status_code: u16,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> Vec<u8> {
    let first_line = content_type.split(['\r', '\n']).next().unwrap_or("");
    let clean_ct: String = first_line.chars().filter(|c| !c.is_control()).collect();
    let eff_ct = if clean_ct.trim().is_empty() {
        "application/octet-stream"
    } else {
        clean_ct.trim()
    };
    let reason = http_status_reason(status_code);
    let header = format!(
        "HTTP/1.1 {status_code} {reason}\r\nContent-Type: {eff_ct}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = Vec::with_capacity(header.len() + if head_only { 0 } else { body.len() });
    out.extend_from_slice(header.as_bytes());
    if !head_only {
        out.extend_from_slice(body);
    }
    out
}

pub async fn fetch_local_port_preview_raw(
    port: u16,
    path_and_query: &str,
    max_bytes: usize,
) -> Result<(u16, String, Vec<u8>), String> {
    let url = validate_local_preview_target(port, path_and_query)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "Failed building HTTP client".to_string())?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("Local port {port} unreachable: {err}"))?;
    let status = resp.status().as_u16();
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let eff_ct = if ct.trim().is_empty() {
        infer_content_type_from_path(path_and_query).to_string()
    } else {
        ct
    };
    let mut buf = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((status, eff_ct, buf))
}

pub async fn fetch_local_port_preview(port: u16, path: &str) -> (u16, String, String) {
    match fetch_local_port_preview_raw(port, path, 512 * 1024).await {
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

async fn read_http_request_headers(stream: &mut tokio::net::TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if buf.len() >= MAX_HTTP_REQ_HEADER_BYTES {
            break;
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n") {
            break;
        }
    }
    Ok(buf)
}

/// Spawns a strictly loopback (`127.0.0.1:0`) HTTP/1.1 proxy listener forwarding requests via `fetcher`.
pub async fn spawn_loopback_http_proxy_with_fetcher<F, Fut>(
    remote_port: u16,
    fetcher: F,
) -> Result<(u16, tokio::sync::watch::Sender<bool>), String>
where
    F: Fn(u16, String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(u16, String, Vec<u8>), String>> + Send + 'static,
{
    validate_local_preview_target(remote_port, "/")?;
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
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let Ok((mut stream, peer_addr)) = accepted else {
                        break;
                    };
                    if !peer_addr.ip().is_loopback() {
                        continue;
                    }
                    let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
                        break;
                    };
                    let fetcher_conn = Arc::clone(&fetcher);
                    tokio::spawn(async move {
                        let _permit = permit;
                        let req_bytes = match tokio::time::timeout(
                            Duration::from_secs(5),
                            read_http_request_headers(&mut stream),
                        )
                        .await
                        {
                            Ok(Ok(b)) if !b.is_empty() => b,
                            _ => return,
                        };

                        let (is_head, path_and_query) = match parse_http_proxy_request_target(&req_bytes) {
                            Ok(v) => v,
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

                        if let Err(msg) = validate_local_preview_target(remote_port, &path_and_query) {
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

                        let resp = match fetcher_conn(remote_port, path_and_query.clone()).await {
                            Ok((status_code, content_type, body)) => {
                                let eff_ct = if content_type.trim().is_empty() {
                                    infer_content_type_from_path(&path_and_query).to_string()
                                } else {
                                    content_type
                                };
                                build_http_11_response(status_code, &eff_ct, &body, is_head)
                            }
                            Err(err) => build_http_11_response(
                                502,
                                "text/plain; charset=utf-8",
                                err.as_bytes(),
                                is_head,
                            ),
                        };
                        let _ = stream.write_all(&resp).await;
                        let _ = stream.shutdown().await;
                    });
                }
            }
        }
    });

    Ok((local_port, stop_tx))
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
                            "Inspect 127.0.0.1:PORT or open a live loopback browser tunnel (zero public exposure)",
                            "通过加密通道探测 127.0.0.1:端口，或开启本地回环实时浏览器隧道（零公网暴露）",
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
        .when_some(pv.active_tunnel_url.clone(), |el, tunnel_url| {
            let url_open = tunnel_url.clone();
            let url_copy = tunnel_url.clone();
            let target_badge = match pv.active_tunnel_key {
                Some(TunnelTargetKey {
                    device_id: Some(_),
                    remote_port,
                }) => format!("E2EE -> 127.0.0.1:{remote_port}"),
                Some(TunnelTargetKey {
                    device_id: None,
                    remote_port,
                }) => format!("Local -> 127.0.0.1:{remote_port}"),
                None => "Loopback Tunnel".to_string(),
            };
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
                                            .child(tr(loc, "Live Loopback Tunnel", "本地实时隧道运行中")),
                                    )
                                    .child(
                                        div()
                                            .font_family("Menlo")
                                            .text_size(px(10.0))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(target_badge),
                                    ),
                            )
                            .child(
                                div()
                                    .id("btn-port-stop-tunnel")
                                    .occlude()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(palette::ERROR_RED))
                                    .cursor_pointer()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::ERROR_RED))
                                    .child(tr(loc, "Stop Tunnel", "停止隧道"))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.stop_active_port_tunnel();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("btn-port-reopen-tunnel-url")
                                    .occlude()
                                    .min_w_0()
                                    .flex_1()
                                    .cursor_pointer()
                                    .font_family("Menlo")
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .truncate()
                                    .child(tunnel_url)
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                                        let _ = open_loopback_tunnel_in_browser(&url_open);
                                    })),
                            )
                            .child(
                                div()
                                    .id("btn-port-copy-tunnel-url")
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .cursor_pointer()
                                    .text_size(px(10.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(tr(loc, "Copy", "复制"))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            url_copy.clone(),
                                        ));
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
                                    })),
                            ),
                    ),
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
    fn test_parse_preview_port_and_pretty_json() {
        assert_eq!(
            parse_preview_port_and_path("5173", "api/health").unwrap(),
            (5173, "/api/health".to_string())
        );
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
    fn test_http_proxy_request_parser_and_response_builder() {
        let (head, target) = parse_http_proxy_request_target(
            b"GET /assets/app.js?v=123#ignored HTTP/1.1\r\nHost: 127.0.0.1:50000\r\n\r\n",
        )
        .unwrap();
        assert!(!head);
        assert_eq!(target, "/assets/app.js?v=123");

        let (is_head, _) =
            parse_http_proxy_request_target(b"HEAD /index.html HTTP/1.1\r\n\r\n").unwrap();
        assert!(is_head);

        assert_eq!(
            parse_http_proxy_request_target(b"POST /mutate HTTP/1.1\r\n\r\n").unwrap_err().0,
            405
        );
        assert_eq!(
            parse_http_proxy_request_target(b"GET //evil.com/path HTTP/1.1\r\n\r\n")
                .unwrap_err()
                .0,
            400
        );

        let raw_png = b"\x89PNG\r\n\x1a\n\x00\xff\x10\x20";
        let resp = build_http_11_response(200, "image/png\r\nEvil: injected", raw_png, false);
        let split_pos = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&resp[..split_pos + 2]).unwrap();
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(headers.contains("Content-Type: image/png\r\n"));
        assert!(!headers.contains("Evil:"));
        assert!(headers.contains(&format!("Content-Length: {}\r\n", raw_png.len())));
        assert!(headers.contains("Connection: close\r\n"));
        assert_eq!(&resp[split_pos + 4..], raw_png);
    }

    #[tokio::test]
    async fn test_live_loopback_http_proxy_tunnel_preserves_binary_assets_and_stops() {
        let png_payload: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff, 0x42];
        let wasm_payload: Vec<u8> = vec![0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00, 0xde, 0xad];
        let png_clone = png_payload.clone();
        let wasm_clone = wasm_payload.clone();

        let (local_port, stop_tx) = spawn_loopback_http_proxy_with_fetcher(5173, move |port, path| {
            let png = png_clone.clone();
            let wasm = wasm_clone.clone();
            async move {
                assert_eq!(port, 5173);
                match path.as_str() {
                    "/?tab=live" => Ok((
                        200,
                        "text/html; charset=utf-8".to_string(),
                        b"<!doctype html><h1>Coda Tunnel</h1>".to_vec(),
                    )),
                    "/logo.png" => Ok((200, "image/png".to_string(), png)),
                    "/engine.wasm" => Ok((200, String::new(), wasm)),
                    _ => Ok((404, "text/plain".to_string(), b"missing".to_vec())),
                }
            }
        })
        .await
        .expect("spawn loopback tunnel");

        let mgr = PortPreviewTunnelManager::new();
        let key = TunnelTargetKey {
            device_id: Some(Uuid::nil()),
            remote_port: 5173,
        };
        mgr.register_tunnel(key, local_port, stop_tx);
        assert_eq!(mgr.reusable_local_port(key), Some(local_port));

        let client = reqwest::Client::new();
        let html_resp = client
            .get(format!("http://127.0.0.1:{local_port}/?tab=live"))
            .send()
            .await
            .unwrap();
        assert_eq!(html_resp.status().as_u16(), 200);
        assert_eq!(
            html_resp.text().await.unwrap(),
            "<!doctype html><h1>Coda Tunnel</h1>"
        );

        let png_resp = client
            .get(format!("http://127.0.0.1:{local_port}/logo.png"))
            .send()
            .await
            .unwrap();
        assert_eq!(png_resp.status().as_u16(), 200);
        assert_eq!(
            png_resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "image/png"
        );
        assert_eq!(png_resp.bytes().await.unwrap().as_ref(), png_payload.as_slice());

        let wasm_resp = client
            .get(format!("http://127.0.0.1:{local_port}/engine.wasm"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            wasm_resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/wasm"
        );
        assert_eq!(wasm_resp.bytes().await.unwrap().as_ref(), wasm_payload.as_slice());

        assert!(mgr.stop_tunnel(key));
        assert_eq!(mgr.reusable_local_port(key), None);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            client
                .get(format!("http://127.0.0.1:{local_port}/"))
                .send()
                .await
                .is_err(),
            "stopped tunnel must release loopback port"
        );
    }
}
