//! Zero-exposure `localhost` HTTP port preview (`127.0.0.1:PORT`) for local and remote E2EE hosts.

use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::fs_sandbox::validate_local_preview_target;
use gpui::{div, prelude::*, px, rgb, ClickEvent, Context, FontWeight, SharedString};

use crate::config::tr;
use crate::{palette, ActiveInputTarget, CodaDesktopState, CodaGpuWorkspaceView};

pub const QUICK_PREVIEW_PORTS: &[u16] = &[3000, 5173, 8000, 8080, 4321];
const MAX_DISPLAY_PREVIEW_CHARS: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct PortPreviewState {
    pub port_input: String,
    pub path_input: String,
    pub loading: bool,
    pub status_code: Option<u16>,
    pub content_type: String,
    pub body_preview: String,
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
        }
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

pub fn decode_remote_preview_response(
    status_code: u16,
    content_type: &str,
    body_b64: &str,
) -> (u16, String, String) {
    let bytes = B64.decode(body_b64.trim()).unwrap_or_default();
    let formatted = format_preview_body_bytes(&bytes, content_type);
    (status_code, content_type.to_string(), formatted)
}

pub async fn fetch_local_port_preview(port: u16, path: &str) -> (u16, String, String) {
    let url = match validate_local_preview_target(port, path) {
        Ok(u) => u,
        Err(msg) => return (403, "text/plain".to_string(), msg),
    };
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(6))
        .build()
    else {
        return (500, "text/plain".to_string(), "Failed building HTTP client".to_string());
    };
    match client.get(url).send().await {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/plain; charset=utf-8")
                .to_string();
            let mut buf = Vec::new();
            while let Ok(Some(chunk)) = resp.chunk().await {
                if buf.len().saturating_add(chunk.len()) > 512 * 1024 {
                    break;
                }
                buf.extend_from_slice(&chunk);
            }
            (status, ct.clone(), format_preview_body_bytes(&buf, &ct))
        }
        Err(err) => (
            502,
            "text/plain".to_string(),
            format!("Local port {port} unreachable: {err}"),
        ),
    }
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
                            "Inspect 127.0.0.1:PORT on active host (zero public exposure)",
                            "通过加密通道探测当前主机 127.0.0.1:端口（零公网暴露）",
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
                                .border_color(rgb(if is_cur { accent } else { palette::BORDER }))
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
                ),
        )
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
}
