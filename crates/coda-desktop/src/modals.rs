//! Command Palette (`Cmd+P` / `Cmd+K`) and Settings Modal (`Cmd+,`) overlays for `coda-desktop`.

use std::sync::Arc;

use coda_herdr::discover_hc_binary;
use gpui::{
    div, prelude::*, px, rgb, rgba, App, ClickEvent, Context, FontWeight, MouseButton,
    SharedString, Window,
};

use crate::config::{
    config_root_dir, display_compact_path, reveal_in_finder, tr, LayoutMode, MalleableLayoutConfig,
    UiLocale,
};
use crate::remote_mesh::spawn_device_login;
use crate::search_export::format_host_health_compact;
use crate::{
    palette, ActiveInputTarget, CodaDesktopState, CodaGpuWorkspaceView, SettingsSection,
};

impl CodaGpuWorkspaceView {
    pub(crate) fn render_command_palette_overlay(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let items = st.palette_items();
        let selected = st.palette_selected.min(items.len().saturating_sub(1));

        div()
            .id("palette-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000099))
            .pt(px(72.0))
            .flex()
            .justify_center()
            .items_start()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.palette_open = false;
                    g.input_target = ActiveInputTarget::TerminalDirect;
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("palette-modal")
                    .occlude()
                    .w(px(540.0))
                    .rounded(px(10.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER_STRONG))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .h(px(42.0))
                            .px(px(14.0))
                            .border_b_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(accent))
                                    .child("⌘P"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .text_color(rgb(if st.palette_query.is_empty() {
                                        palette::TEXT_DIM
                                    } else {
                                        palette::TEXT_PRIMARY
                                    }))
                                    .child(if st.palette_query.is_empty() {
                                        tr(
                                            loc,
                                            "Jump to space, terminal, layout, or command...",
                                            "搜索工作区、终端会话、视图或命令...",
                                        )
                                        .to_string()
                                    } else {
                                        format!("{}|", st.palette_query)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("palette-list")
                            .max_h(px(360.0))
                            .p(px(6.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(items.into_iter().enumerate().map(|(idx, item)| {
                                let is_sel = idx == selected;
                                let action = item.action.clone();
                                div()
                                    .id(SharedString::from(format!("pal-item-{idx}")))
                                    .occlude()
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(if is_sel {
                                        palette::HOVER_BG
                                    } else {
                                        palette::CHROME_BG
                                    }))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .px(px(5.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(palette::SURFACE_BG))
                                                    .text_size(px(9.5))
                                                    .text_color(rgb(accent))
                                                    .child(item.kind_tag),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.5))
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .child(item.label),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .truncate()
                                            .child(item.detail),
                                    )
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.execute_palette_action(action.clone());
                                            cx.notify();
                                        },
                                    ))
                            })),
                    ),
            )
    }

    pub(crate) fn render_settings_modal_overlay(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let active_sec = st.settings_section;
        let section_title = match active_sec {
            SettingsSection::Appearance => tr(loc, "Appearance & Layout", "外观与布局"),
            SettingsSection::Runtime => tr(loc, "Terminals & Spaces", "终端与工作区"),
            SettingsSection::Machines => tr(loc, "Account & Remote Nodes", "账号与远端节点"),
            SettingsSection::Shortcuts => tr(loc, "Keyboard Shortcuts", "快捷键"),
        };

        let nav_items: &[(SettingsSection, &str, &str)] = &[
            (SettingsSection::Appearance, "Appearance", "外观与布局"),
            (SettingsSection::Runtime, "Terminals & Spaces", "终端与工作区"),
            (SettingsSection::Machines, "Account & Nodes", "账号与远端节点"),
            (SettingsSection::Shortcuts, "Shortcuts", "快捷键"),
        ];

        let content = match active_sec {
            SettingsSection::Appearance => {
                self.render_settings_appearance_pane(st, cx).into_any_element()
            }
            SettingsSection::Runtime => {
                self.render_settings_runtime_pane(st, cx).into_any_element()
            }
            SettingsSection::Machines => {
                self.render_settings_machines_pane(st, cx).into_any_element()
            }
            SettingsSection::Shortcuts => self.render_settings_shortcuts_pane(st).into_any_element(),
        };

        div()
            .id("settings-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .bg(rgba(0x000000aa))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.settings_open = false;
                    g.clear_settings_ephemeral_inputs();
                    g.input_target = ActiveInputTarget::TerminalDirect;
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("settings-modal")
                    .occlude()
                    .w(px(720.0))
                    .h(px(500.0))
                    .rounded(px(10.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER_STRONG))
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        // Left navigation rail (184px)
                        div()
                            .w(px(184.0))
                            .h_full()
                            .flex_shrink_0()
                            .bg(rgb(palette::SIDEBAR_BG))
                            .border_r_1()
                            .border_color(rgb(palette::BORDER))
                            .p(px(10.0))
                            .flex()
                            .flex_col()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .child(tr(loc, "Settings", "设置")),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(palette::TEXT_DIM))
                                                    .child("⌘,"),
                                            ),
                                    )
                                    .children(nav_items.iter().map(|&(sec, en, zh)| {
                                        let is_active = active_sec == sec;
                                        div()
                                            .id(SharedString::from(format!("settings-nav-{sec:?}")))
                                            .occlude()
                                            .px(px(9.0))
                                            .py(px(6.0))
                                            .rounded(px(6.0))
                                            .bg(rgb(if is_active {
                                                palette::HOVER_BG
                                            } else {
                                                palette::SIDEBAR_BG
                                            }))
                                            .border_1()
                                            .border_color(rgb(if is_active {
                                                accent
                                            } else {
                                                palette::SIDEBAR_BG
                                            }))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .cursor_pointer()
                                            .text_size(px(12.0))
                                            .text_color(rgb(if is_active {
                                                palette::TEXT_PRIMARY
                                            } else {
                                                palette::TEXT_MUTED
                                            }))
                                            .child(tr(loc, en, zh))
                                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                let mut should_fetch_wh = false;
                                                if let Ok(mut g) = this.state.lock() {
                                                    if g.settings_section != sec {
                                                        g.clear_settings_ephemeral_inputs();
                                                        if sec == SettingsSection::Machines && g.is_logged_in {
                                                            should_fetch_wh = true;
                                                        }
                                                    }
                                                    g.settings_section = sec;
                                                    g.input_target =
                                                        ActiveInputTarget::TerminalDirect;
                                                }
                                                if should_fetch_wh {
                                                    this.reload_notification_webhooks();
                                                }
                                                cx.notify();
                                            }))
                                    })),
                            )
                            .child(
                                div()
                                    .id("btn-settings-open-config-dir")
                                    .occlude()
                                    .px(px(8.0))
                                    .py(px(6.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(tr(
                                        loc,
                                        "Open ~/.config/coda",
                                        "打开 ~/.config/coda",
                                    ))
                                    .on_click(cx.listener(|_, _: &ClickEvent, _, _| {
                                        if let Some(dir) = config_root_dir() {
                                            let _ = std::fs::create_dir_all(&dir);
                                            reveal_in_finder(&dir.to_string_lossy());
                                        }
                                    })),
                            ),
                    )
                    .child(
                        // Right section content column
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(42.0))
                                    .flex_shrink_0()
                                    .px(px(16.0))
                                    .border_b_1()
                                    .border_color(rgb(palette::BORDER))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(10.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .child(section_title),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(if st.status_notice.is_some() {
                                                        palette::NEEDS_YOU_AMBER
                                                    } else {
                                                        palette::TEXT_DIM
                                                    }))
                                                    .child(
                                                        st.status_notice.clone().unwrap_or_else(|| {
                                                            tr(
                                                                loc,
                                                                "Changes save & apply immediately",
                                                                "更改自动保存并实时生效",
                                                            )
                                                            .to_string()
                                                        }),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("btn-close-settings")
                                            .occlude()
                                            .w(px(24.0))
                                            .h(px(24.0))
                                            .rounded(px(5.0))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(14.0))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child("×")
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.settings_open = false;
                                                    g.clear_settings_ephemeral_inputs();
                                                    g.input_target =
                                                        ActiveInputTarget::TerminalDirect;
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p(px(14.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .overflow_hidden()
                                    .child(content),
                            ),
                    ),
            )
    }

    fn render_settings_appearance_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let font_px = st.layout.terminal_font_size_px.clamp(10.0, 22.0);
        let sidebar_w_px = st.layout.sidebar_width_px.clamp(200.0, 320.0);

        let accent_swatches: &[(&str, u32)] = &[
            ("#38bdf8", 0x38bdf8),
            ("#22c55e", 0x22c55e),
            ("#a78bfa", 0xa78bfa),
            ("#f59e0b", 0xf59e0b),
            ("#f472b6", 0xf472b6),
            ("#2dd4bf", 0x2dd4bf),
        ];
        let font_choices: &[&str] = &["Menlo", "SF Mono", "JetBrains Mono", "Monaco"];

        div()
            .id("settings-appearance-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(render_settings_row(
                tr(loc, "Language", "界面语言"),
                tr(loc, "Desktop UI localization", "切换桌面客户端显示语言"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(self.render_setting_pill(
                        "settings-lang-zh",
                        "中文",
                        loc == UiLocale::Zh,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.set_locale(UiLocale::Zh);
                            cx.notify();
                        }),
                    ))
                    .child(self.render_setting_pill(
                        "settings-lang-en",
                        "English",
                        loc == UiLocale::En,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.set_locale(UiLocale::En);
                            cx.notify();
                        }),
                    )),
            ))
            .child(render_settings_row(
                tr(loc, "Workspace Layout", "默认主视图"),
                tr(
                    loc,
                    "Main workbench or terminals grid",
                    "选择专注终端工作台或平铺网格",
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(self.render_setting_pill(
                        "settings-layout-workbench",
                        tr(loc, "Main", "主工作台"),
                        st.layout.layout == LayoutMode::Workbench,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.layout = LayoutMode::Workbench;
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    ))
                    .child(self.render_setting_pill(
                        "settings-layout-terminals",
                        tr(loc, "Terminals", "终端平铺"),
                        st.layout.layout == LayoutMode::Terminals,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.layout = LayoutMode::Terminals;
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    )),
            ))
            .child(render_settings_row(
                tr(loc, "Accent Color", "主题强调色"),
                tr(loc, "Active indicators & focus rings", "用于高亮标签、边框和活动指示器"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .children(accent_swatches.iter().map(|&(hex, color_val)| {
                        let is_sel = st.layout.accent_hex.eq_ignore_ascii_case(hex);
                        let hex_owned = hex.to_string();
                        div()
                            .id(SharedString::from(format!("settings-swatch-{hex}")))
                            .occlude()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(99.0))
                            .bg(rgb(color_val))
                            .border_2()
                            .border_color(rgb(if is_sel {
                                palette::TEXT_PRIMARY
                            } else {
                                palette::SURFACE_BG
                            }))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.layout.accent_hex = hex_owned.clone();
                                }
                                this.persist_layout_config();
                                cx.notify();
                            }))
                    })),
            ))
            .child(render_settings_row(
                tr(loc, "Terminal Font", "终端字体"),
                tr(loc, "Monospace font family", "终端渲染使用的等宽字体"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .children(font_choices.iter().map(|&font| {
                        let is_sel = st.layout.font_family.eq_ignore_ascii_case(font);
                        let font_owned = font.to_string();
                        self.render_setting_pill(
                            SharedString::from(format!("settings-font-{font}")),
                            font,
                            is_sel,
                            accent,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.layout.font_family = font_owned.clone();
                                }
                                this.persist_layout_config();
                                cx.notify();
                            }),
                        )
                    })),
            ))
            .child(render_settings_row(
                tr(loc, "Terminal Font Size", "终端字号"),
                tr(loc, "10 px – 22 px", "可调节范围 10 px – 22 px"),
                self.render_stepper_control(
                    "font-size",
                    format!("{} px", font_px as u32),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Ok(mut g) = this.state.lock() {
                            g.layout.terminal_font_size_px =
                                (g.layout.terminal_font_size_px - 1.0).max(10.0);
                        }
                        this.persist_layout_config();
                        cx.notify();
                    }),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Ok(mut g) = this.state.lock() {
                            g.layout.terminal_font_size_px =
                                (g.layout.terminal_font_size_px + 1.0).min(22.0);
                        }
                        this.persist_layout_config();
                        cx.notify();
                    }),
                ),
            ))
            .child(render_settings_row(
                tr(loc, "Sidebar", "左侧边栏"),
                tr(
                    loc,
                    "Toggle visibility and width (200–320 px)",
                    "控制显示状态与宽度 (200–320 px)",
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(self.render_setting_pill(
                        "settings-sidebar-toggle",
                        if st.layout.sidebar_visible {
                            tr(loc, "Visible", "显示")
                        } else {
                            tr(loc, "Hidden", "隐藏")
                        },
                        st.layout.sidebar_visible,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.sidebar_visible = !g.layout.sidebar_visible;
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    ))
                    .child(self.render_stepper_control(
                        "sidebar-width",
                        format!("{} px", sidebar_w_px as u32),
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.sidebar_width_px =
                                    (g.layout.sidebar_width_px - 12.0).max(200.0);
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.sidebar_width_px =
                                    (g.layout.sidebar_width_px + 12.0).min(320.0);
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    )),
            ))
    }

    fn render_settings_runtime_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let local_spaces_count = st.spaces.iter().filter(|s| !s.is_remote).count();
        let hidden_count = st.dismissed_spaces.len();
        let hc_ver = st
            .hc_version
            .as_deref()
            .unwrap_or_else(|| tr(loc, "not detected", "未检测到"));
        let hc_path_label = st
            .hc_path
            .as_ref()
            .map(|p| display_compact_path(&p.to_string_lossy()))
            .unwrap_or_else(|| "—".to_string());

        div()
            .id("settings-runtime-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .p(px(11.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .w(px(7.0))
                                            .h(px(7.0))
                                            .rounded(px(99.0))
                                            .bg(rgb(if st.hc_available {
                                                palette::DONE_GREEN
                                            } else {
                                                palette::TEXT_DIM
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(format!("herdr-core (hc) · {hc_ver}")),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .truncate()
                                    .child(hc_path_label),
                            ),
                    )
                    .child(self.render_setting_pill(
                        "settings-recheck-hc",
                        tr(loc, "Re-check", "重新检测"),
                        false,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            let detected = discover_hc_binary();
                            if let Ok(mut g) = this.state.lock() {
                                g.hc_available = detected.is_some();
                                g.hc_path = detected;
                            }
                            cx.notify();
                        }),
                    )),
            )
            .child(render_settings_row(
                tr(loc, "Split Terminals", "双栏终端分屏"),
                tr(loc, "Show two panes side by side (Cmd+D)", "并排显示左右两个终端窗口 (Cmd+D)"),
                self.render_setting_pill(
                    "settings-split-toggle",
                    if st.layout.split_terminals {
                        tr(loc, "Enabled", "开启")
                    } else {
                        tr(loc, "Off", "关闭")
                    },
                    st.layout.split_terminals,
                    accent,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.toggle_split_terminals();
                        cx.notify();
                    }),
                ),
            ))
            .child(render_settings_row(
                tr(loc, "Local Workspaces", "本地工作区"),
                &format!(
                    "{}: {local_spaces_count}",
                    tr(loc, "Active local spaces", "当前本地工作区数量")
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .when(hidden_count > 0, |el| {
                        el.child(self.render_setting_pill(
                            "settings-restore-spaces",
                            &format!(
                                "{} ({hidden_count})",
                                tr(loc, "Restore Hidden", "恢复已隐藏工作区")
                            ),
                            false,
                            accent,
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.restore_dismissed_spaces();
                                cx.notify();
                            }),
                        ))
                    })
                    .child(self.render_setting_pill(
                        "settings-add-folder",
                        tr(loc, "+ Add Folder…", "+ 添加本地目录…"),
                        false,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.add_workspace_folder();
                            cx.notify();
                        }),
                    )),
            ))
            .child(render_settings_row(
                tr(loc, "Reset Appearance", "重置外观配置"),
                tr(
                    loc,
                    "Restore default layout.json parameters",
                    "恢复 layout.json 默认字体、布局与强调色",
                ),
                self.render_setting_pill(
                    "settings-reset-layout",
                    tr(loc, "Reset Layout Defaults", "恢复默认布局参数"),
                    false,
                    accent,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Ok(mut g) = this.state.lock() {
                            g.layout = MalleableLayoutConfig::default();
                        }
                        this.persist_layout_config();
                        cx.notify();
                    }),
                ),
            ))
            .child({
                let v_url_focus = st.input_target == ActiveInputTarget::VoiceBaseUrl;
                let v_model_focus = st.input_target == ActiveInputTarget::VoiceModel;
                let v_key_focus = st.input_target == ActiveInputTarget::VoiceApiKey;
                let masked_key = if st.voice_api_key.is_empty() {
                    tr(loc, "sk-... (stored in macOS Keychain)", "sk-...（存入 macOS 钥匙串）")
                        .to_string()
                } else {
                    "•".repeat(st.voice_api_key.chars().count().min(28))
                };
                div()
                    .p(px(11.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(tr(
                                                loc,
                                                "Whisper Voice Dictation (🎙)",
                                                "Whisper 语音听写 (🎙)",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(if st.voice_api_key.is_empty() && cfg!(target_os = "macos") {
                                                tr(
                                                    loc,
                                                    "Empty API Key uses zero-config macOS Dictation (or set HTTPS / 127.0.0.1 Whisper)",
                                                    "API Key 留空时自动回退使用 macOS 原生语音听写（或填写 Whisper Key / 127.0.0.1）",
                                                )
                                            } else {
                                                tr(
                                                    loc,
                                                    "OpenAI-compatible /audio/transcriptions (HTTPS or local 127.0.0.1)",
                                                    "兼容 OpenAI /audio/transcriptions（HTTPS 或本机 127.0.0.1）",
                                                )
                                            }),
                                    ),
                            )
                            .child(self.render_setting_pill(
                                "settings-save-voice",
                                tr(loc, "Save Voice", "保存语音配置"),
                                false,
                                accent,
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.save_voice_preferences();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("settings-voice-base-url")
                                    .occlude()
                                    .flex_1()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(5.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(if v_url_focus {
                                        accent
                                    } else {
                                        palette::BORDER
                                    }))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .truncate()
                                    .child(if st.voice_base_url.is_empty() {
                                        "https://api.openai.com/v1".to_string()
                                    } else {
                                        format!(
                                            "{}{}",
                                            st.voice_base_url,
                                            if v_url_focus { "▏" } else { "" }
                                        )
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.input_selected_all = false;
                                            g.input_target = ActiveInputTarget::VoiceBaseUrl;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("settings-voice-model")
                                    .occlude()
                                    .w(px(140.0))
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(5.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(if v_model_focus {
                                        accent
                                    } else {
                                        palette::BORDER
                                    }))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .truncate()
                                    .child(if st.voice_model.is_empty() {
                                        "whisper-1".to_string()
                                    } else {
                                        format!(
                                            "{}{}",
                                            st.voice_model,
                                            if v_model_focus { "▏" } else { "" }
                                        )
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.input_selected_all = false;
                                            g.input_target = ActiveInputTarget::VoiceModel;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-voice-api-key")
                            .occlude()
                            .w_full()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::CHROME_BG))
                            .border_1()
                            .border_color(rgb(if v_key_focus {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if st.voice_api_key.is_empty() {
                                palette::TEXT_DIM
                            } else {
                                palette::TEXT_PRIMARY
                            }))
                            .truncate()
                            .child(format!(
                                "{masked_key}{}",
                                if v_key_focus { "▏" } else { "" }
                            ))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_selected_all = false;
                                    g.input_target = ActiveInputTarget::VoiceApiKey;
                                }
                                cx.notify();
                            })),
                    )
            })
    }

    fn render_settings_machines_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let show_auth_form = !st.is_logged_in || st.auth_form_expanded;
        let host_mode_on = st.allow_remote_control;
        let srv_focused = st.input_target == ActiveInputTarget::AuthServerUrl;
        let acct_focused = st.input_target == ActiveInputTarget::AuthAccount;
        let pass_focused = st.input_target == ActiveInputTarget::AuthPassword;

        div()
            .id("settings-machines-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                // Control-plane Server URL + quick presets
                div()
                    .p(px(10.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(tr(loc, "Control-Plane Server", "控制面服务器")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(self.render_setting_pill(
                                        "settings-srv-local",
                                        "Local :8080",
                                        st.server_url == "http://127.0.0.1:8080",
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.save_server_url_preference(
                                                "http://127.0.0.1:8080",
                                            );
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.render_setting_pill(
                                        "settings-srv-cloud",
                                        "codahub.dev",
                                        st.server_url == "https://codahub.dev",
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.save_server_url_preference("https://codahub.dev");
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-input-auth-server")
                            .occlude()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::CANVAS_BG))
                            .border_1()
                            .border_color(rgb(if srv_focused {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child(if srv_focused {
                                format!("{}|", st.server_url)
                            } else {
                                st.server_url.clone()
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthServerUrl;
                                    g.input_selected_all = false;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                // Account card (signed in vs login form)
                div()
                    .p(px(10.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .when(!show_auth_form, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .rounded(px(99.0))
                                                .bg(rgb(palette::DONE_GREEN)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .truncate()
                                                .child(st.account_email.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(self.render_setting_pill(
                                            "settings-toggle-pw-change",
                                            tr(loc, "Password", "改密"),
                                            st.pw_change_expanded,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.pw_change_expanded = !g.pw_change_expanded;
                                                    g.auth_error = None;
                                                    if g.pw_change_expanded {
                                                        g.input_target = ActiveInputTarget::ChangePwCurrent;
                                                    } else {
                                                        g.pw_current_input.clear();
                                                        g.pw_new_input.clear();
                                                        g.input_target = ActiveInputTarget::TerminalDirect;
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.render_setting_pill(
                                            "settings-switch-acct",
                                            tr(loc, "Switch Account", "切换账号"),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.auth_form_expanded = true;
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.render_setting_pill(
                                            "settings-sign-out",
                                            tr(loc, "Sign Out", "退出登录"),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                this.mesh_hub.sign_out();
                                                cx.notify();
                                            }),
                                        )),
                                ),
                        )
                        .when(st.pw_change_expanded, |el2| {
                            let cur_focused = st.input_target == ActiveInputTarget::ChangePwCurrent;
                            let new_focused = st.input_target == ActiveInputTarget::ChangePwNew;
                            let cur_mask = "•".repeat(st.pw_current_input.chars().count());
                            let new_mask = "•".repeat(st.pw_new_input.chars().count());
                            el2.child(
                                div()
                                    .mt(px(4.0))
                                    .p(px(8.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .id("settings-pw-current")
                                                    .occlude()
                                                    .flex_1()
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if cur_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(if st.pw_current_input.is_empty() {
                                                        palette::TEXT_DIM
                                                    } else {
                                                        palette::TEXT_PRIMARY
                                                    }))
                                                    .child(if st.pw_current_input.is_empty() {
                                                        if cur_focused {
                                                            "|".to_string()
                                                        } else {
                                                            tr(loc, "Current password", "当前密码").to_string()
                                                        }
                                                    } else if cur_focused {
                                                        format!("{cur_mask}|")
                                                    } else {
                                                        cur_mask
                                                    })
                                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.input_target = ActiveInputTarget::ChangePwCurrent;
                                                            g.input_selected_all = false;
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("settings-pw-new")
                                                    .occlude()
                                                    .flex_1()
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if new_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(if st.pw_new_input.is_empty() {
                                                        palette::TEXT_DIM
                                                    } else {
                                                        palette::TEXT_PRIMARY
                                                    }))
                                                    .child(if st.pw_new_input.is_empty() {
                                                        if new_focused {
                                                            "|".to_string()
                                                        } else {
                                                            tr(loc, "New password (8+ chars)", "新密码（至少 8 位）").to_string()
                                                        }
                                                    } else if new_focused {
                                                        format!("{new_mask}|")
                                                    } else {
                                                        new_mask
                                                    })
                                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.input_target = ActiveInputTarget::ChangePwNew;
                                                            g.input_selected_all = false;
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(self.render_setting_pill(
                                                "settings-pw-submit",
                                                tr(loc, "Update", "确认更新"),
                                                true,
                                                accent,
                                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                                    this.submit_change_password();
                                                    cx.notify();
                                                }),
                                            )),
                                    )
                                    .when_some(st.auth_error.clone(), |err_el, err_msg| {
                                        err_el.child(
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(rgb(palette::ERROR_RED))
                                                .child(err_msg),
                                        )
                                    }),
                            )
                        })
                    })
                    .when(show_auth_form, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(tr(loc, "Coda Account Sign In", "Coda 账号登录")),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(self.render_setting_pill(
                                            "settings-btn-browser-signin",
                                            tr(
                                                loc,
                                                "Sign in via Browser",
                                                "浏览器一键授权登录",
                                            ),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                let srv = this
                                                    .state
                                                    .lock()
                                                    .map(|mut g| {
                                                        g.auth_loading = true;
                                                        g.auth_error = None;
                                                        g.server_url.clone()
                                                    })
                                                    .unwrap_or_default();
                                                spawn_device_login(
                                                    Arc::clone(&this.state),
                                                    Arc::clone(&this.ui_dirty),
                                                    Arc::clone(&this.mesh_hub),
                                                    srv,
                                                );
                                                cx.notify();
                                            }),
                                        ))
                                        .when(st.is_logged_in, |c| {
                                            c.child(self.render_setting_pill(
                                                "settings-collapse-auth",
                                                tr(loc, "Cancel", "取消"),
                                                false,
                                                accent,
                                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.auth_form_expanded = false;
                                                        g.input_target =
                                                            ActiveInputTarget::TerminalDirect;
                                                    }
                                                    cx.notify();
                                                }),
                                            ))
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id("settings-input-auth-email")
                                        .occlude()
                                        .flex_1()
                                        .px(px(8.0))
                                        .py(px(5.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::CANVAS_BG))
                                        .border_1()
                                        .border_color(rgb(if acct_focused {
                                            accent
                                        } else {
                                            palette::BORDER
                                        }))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(
                                            if st.auth_account_input.is_empty() && !acct_focused {
                                                palette::TEXT_DIM
                                            } else {
                                                palette::TEXT_PRIMARY
                                            },
                                        ))
                                        .child(if st.auth_account_input.is_empty() {
                                            if acct_focused {
                                                "|".to_string()
                                            } else {
                                                tr(loc, "Email / Username", "账号邮箱").to_string()
                                            }
                                        } else if acct_focused {
                                            format!("{}|", st.auth_account_input)
                                        } else {
                                            st.auth_account_input.clone()
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.input_target = ActiveInputTarget::AuthAccount;
                                                g.input_selected_all = false;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("settings-input-auth-pass")
                                        .occlude()
                                        .flex_1()
                                        .px(px(8.0))
                                        .py(px(5.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::CANVAS_BG))
                                        .border_1()
                                        .border_color(rgb(if pass_focused {
                                            accent
                                        } else {
                                            palette::BORDER
                                        }))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(
                                            if st.auth_password_input.is_empty() && !pass_focused {
                                                palette::TEXT_DIM
                                            } else {
                                                palette::TEXT_PRIMARY
                                            },
                                        ))
                                        .child(if st.auth_password_input.is_empty() {
                                            if pass_focused {
                                                "|".to_string()
                                            } else {
                                                tr(loc, "Password", "密码").to_string()
                                            }
                                        } else {
                                            let dots =
                                                "*".repeat(st.auth_password_input.chars().count());
                                            if pass_focused {
                                                format!("{dots}|")
                                            } else {
                                                dots
                                            }
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.input_target = ActiveInputTarget::AuthPassword;
                                                g.input_selected_all = false;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("settings-btn-password-signin")
                                        .occlude()
                                        .flex_shrink_0()
                                        .px(px(12.0))
                                        .py(px(5.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(accent))
                                        .hover(|h| h.opacity(0.9))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette::CANVAS_BG))
                                        .child(if st.auth_loading {
                                            tr(loc, "Signing in…", "登录中…")
                                        } else {
                                            tr(loc, "Sign In", "登录")
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.submit_password_login();
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when_some(st.auth_error.clone(), |el, err| {
                            el.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(palette::ERROR_RED))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::ERROR_RED))
                                    .child(err),
                            )
                        })
                        .when_some(st.device_login_code.clone(), |el, code| {
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::WORKING_SKY))
                                    .child(format!("Code: {code}")),
                            )
                        })
                    }),
            )
            .child({
                let wh = &st.webhook_settings;
                let bark_focus = st.input_target == ActiveInputTarget::WebhookBarkUrl;
                let tg_tok_focus = st.input_target == ActiveInputTarget::WebhookTelegramToken;
                let tg_chat_focus = st.input_target == ActiveInputTarget::WebhookTelegramChatId;
                let feishu_focus = st.input_target == ActiveInputTarget::WebhookFeishuUrl;
                let discord_focus = st.input_target == ActiveInputTarget::WebhookDiscordUrl;
                let tg_tok_masked = if wh.telegram_bot_token.is_empty() {
                    if tg_tok_focus {
                        "▏".to_string()
                    } else {
                        tr(loc, "Telegram Bot Token (123456:ABC...)", "Telegram Bot Token")
                            .to_string()
                    }
                } else {
                    format!(
                        "{}{}",
                        "•".repeat(wh.telegram_bot_token.chars().count().min(24)),
                        if tg_tok_focus { "▏" } else { "" }
                    )
                };
                let needs_you_on = wh.notify_on_needs_you;
                let finished_on = wh.notify_on_finished;

                div()
                    .p(px(10.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(tr(
                                                loc,
                                                "Offline Notification Webhooks",
                                                "离线告警推送 Webhooks",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .truncate()
                                            .child(tr(
                                                loc,
                                                "Zero-plaintext metadata push via Bark, Telegram, Feishu & Discord",
                                                "零终端明文泄露，支持 Bark、Telegram、飞书与 Discord 推送",
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(self.render_setting_pill(
                                        "settings-webhook-needs-you",
                                        if needs_you_on {
                                            tr(loc, "Needs-You ✓", "待确认 ✓")
                                        } else {
                                            tr(loc, "Needs-You", "待确认")
                                        },
                                        needs_you_on,
                                        accent,
                                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.webhook_settings.notify_on_needs_you = !needs_you_on;
                                            }
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.render_setting_pill(
                                        "settings-webhook-finished",
                                        if finished_on {
                                            tr(loc, "Finished ✓", "完成 ✓")
                                        } else {
                                            tr(loc, "Finished", "完成")
                                        },
                                        finished_on,
                                        accent,
                                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.webhook_settings.notify_on_finished = !finished_on;
                                            }
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.render_setting_pill(
                                        "settings-webhook-test",
                                        tr(loc, "Test Push", "测试推送"),
                                        false,
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.test_notification_webhooks();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.render_setting_pill(
                                        "settings-webhook-save",
                                        if st.webhook_loading {
                                            "…"
                                        } else {
                                            tr(loc, "Save Webhooks", "保存推送")
                                        },
                                        true,
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.save_notification_webhooks();
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-input-webhook-bark")
                            .occlude()
                            .w_full()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(palette::CANVAS_BG))
                            .border_1()
                            .border_color(rgb(if bark_focus { accent } else { palette::BORDER }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if wh.bark_url.is_empty() && !bark_focus {
                                palette::TEXT_DIM
                            } else {
                                palette::TEXT_PRIMARY
                            }))
                            .truncate()
                            .child(if wh.bark_url.is_empty() {
                                if bark_focus {
                                    "▏".to_string()
                                } else {
                                    tr(
                                        loc,
                                        "Bark URL (https://api.day.app/<device_key>)",
                                        "Bark 推送地址 (https://api.day.app/<key>)",
                                    )
                                    .to_string()
                                }
                            } else {
                                format!("{}{}", wh.bark_url, if bark_focus { "▏" } else { "" })
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_selected_all = false;
                                    g.input_target = ActiveInputTarget::WebhookBarkUrl;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("settings-input-webhook-tg-token")
                                    .occlude()
                                    .flex_1()
                                    .min_w_0()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(if tg_tok_focus {
                                        accent
                                    } else {
                                        palette::BORDER
                                    }))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(if wh.telegram_bot_token.is_empty() && !tg_tok_focus {
                                        palette::TEXT_DIM
                                    } else {
                                        palette::TEXT_PRIMARY
                                    }))
                                    .truncate()
                                    .child(tg_tok_masked)
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.input_selected_all = false;
                                            g.input_target = ActiveInputTarget::WebhookTelegramToken;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("settings-input-webhook-tg-chat")
                                    .occlude()
                                    .w(px(160.0))
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(if tg_chat_focus {
                                        accent
                                    } else {
                                        palette::BORDER
                                    }))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(if wh.telegram_chat_id.is_empty() && !tg_chat_focus {
                                        palette::TEXT_DIM
                                    } else {
                                        palette::TEXT_PRIMARY
                                    }))
                                    .truncate()
                                    .child(if wh.telegram_chat_id.is_empty() {
                                        if tg_chat_focus {
                                            "▏".to_string()
                                        } else {
                                            tr(loc, "Telegram Chat ID", "Telegram Chat ID").to_string()
                                        }
                                    } else {
                                        format!(
                                            "{}{}",
                                            wh.telegram_chat_id,
                                            if tg_chat_focus { "▏" } else { "" }
                                        )
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.input_selected_all = false;
                                            g.input_target = ActiveInputTarget::WebhookTelegramChatId;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("settings-input-webhook-feishu")
                                    .occlude()
                                    .flex_1()
                                    .min_w_0()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(if feishu_focus {
                                        accent
                                    } else {
                                        palette::BORDER
                                    }))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(if wh.feishu_webhook_url.is_empty() && !feishu_focus {
                                        palette::TEXT_DIM
                                    } else {
                                        palette::TEXT_PRIMARY
                                    }))
                                    .truncate()
                                    .child(if wh.feishu_webhook_url.is_empty() {
                                        if feishu_focus {
                                            "▏".to_string()
                                        } else {
                                            tr(
                                                loc,
                                                "Feishu Webhook (https://open.feishu.cn/...)",
                                                "飞书机器人 Webhook 地址",
                                            )
                                            .to_string()
                                        }
                                    } else {
                                        format!(
                                            "{}{}",
                                            wh.feishu_webhook_url,
                                            if feishu_focus { "▏" } else { "" }
                                        )
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.input_selected_all = false;
                                            g.input_target = ActiveInputTarget::WebhookFeishuUrl;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("settings-input-webhook-discord")
                                    .occlude()
                                    .flex_1()
                                    .min_w_0()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(if discord_focus {
                                        accent
                                    } else {
                                        palette::BORDER
                                    }))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(if wh.discord_webhook_url.is_empty() && !discord_focus {
                                        palette::TEXT_DIM
                                    } else {
                                        palette::TEXT_PRIMARY
                                    }))
                                    .truncate()
                                    .child(if wh.discord_webhook_url.is_empty() {
                                        if discord_focus {
                                            "▏".to_string()
                                        } else {
                                            tr(
                                                loc,
                                                "Discord Webhook (https://discord.com/api/webhooks/...)",
                                                "Discord Webhook 地址",
                                            )
                                            .to_string()
                                        }
                                    } else {
                                        format!(
                                            "{}{}",
                                            wh.discord_webhook_url,
                                            if discord_focus { "▏" } else { "" }
                                        )
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.input_selected_all = false;
                                            g.input_target = ActiveInputTarget::WebhookDiscordUrl;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
            })
            .child(
                // Machines list (local Mac + Host Mode toggle + Prune Offline Web Devices + ALL remote hosts)
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(format!(
                                        "{} ({})",
                                        tr(loc, "ONLINE MACHINES", "在线设备列表"),
                                        st.remote_hosts.len() + 1
                                    )),
                            )
                            .child(self.render_setting_pill(
                                "settings-prune-web-devices",
                                tr(loc, "Prune Offline Web Devices", "清理离线 Web 设备"),
                                false,
                                accent,
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.prune_offline_web_devices();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .flex_shrink_0()
                                            .rounded(px(99.0))
                                            .bg(rgb(if st.embedded_host_active || st.external_daemon_running {
                                                palette::DONE_GREEN
                                            } else {
                                                palette::TEXT_DIM
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .truncate()
                                            .child(format!(
                                                "{} · {}",
                                                tr(loc, "This Mac", "本机"),
                                                st.local_machine_name
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(if st.external_daemon_running {
                                                "daemon"
                                            } else if host_mode_on {
                                                "hybrid"
                                            } else {
                                                "client"
                                            }),
                                    )
                                    .child(self.render_setting_pill(
                                        "settings-toggle-remote-host",
                                        if host_mode_on {
                                            tr(loc, "Allow Control: ON", "允许被控: 开")
                                        } else {
                                            tr(loc, "Allow Control: OFF", "允许被控: 关")
                                        },
                                        host_mode_on,
                                        accent,
                                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.mesh_hub.set_allow_remote_control(!host_mode_on);
                                            cx.notify();
                                        }),
                                    ))
                                    .child({
                                        let notify_on = st.desktop_notifications;
                                        self.render_setting_pill(
                                            "settings-toggle-desktop-notify",
                                            if notify_on {
                                                tr(loc, "Alerts: ON", "通知: 开")
                                            } else {
                                                tr(loc, "Alerts: OFF", "通知: 关")
                                            },
                                            notify_on,
                                            accent,
                                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                let next = !notify_on;
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.desktop_notifications = next;
                                                    g.status_notice = Some(
                                                        if next {
                                                            tr(
                                                                g.locale,
                                                                "Native macOS OSC 7501 alerts enabled",
                                                                "已开启 macOS 原生 OSC 7501 系统通知",
                                                            )
                                                        } else {
                                                            tr(
                                                                g.locale,
                                                                "Native macOS OSC 7501 alerts muted",
                                                                "已关闭 macOS 原生系统通知",
                                                            )
                                                        }
                                                        .to_string(),
                                                    );
                                                }
                                                CodaGpuWorkspaceView::update_prefs_async(move |p| {
                                                    p.desktop_notifications = next;
                                                });
                                                cx.notify();
                                            }),
                                        )
                                    })
                                    .child(self.render_setting_pill(
                                        "settings-probe-lan-now",
                                        tr(loc, "Probe LAN", "探测直连"),
                                        false,
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.mesh_hub.probe_all_relay_peers_now();
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    )
                    .children(st.remote_hosts.iter().map(|host| {
                        let host_clone = host.clone();
                        let upgrade_host = host.clone();
                        let can_ota = host.is_online
                            && host
                                .health
                                .as_ref()
                                .map(|h| h.ota_supported)
                                .unwrap_or(true);
                        let meta_line = match host.health.as_ref() {
                            Some(hh) => format!(
                                "{} · {} · {}",
                                host.platform,
                                host.role,
                                format_host_health_compact(hh)
                            ),
                            None => format!("{} · {}", host.platform, host.role),
                        };
                        let rename_id = host.id;
                        let rename_cur_name = host.name.clone();
                        let remove_id = host.id;
                        let is_renaming = st.renaming_host_id == Some(host.id);
                        let rename_focused = st.input_target == ActiveInputTarget::RenameDeviceInput;
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "settings-mesh-host-{}",
                                        host.id
                                    )))
                                    .occlude()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .w(px(6.0))
                                                    .h(px(6.0))
                                                    .flex_shrink_0()
                                                    .rounded(px(99.0))
                                                    .bg(rgb(if host.is_online {
                                                        palette::DONE_GREEN
                                                    } else {
                                                        palette::TEXT_DIM
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.5))
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .truncate()
                                                    .child(host.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(palette::TEXT_DIM))
                                                    .truncate()
                                                    .child(meta_line),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(accent))
                                                    .child(tr(loc, "Open Workspace", "进入工作区")),
                                            )
                                            .when(can_ota, |el| {
                                                el.child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "settings-ota-host-{}",
                                                            host.id
                                                        )))
                                                        .occlude()
                                                        .px(px(6.0))
                                                        .py(px(2.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(palette::CHROME_BG))
                                                        .border_1()
                                                        .border_color(rgb(palette::BORDER))
                                                        .hover(|h| {
                                                            h.border_color(rgb(accent))
                                                                .text_color(rgb(accent))
                                                        })
                                                        .cursor_pointer()
                                                        .text_size(px(10.0))
                                                        .text_color(rgb(palette::TEXT_MUTED))
                                                        .child(tr(loc, "Upgrade", "无感升级"))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            |_, _, cx| {
                                                                cx.stop_propagation();
                                                            },
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _: &ClickEvent, _, cx| {
                                                                cx.stop_propagation();
                                                                this.mesh_hub.upgrade_remote_daemon(
                                                                    upgrade_host.clone(),
                                                                );
                                                                cx.notify();
                                                            },
                                                        )),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "settings-rename-host-{}",
                                                        host.id
                                                    )))
                                                    .occlude()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CHROME_BG))
                                                    .border_1()
                                                    .border_color(rgb(if is_renaming {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .hover(|h| {
                                                        h.border_color(rgb(accent))
                                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                                    })
                                                    .cursor_pointer()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(palette::TEXT_MUTED))
                                                    .child(tr(loc, "Rename", "改名"))
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                        cx.stop_propagation();
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            if let Ok(mut g) = this.state.lock() {
                                                                if g.renaming_host_id == Some(rename_id) {
                                                                    g.renaming_host_id = None;
                                                                    g.rename_host_input.clear();
                                                                    g.input_target = ActiveInputTarget::TerminalDirect;
                                                                } else {
                                                                    g.renaming_host_id = Some(rename_id);
                                                                    g.rename_host_input = rename_cur_name.clone();
                                                                    g.input_selected_all = true;
                                                                    g.input_target = ActiveInputTarget::RenameDeviceInput;
                                                                }
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "settings-remove-host-{}",
                                                        host.id
                                                    )))
                                                    .occlude()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CHROME_BG))
                                                    .border_1()
                                                    .border_color(rgb(palette::BORDER))
                                                    .hover(|h| {
                                                        h.border_color(rgb(palette::ERROR_RED))
                                                            .text_color(rgb(palette::ERROR_RED))
                                                    })
                                                    .cursor_pointer()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(palette::TEXT_MUTED))
                                                    .child(tr(loc, "Remove", "移除"))
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                        cx.stop_propagation();
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.mesh_hub.delete_remote_device(remove_id);
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.mesh_hub.probe_host(host_clone.clone());
                                        if let Ok(mut g) = this.state.lock() {
                                            g.settings_open = false;
                                            g.clear_settings_ephemeral_inputs();
                                            g.input_target = ActiveInputTarget::TerminalDirect;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(is_renaming, |row_el| {
                                row_el.child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(5.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(palette::CHROME_BG))
                                        .border_1()
                                        .border_color(rgb(palette::BORDER))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .id("settings-rename-input")
                                                .occlude()
                                                .flex_1()
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(palette::CANVAS_BG))
                                                .border_1()
                                                .border_color(rgb(if rename_focused {
                                                    accent
                                                } else {
                                                    palette::BORDER
                                                }))
                                                .cursor_pointer()
                                                .text_size(px(11.0))
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .child(if rename_focused {
                                                    format!("{}|", st.rename_host_input)
                                                } else {
                                                    st.rename_host_input.clone()
                                                })
                                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.input_target = ActiveInputTarget::RenameDeviceInput;
                                                        g.input_selected_all = false;
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .child(self.render_setting_pill(
                                            "settings-rename-save",
                                            tr(loc, "Save", "保存"),
                                            true,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                this.submit_rename_device();
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.render_setting_pill(
                                            "settings-rename-cancel",
                                            tr(loc, "Cancel", "取消"),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.renaming_host_id = None;
                                                    g.rename_host_input.clear();
                                                    g.input_target = ActiveInputTarget::TerminalDirect;
                                                }
                                                cx.notify();
                                            }),
                                        )),
                                )
                            })
                    })),
            )
    }

    fn render_settings_shortcuts_pane(&self, st: &CodaDesktopState) -> impl IntoElement {
        let loc = st.locale;
        let left_col: &[(&str, &str, &str)] = &[
            ("⌘ ,", "Open Settings", "打开设置"),
            ("⌘ P / ⌘ K", "Command Palette", "命令面板"),
            ("⌘ J", "Jump to Next Needs-You", "跳转待确认会话"),
            ("⌘ F", "Find in Terminal", "终端内搜索"),
            ("⌘ N / ⌘ T", "New Terminal", "新建终端"),
            ("⌘ W", "Close Active Terminal", "关闭当前终端"),
            ("⌘ D", "Toggle Split Panes", "分屏终端"),
        ];
        let right_col: &[(&str, &str, &str)] = &[
            ("⌘ B", "Toggle Left Sidebar", "显示/隐藏侧边栏"),
            ("⌘ Click", "Open Terminal URL", "点击打开链接"),
            ("⌘ 1", "Main Workbench", "主工作台"),
            ("⌘ 2", "Terminals Grid", "终端平铺"),
            ("⌃ C", "Interrupt Terminal (SIGINT)", "中断终端进程"),
            ("⌘ V", "Paste into Terminal / Input", "粘贴"),
        ];

        div()
            .flex()
            .flex_row()
            .gap(px(10.0))
            .children([left_col, right_col].into_iter().map(|col| {
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(col.iter().map(|&(keys, en, zh)| {
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(6.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(tr(loc, en, zh)),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER_STRONG))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(keys),
                            )
                    }))
            }))
    }

    fn render_setting_pill(
        &self,
        id: impl Into<SharedString>,
        label: &str,
        active: bool,
        accent: u32,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id.into())
            .occlude()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .bg(rgb(if active {
                palette::HOVER_BG
            } else {
                palette::CHROME_BG
            }))
            .border_1()
            .border_color(rgb(if active { accent } else { palette::BORDER }))
            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(rgb(if active {
                palette::TEXT_PRIMARY
            } else {
                palette::TEXT_MUTED
            }))
            .child(label.to_string())
            .on_click(on_click)
    }

    fn render_stepper_control(
        &self,
        slug: &'static str,
        value_label: String,
        on_dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .id(SharedString::from(format!("settings-{slug}-dec")))
                    .occlude()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded(px(5.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(rgb(palette::TEXT_PRIMARY))
                    .child("−")
                    .on_click(on_dec),
            )
            .child(
                div()
                    .min_w(px(52.0))
                    .flex()
                    .justify_center()
                    .text_size(px(11.5))
                    .text_color(rgb(palette::TEXT_PRIMARY))
                    .child(value_label),
            )
            .child(
                div()
                    .id(SharedString::from(format!("settings-{slug}-inc")))
                    .occlude()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded(px(5.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(rgb(palette::TEXT_PRIMARY))
                    .child("+")
                    .on_click(on_inc),
            )
    }
}

fn render_settings_row(label: &str, hint: &str, control: impl IntoElement) -> impl IntoElement {
    div()
        .px(px(11.0))
        .py(px(8.0))
        .rounded(px(7.0))
        .bg(rgb(palette::SURFACE_BG))
        .border_1()
        .border_color(rgb(palette::BORDER))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(palette::TEXT_PRIMARY))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(palette::TEXT_DIM))
                        .truncate()
                        .child(hint.to_string()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
}
