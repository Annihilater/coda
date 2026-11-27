//! Command Palette (`Cmd+P` / `Cmd+K`) and Settings Modal (`Cmd+,`) overlays for `coda-desktop`.

use std::sync::Arc;

use coda_herdr::discover_hc_binary;
use gpui::{
    div, prelude::*, px, rgb, rgba, App, ClickEvent, ClipboardItem, Context, FontWeight,
    MouseButton, SharedString, Window,
};

use crate::config::{
    config_root_dir, display_compact_path, reveal_in_finder, tr, LayoutMode, MalleableLayoutConfig,
    UiLocale, MAX_PROMPT_SNIPPETS,
};
use crate::helpers::build_invite_registration_url;
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
                                                let mut should_sync = false;
                                                if let Ok(mut g) = this.state.lock() {
                                                    if g.settings_section != sec {
                                                        g.clear_settings_ephemeral_inputs();
                                                        if matches!(
                                                            sec,
                                                            SettingsSection::Machines
                                                                | SettingsSection::Runtime
                                                        ) && g.is_logged_in
                                                        {
                                                            should_sync = true;
                                                        }
                                                    }
                                                    g.settings_section = sec;
                                                    g.input_target =
                                                        ActiveInputTarget::TerminalDirect;
                                                }
                                                if should_sync {
                                                    this.reload_account_settings_data();
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
            .child(self.render_settings_app_update_card(st, cx))
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
            .child(self.render_settings_snippets_card(st, cx))
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
                        let reset_mode = st.auth_reset_mode;
                        let register_mode = st.auth_register_mode && !reset_mode;
                        let signin_mode = !reset_mode && !register_mode;
                        let tok_focused = st.input_target == ActiveInputTarget::AuthResetToken;
                        let rpw_focused =
                            st.input_target == ActiveInputTarget::AuthResetNewPassword;
                        let reg_inv_focused =
                            st.input_target == ActiveInputTarget::AuthRegisterInviteCode;
                        let reg_email_focused =
                            st.input_target == ActiveInputTarget::AuthRegisterEmail;
                        let reg_handle_focused =
                            st.input_target == ActiveInputTarget::AuthRegisterHandle;
                        let reg_disp_focused =
                            st.input_target == ActiveInputTarget::AuthRegisterDisplayName;
                        let reg_pw_focused =
                            st.input_target == ActiveInputTarget::AuthRegisterPassword;
                        el.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(if reset_mode {
                                            tr(
                                                loc,
                                                "Reset Password with Token",
                                                "使用一次性口令重置密码",
                                            )
                                        } else if register_mode {
                                            tr(
                                                loc,
                                                "Register with Invite Code",
                                                "使用邀请码注册账号",
                                            )
                                        } else {
                                            tr(loc, "Coda Account Sign In", "Coda 账号登录")
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(5.0))
                                        .child(self.render_setting_pill(
                                            "settings-btn-tab-signin",
                                            tr(loc, "Sign In", "账号登录"),
                                            signin_mode,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.auth_reset_mode = false;
                                                    g.auth_register_mode = false;
                                                    g.auth_error = None;
                                                    g.input_selected_all = false;
                                                    g.input_target = ActiveInputTarget::AuthAccount;
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.render_setting_pill(
                                            "settings-btn-toggle-register-invite",
                                            tr(
                                                loc,
                                                "Register (with Invite Code)",
                                                "邀请码注册",
                                            ),
                                            register_mode,
                                            accent,
                                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.auth_register_mode = !register_mode;
                                                    g.auth_reset_mode = false;
                                                    g.auth_error = None;
                                                    g.input_selected_all = false;
                                                    g.input_target = if g.auth_register_mode {
                                                        ActiveInputTarget::AuthRegisterInviteCode
                                                    } else {
                                                        ActiveInputTarget::AuthAccount
                                                    };
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.render_setting_pill(
                                            "settings-btn-toggle-reset-token",
                                            tr(
                                                loc,
                                                "Reset Password",
                                                "口令改密",
                                            ),
                                            reset_mode,
                                            accent,
                                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.auth_reset_mode = !reset_mode;
                                                    g.auth_register_mode = false;
                                                    g.auth_error = None;
                                                    g.input_selected_all = false;
                                                    if g.auth_reset_mode {
                                                        g.input_target =
                                                            ActiveInputTarget::AuthResetToken;
                                                    } else {
                                                        g.auth_reset_token_input.clear();
                                                        g.auth_reset_new_pw_input.clear();
                                                        g.input_target =
                                                            ActiveInputTarget::AuthAccount;
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .when(signin_mode, |c| {
                                            c.child(self.render_setting_pill(
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
                                        })
                                        .when(st.is_logged_in, |c| {
                                            c.child(self.render_setting_pill(
                                                "settings-collapse-auth",
                                                tr(loc, "Cancel", "取消"),
                                                false,
                                                accent,
                                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.auth_form_expanded = false;
                                                        g.auth_reset_mode = false;
                                                        g.auth_register_mode = false;
                                                        g.auth_reset_token_input.clear();
                                                        g.auth_reset_new_pw_input.clear();
                                                        g.auth_register_password_input.clear();
                                                        g.input_target =
                                                            ActiveInputTarget::TerminalDirect;
                                                    }
                                                    cx.notify();
                                                }),
                                            ))
                                        }),
                                ),
                        )
                        .when(signin_mode, |el2| {
                            el2.child(
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
                                                if st.auth_account_input.is_empty() && !acct_focused
                                                {
                                                    palette::TEXT_DIM
                                                } else {
                                                    palette::TEXT_PRIMARY
                                                },
                                            ))
                                            .child(if st.auth_account_input.is_empty() {
                                                if acct_focused {
                                                    "|".to_string()
                                                } else {
                                                    tr(loc, "Email / Username", "账号邮箱")
                                                        .to_string()
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
                                                if st.auth_password_input.is_empty()
                                                    && !pass_focused
                                                {
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
                                                let dots = "*".repeat(
                                                    st.auth_password_input.chars().count(),
                                                );
                                                if pass_focused {
                                                    format!("{dots}|")
                                                } else {
                                                    dots
                                                }
                                            })
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.input_target =
                                                        ActiveInputTarget::AuthPassword;
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
                        })
                        .when(register_mode, |el2| {
                            let reg_pw_dots = "•"
                                .repeat(st.auth_register_password_input.chars().count().min(32));
                            el2.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .id("settings-input-register-invite")
                                            .occlude()
                                            .w_full()
                                            .px(px(8.0))
                                            .py(px(5.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(palette::CANVAS_BG))
                                            .border_1()
                                            .border_color(rgb(if reg_inv_focused {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .cursor_pointer()
                                            .font_family("Menlo")
                                            .text_size(px(10.5))
                                            .text_color(rgb(
                                                if st.auth_register_invite_input.is_empty()
                                                    && !reg_inv_focused
                                                {
                                                    palette::TEXT_DIM
                                                } else {
                                                    palette::TEXT_PRIMARY
                                                },
                                            ))
                                            .truncate()
                                            .child(if st.auth_register_invite_input.is_empty() {
                                                if reg_inv_focused {
                                                    "▏".to_string()
                                                } else {
                                                    tr(
                                                        loc,
                                                        "Invite code (CODA-XXXXXX) or paste full https://.../register?invite=... link",
                                                        "输入邀请码 (CODA-XXXXXX) 或直接粘贴完整邀请链接",
                                                    )
                                                    .to_string()
                                                }
                                            } else if reg_inv_focused {
                                                format!("{}▏", st.auth_register_invite_input)
                                            } else {
                                                st.auth_register_invite_input.clone()
                                            })
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.input_target =
                                                        ActiveInputTarget::AuthRegisterInviteCode;
                                                    g.input_selected_all = false;
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
                                                    .id("settings-input-register-email")
                                                    .occlude()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if reg_email_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(
                                                        if st.auth_register_email_input.is_empty()
                                                            && !reg_email_focused
                                                        {
                                                            palette::TEXT_DIM
                                                        } else {
                                                            palette::TEXT_PRIMARY
                                                        },
                                                    ))
                                                    .truncate()
                                                    .child(
                                                        if st.auth_register_email_input.is_empty() {
                                                            if reg_email_focused {
                                                                "▏".to_string()
                                                            } else {
                                                                tr(
                                                                    loc,
                                                                    "Email (you@example.com)",
                                                                    "邮箱 (you@example.com)",
                                                                )
                                                                .to_string()
                                                            }
                                                        } else if reg_email_focused {
                                                            format!(
                                                                "{}▏",
                                                                st.auth_register_email_input
                                                            )
                                                        } else {
                                                            st.auth_register_email_input.clone()
                                                        },
                                                    )
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            if let Ok(mut g) = this.state.lock() {
                                                                g.input_target = ActiveInputTarget::AuthRegisterEmail;
                                                                g.input_selected_all = false;
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id("settings-input-register-handle")
                                                    .occlude()
                                                    .w(px(150.0))
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if reg_handle_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(
                                                        if st.auth_register_handle_input.is_empty()
                                                            && !reg_handle_focused
                                                        {
                                                            palette::TEXT_DIM
                                                        } else {
                                                            palette::TEXT_PRIMARY
                                                        },
                                                    ))
                                                    .truncate()
                                                    .child(
                                                        if st.auth_register_handle_input.is_empty()
                                                        {
                                                            if reg_handle_focused {
                                                                "▏".to_string()
                                                            } else {
                                                                tr(
                                                                    loc,
                                                                    "Handle (e.g. alice)",
                                                                    "用户名 handle",
                                                                )
                                                                .to_string()
                                                            }
                                                        } else if reg_handle_focused {
                                                            format!(
                                                                "{}▏",
                                                                st.auth_register_handle_input
                                                            )
                                                        } else {
                                                            st.auth_register_handle_input.clone()
                                                        },
                                                    )
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            if let Ok(mut g) = this.state.lock() {
                                                                g.input_target = ActiveInputTarget::AuthRegisterHandle;
                                                                g.input_selected_all = false;
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .id("settings-input-register-display")
                                                    .occlude()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if reg_disp_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(
                                                        if st
                                                            .auth_register_display_name_input
                                                            .is_empty()
                                                            && !reg_disp_focused
                                                        {
                                                            palette::TEXT_DIM
                                                        } else {
                                                            palette::TEXT_PRIMARY
                                                        },
                                                    ))
                                                    .truncate()
                                                    .child(
                                                        if st
                                                            .auth_register_display_name_input
                                                            .is_empty()
                                                        {
                                                            if reg_disp_focused {
                                                                "▏".to_string()
                                                            } else {
                                                                tr(
                                                                    loc,
                                                                    "Display name (optional)",
                                                                    "显示昵称（可选）",
                                                                )
                                                                .to_string()
                                                            }
                                                        } else if reg_disp_focused {
                                                            format!(
                                                                "{}▏",
                                                                st.auth_register_display_name_input
                                                            )
                                                        } else {
                                                            st.auth_register_display_name_input
                                                                .clone()
                                                        },
                                                    )
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            if let Ok(mut g) = this.state.lock() {
                                                                g.input_target = ActiveInputTarget::AuthRegisterDisplayName;
                                                                g.input_selected_all = false;
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id("settings-input-register-pw")
                                                    .occlude()
                                                    .w(px(176.0))
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if reg_pw_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(
                                                        if st.auth_register_password_input.is_empty()
                                                            && !reg_pw_focused
                                                        {
                                                            palette::TEXT_DIM
                                                        } else {
                                                            palette::TEXT_PRIMARY
                                                        },
                                                    ))
                                                    .truncate()
                                                    .child(
                                                        if st
                                                            .auth_register_password_input
                                                            .is_empty()
                                                        {
                                                            if reg_pw_focused {
                                                                "▏".to_string()
                                                            } else {
                                                                tr(
                                                                    loc,
                                                                    "Password (6+ chars)",
                                                                    "密码（至少 6 位）",
                                                                )
                                                                .to_string()
                                                            }
                                                        } else if reg_pw_focused {
                                                            format!("{reg_pw_dots}▏")
                                                        } else {
                                                            reg_pw_dots
                                                        },
                                                    )
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            if let Ok(mut g) = this.state.lock() {
                                                                g.input_target = ActiveInputTarget::AuthRegisterPassword;
                                                                g.input_selected_all = false;
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id("settings-btn-submit-register")
                                                    .occlude()
                                                    .flex_shrink_0()
                                                    .px(px(11.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(accent))
                                                    .hover(|h| h.opacity(0.9))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(palette::CANVAS_BG))
                                                    .child(if st.auth_loading {
                                                        tr(loc, "Registering…", "注册中…")
                                                    } else {
                                                        tr(loc, "Create Account", "注册并登录")
                                                    })
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            this.submit_invite_registration();
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    ),
                            )
                        })
                        .when(reset_mode, |el2| {
                            let pw_dots =
                                "•".repeat(st.auth_reset_new_pw_input.chars().count().min(32));
                            el2.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .id("settings-input-reset-token")
                                            .occlude()
                                            .flex_1()
                                            .min_w_0()
                                            .px(px(8.0))
                                            .py(px(5.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(palette::CANVAS_BG))
                                            .border_1()
                                            .border_color(rgb(if tok_focused {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .cursor_pointer()
                                            .font_family("Menlo")
                                            .text_size(px(10.5))
                                            .text_color(rgb(
                                                if st.auth_reset_token_input.is_empty()
                                                    && !tok_focused
                                                {
                                                    palette::TEXT_DIM
                                                } else {
                                                    palette::TEXT_PRIMARY
                                                },
                                            ))
                                            .truncate()
                                            .child(if st.auth_reset_token_input.is_empty() {
                                                if tok_focused {
                                                    "▏".to_string()
                                                } else {
                                                    tr(
                                                        loc,
                                                        "Paste reset link or rst_... token",
                                                        "粘贴重置链接或 rst_... 口令",
                                                    )
                                                    .to_string()
                                                }
                                            } else if tok_focused {
                                                format!("{}▏", st.auth_reset_token_input)
                                            } else {
                                                st.auth_reset_token_input.clone()
                                            })
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.input_target =
                                                        ActiveInputTarget::AuthResetToken;
                                                    g.input_selected_all = false;
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("settings-input-reset-new-pw")
                                            .occlude()
                                            .w(px(176.0))
                                            .px(px(8.0))
                                            .py(px(5.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(palette::CANVAS_BG))
                                            .border_1()
                                            .border_color(rgb(if rpw_focused {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .cursor_pointer()
                                            .text_size(px(11.0))
                                            .text_color(rgb(
                                                if st.auth_reset_new_pw_input.is_empty()
                                                    && !rpw_focused
                                                {
                                                    palette::TEXT_DIM
                                                } else {
                                                    palette::TEXT_PRIMARY
                                                },
                                            ))
                                            .truncate()
                                            .child(if st.auth_reset_new_pw_input.is_empty() {
                                                if rpw_focused {
                                                    "▏".to_string()
                                                } else {
                                                    tr(
                                                        loc,
                                                        "New password (8+ chars)",
                                                        "新密码（至少 8 位）",
                                                    )
                                                    .to_string()
                                                }
                                            } else if rpw_focused {
                                                format!("{pw_dots}▏")
                                            } else {
                                                pw_dots
                                            })
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.input_target =
                                                        ActiveInputTarget::AuthResetNewPassword;
                                                    g.input_selected_all = false;
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("settings-btn-submit-reset-pw")
                                            .occlude()
                                            .flex_shrink_0()
                                            .px(px(11.0))
                                            .py(px(5.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(accent))
                                            .hover(|h| h.opacity(0.9))
                                            .cursor_pointer()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::CANVAS_BG))
                                            .child(if st.auth_loading {
                                                tr(loc, "Resetting…", "重置中…")
                                            } else {
                                                tr(loc, "Reset Password", "确认重置")
                                            })
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                this.submit_password_reset_token();
                                                cx.notify();
                                            })),
                                    ),
                            )
                        })
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
            .when(st.is_logged_in && st.account_is_admin, |el| {
                el.child(self.render_settings_admin_friends_card(st, cx))
            })
            .child({
                let wh = &st.webhook_settings;
                let bark_focus = st.input_target == ActiveInputTarget::WebhookBarkUrl;
                let tg_tok_focus = st.input_target == ActiveInputTarget::WebhookTelegramToken;
                let tg_chat_focus = st.input_target == ActiveInputTarget::WebhookTelegramChatId;
                let feishu_focus = st.input_target == ActiveInputTarget::WebhookFeishuUrl;
                let discord_focus = st.input_target == ActiveInputTarget::WebhookDiscordUrl;
                let generic_focus = st.input_target == ActiveInputTarget::WebhookGenericUrl;
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
                                                "Zero-plaintext metadata push via Bark, Telegram, Feishu, Discord & Generic HTTP",
                                                "零终端明文泄露，支持 Bark、Telegram、飞书、Discord 与自定义 Webhook",
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
                                                g.webhook_dirty = true;
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
                                                g.webhook_dirty = true;
                                            }
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.render_setting_pill(
                                        "settings-webhook-test",
                                        if st.webhook_dirty {
                                            tr(loc, "Save & Test Push", "保存并测试")
                                        } else {
                                            tr(loc, "Test Push", "测试推送")
                                        },
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
                    .child(
                        div()
                            .id("settings-input-webhook-generic")
                            .occlude()
                            .w_full()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(palette::CANVAS_BG))
                            .border_1()
                            .border_color(rgb(if generic_focus {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if wh.generic_webhook_url.is_empty() && !generic_focus {
                                palette::TEXT_DIM
                            } else {
                                palette::TEXT_PRIMARY
                            }))
                            .truncate()
                            .child(if wh.generic_webhook_url.is_empty() {
                                if generic_focus {
                                    "▏".to_string()
                                } else {
                                    tr(
                                        loc,
                                        "Generic Webhook URL (https://hooks.example.com/...)",
                                        "自定义 Webhook POST 地址 (https://hooks.example.com/...)",
                                    )
                                    .to_string()
                                }
                            } else {
                                format!(
                                    "{}{}",
                                    wh.generic_webhook_url,
                                    if generic_focus { "▏" } else { "" }
                                )
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_selected_all = false;
                                    g.input_target = ActiveInputTarget::WebhookGenericUrl;
                                }
                                cx.notify();
                            })),
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

    fn render_settings_app_update_card(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let upd = &st.self_update;
        let cur_ver = &upd.current_version;
        let has_update = upd.update_available;
        let latest_label = upd.latest_version.as_deref().unwrap_or(cur_ver);
        let subtitle = if let Some(ref prog) = upd.progress_message {
            prog.clone()
        } else if has_update {
            format!(
                "{} {latest_label} · {}",
                tr(loc, "New release", "发现新版本"),
                tr(
                    loc,
                    "SHA-256 verified in-place bundle upgrade",
                    "SHA-256 严格校验 · 一键覆盖并重启"
                )
            )
        } else if upd.latest_version.is_some() {
            format!(
                "{} (v{cur_ver})",
                tr(
                    loc,
                    "You are running the latest version",
                    "当前已是最新版本"
                )
            )
        } else {
            tr(
                loc,
                "Check /api/v1/site-info & GitHub Releases for Coda.app updates",
                "自动检查服务端与 GitHub Releases 最新版本",
            )
            .to_string()
        };

        div()
            .p(px(11.0))
            .rounded(px(7.0))
            .bg(rgb(palette::SURFACE_BG))
            .border_1()
            .border_color(rgb(if has_update {
                palette::DONE_GREEN
            } else {
                palette::BORDER
            }))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
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
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(format!(
                                                "{} · v{cur_ver}",
                                                tr(loc, "App Update", "客户端版本更新")
                                            )),
                                    )
                                    .when(has_update, |el| {
                                        el.child(
                                            div()
                                                .px(px(6.0))
                                                .py(px(1.0))
                                                .rounded(px(99.0))
                                                .bg(rgba(0x22c55e24))
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(palette::DONE_GREEN))
                                                .child(format!("✨ {latest_label}")),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(if upd.updating {
                                        palette::WORKING_SKY
                                    } else if has_update {
                                        palette::DONE_GREEN
                                    } else {
                                        palette::TEXT_DIM
                                    }))
                                    .truncate()
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(self.render_setting_pill(
                                "settings-btn-check-update",
                                if upd.checking {
                                    tr(loc, "Checking…", "检查中…")
                                } else {
                                    tr(loc, "Check for Updates", "检查更新")
                                },
                                false,
                                accent,
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.mesh_hub.check_desktop_self_update(true);
                                    cx.notify();
                                }),
                            ))
                            .when(has_update, |el| {
                                let is_updating = upd.updating;
                                el.child(
                                    div()
                                        .id("settings-btn-update-restart")
                                        .occlude()
                                        .px(px(10.0))
                                        .py(px(4.5))
                                        .rounded(px(5.0))
                                        .bg(rgb(palette::DONE_GREEN))
                                        .hover(|h| h.opacity(0.92))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0x09090b))
                                        .child(if is_updating {
                                            tr(loc, "Updating…", "正在下载安装…")
                                        } else {
                                            tr(loc, "Update & Restart", "一键更新并重启")
                                        })
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _, cx| {
                                                if !is_updating {
                                                    this.mesh_hub
                                                        .trigger_desktop_self_update_and_restart();
                                                }
                                                cx.notify();
                                            },
                                        )),
                                )
                            }),
                    ),
            )
            .when_some(upd.error_message.clone(), |el, err| {
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
    }

    fn render_settings_snippets_card(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let title_focus = st.input_target == ActiveInputTarget::SnippetTitleInput;
        let prompt_focus = st.input_target == ActiveInputTarget::SnippetPromptInput;

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
                                    .child(format!(
                                        "{} ({}/{MAX_PROMPT_SNIPPETS})",
                                        tr(
                                            loc,
                                            "Saved Prompt Snippets (⚡ ⌘K)",
                                            "跨设备常用提示词 (⚡ ⌘K)"
                                        ),
                                        st.prompt_snippets.len()
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(tr(
                                        loc,
                                        "Synced via /api/v1/me/snippets + cached in ~/.config/coda/snippets.json",
                                        "通过 /api/v1/me/snippets 多端同步，离线自动回退本地缓存",
                                    )),
                            ),
                    )
                    .child(self.render_setting_pill(
                        "settings-btn-sync-snippets",
                        if st.snippets_loading {
                            tr(loc, "Syncing…", "同步中…")
                        } else {
                            tr(loc, "Sync", "刷新同步")
                        },
                        false,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.mesh_hub.fetch_prompt_snippets();
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
                            .id("settings-input-snippet-title")
                            .occlude()
                            .w(px(148.0))
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::CHROME_BG))
                            .border_1()
                            .border_color(rgb(if title_focus {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(
                                if st.snippet_title_input.is_empty() && !title_focus {
                                    palette::TEXT_DIM
                                } else {
                                    palette::TEXT_PRIMARY
                                },
                            ))
                            .truncate()
                            .child(if st.snippet_title_input.is_empty() {
                                if title_focus {
                                    "▏".to_string()
                                } else {
                                    tr(loc, "Snippet title", "标题（如：跑测试）").to_string()
                                }
                            } else {
                                format!(
                                    "{}{}",
                                    st.snippet_title_input,
                                    if title_focus { "▏" } else { "" }
                                )
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_selected_all = false;
                                    g.input_target = ActiveInputTarget::SnippetTitleInput;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("settings-input-snippet-prompt")
                            .occlude()
                            .flex_1()
                            .min_w_0()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::CHROME_BG))
                            .border_1()
                            .border_color(rgb(if prompt_focus {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(
                                if st.snippet_prompt_input.is_empty() && !prompt_focus {
                                    palette::TEXT_DIM
                                } else {
                                    palette::TEXT_PRIMARY
                                },
                            ))
                            .truncate()
                            .child(if st.snippet_prompt_input.is_empty() {
                                if prompt_focus {
                                    "▏".to_string()
                                } else {
                                    tr(loc, "Prompt text to send…", "输入要一键发送的提示词内容…")
                                        .to_string()
                                }
                            } else {
                                let flat = st.snippet_prompt_input.replace('\n', " ↵ ");
                                format!("{flat}{}", if prompt_focus { "▏" } else { "" })
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_selected_all = false;
                                    g.input_target = ActiveInputTarget::SnippetPromptInput;
                                }
                                cx.notify();
                            })),
                    )
                    .child(self.render_setting_pill(
                        "settings-btn-add-snippet",
                        tr(loc, "+ Add", "+ 添加"),
                        true,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.submit_add_custom_snippet();
                            cx.notify();
                        }),
                    )),
            )
            .children(st.prompt_snippets.iter().enumerate().map(|(idx, snip)| {
                let del_id = snip.id.clone();
                let send_title = snip.title.clone();
                let send_prompt = snip.prompt.clone();
                let preview = snip.prompt.replace('\n', " ↵ ");
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(rgb(palette::CHROME_BG))
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
                                    .flex_shrink_0()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(format!("⚡ {}", snip.title)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(preview),
                            ),
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
                                        "settings-snip-send-{idx}"
                                    )))
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(10.0))
                                    .text_color(rgb(accent))
                                    .child(tr(loc, "Send", "发送"))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.settings_open = false;
                                            g.input_target = ActiveInputTarget::TerminalDirect;
                                        }
                                        this.send_prompt_snippet_to_session(
                                            None,
                                            &send_title,
                                            &send_prompt,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "settings-snip-del-{idx}"
                                    )))
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::ERROR_RED))
                                    .child("×")
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.delete_custom_snippet(&del_id);
                                        cx.notify();
                                    })),
                            ),
                    )
            }))
    }

    fn render_settings_admin_friends_card(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let note_focus = st.input_target == ActiveInputTarget::AdminInviteNote;
        let srv_base = st.server_url.clone();

        div()
            .p(px(10.0))
            .rounded(px(7.0))
            .bg(rgb(palette::SURFACE_BG))
            .border_1()
            .border_color(rgb(palette::BORDER))
            .flex()
            .flex_col()
            .gap(px(7.0))
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
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(tr(
                                        loc,
                                        "Admin · Friend Invites & Password Reset",
                                        "管理员 · 好友邀请注册与密码重置",
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(tr(
                                        loc,
                                        "Share 1-click registration links or mint 24h password reset links for friends",
                                        "一键复制好友注册专属链接，或为忘记密码的好友生成 24h 重置链接",
                                    )),
                            ),
                    )
                    .child(self.render_setting_pill(
                        "settings-admin-refresh",
                        if st.admin_loading {
                            "…"
                        } else {
                            tr(loc, "Refresh", "刷新")
                        },
                        false,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.mesh_hub.refresh_account_profile_and_admin_data();
                            cx.notify();
                        }),
                    )),
            )
            // Invite creation row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("settings-input-admin-invite-note")
                            .occlude()
                            .flex_1()
                            .min_w_0()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(palette::CANVAS_BG))
                            .border_1()
                            .border_color(rgb(if note_focus {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(
                                if st.admin_invite_note_input.is_empty() && !note_focus {
                                    palette::TEXT_DIM
                                } else {
                                    palette::TEXT_PRIMARY
                                },
                            ))
                            .truncate()
                            .child(if st.admin_invite_note_input.is_empty() {
                                if note_focus {
                                    "▏".to_string()
                                } else {
                                    tr(
                                        loc,
                                        "Optional friend note (e.g. Alice's MacBook)",
                                        "选填备注（如：好友小王）",
                                    )
                                    .to_string()
                                }
                            } else {
                                format!(
                                    "{}{}",
                                    st.admin_invite_note_input,
                                    if note_focus { "▏" } else { "" }
                                )
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_selected_all = false;
                                    g.input_target = ActiveInputTarget::AdminInviteNote;
                                }
                                cx.notify();
                            })),
                    )
                    .child(self.render_setting_pill(
                        "settings-admin-create-invite",
                        tr(loc, "+ New Invite Link", "+ 生成邀请链接"),
                        true,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.submit_create_admin_invite();
                            cx.notify();
                        }),
                    )),
            )
            // Active Invite Codes list
            .children(st.admin_invites.iter().map(|inv| {
                let code_copy = inv.code.clone();
                let code_del = inv.code.clone();
                let full_link = build_invite_registration_url(&srv_base, &inv.code);
                let meta = if inv.note.trim().is_empty() {
                    format!("{}/{}", inv.used_count, inv.max_uses)
                } else {
                    format!("{}/{} · {}", inv.used_count, inv.max_uses, inv.note)
                };
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(rgb(palette::CHROME_BG))
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
                            .gap(px(8.0))
                            .child(
                                div()
                                    .font_family("Menlo")
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(accent))
                                    .child(inv.code.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(meta),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .child(self.render_setting_pill(
                                SharedString::from(format!("settings-copy-invite-{}", inv.code)),
                                tr(loc, "Copy Invite Link", "复制邀请链接"),
                                false,
                                accent,
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        full_link.clone(),
                                    ));
                                    if let Ok(mut g) = this.state.lock() {
                                        g.status_notice = Some(format!(
                                            "{}: {code_copy}",
                                            tr(
                                                g.locale,
                                                "Copied invite link",
                                                "已复制邀请注册链接"
                                            )
                                        ));
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "settings-revoke-invite-{}",
                                        inv.code
                                    )))
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::ERROR_RED))
                                    .child("×")
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.mesh_hub.delete_admin_invite_code(code_del.clone());
                                        cx.notify();
                                    })),
                            ),
                    )
            }))
            // Registered Friends list (`GET /api/v1/admin/users`)
            .when(!st.admin_users.is_empty(), |el| {
                el.child(
                    div()
                        .pt(px(2.0))
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(palette::TEXT_DIM))
                        .child(format!(
                            "{} ({})",
                            tr(loc, "REGISTERED FRIENDS", "已注册成员"),
                            st.admin_users.len()
                        )),
                )
                .children(st.admin_users.iter().map(|u| {
                    let uid = u.id;
                    let u_handle = u.handle.clone();
                    let dev_badge = format!(
                        "{}/{} {}",
                        u.online_devices,
                        u.total_devices,
                        tr(loc, "online", "在线设备")
                    );
                    div()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(palette::CHROME_BG))
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
                                        .bg(rgb(if u.online_devices > 0 {
                                            palette::DONE_GREEN
                                        } else {
                                            palette::TEXT_DIM
                                        })),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(format!("@{}", u.handle)),
                                )
                                .when(u.is_admin, |b| {
                                    b.child(
                                        div()
                                            .px(px(4.0))
                                            .py(px(0.5))
                                            .rounded(px(3.0))
                                            .bg(rgb(palette::SURFACE_BG))
                                            .text_size(px(9.5))
                                            .text_color(rgb(accent))
                                            .child("admin"),
                                    )
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::TEXT_DIM))
                                        .child(dev_badge),
                                ),
                        )
                        .child(self.render_setting_pill(
                            SharedString::from(format!("settings-copy-reset-link-{uid}")),
                            tr(loc, "Copy Reset Link", "复制重置密码链接"),
                            false,
                            accent,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.mesh_hub
                                    .create_and_copy_friend_reset_link(uid, u_handle.clone());
                                cx.notify();
                            }),
                        ))
                }))
            })
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

impl CodaGpuWorkspaceView {
    pub(crate) fn render_remote_bootstrap_modal_overlay(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let bm = &st.bootstrap_modal;
        let parent_focus = st.input_target == ActiveInputTarget::BootstrapParentDir;
        let url_focus = st.input_target == ActiveInputTarget::BootstrapGitCloneUrl;
        let folder_focus = st.input_target == ActiveInputTarget::BootstrapFolderName;
        let derived_folder =
            crate::helpers::derive_folder_name_from_git_url(&bm.git_clone_url);
        let is_loading = bm.loading;

        div()
            .id("bootstrap-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .bg(rgba(0x000000aa))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !is_loading {
                    if let Ok(mut g) = this.state.lock() {
                        g.bootstrap_modal.reset();
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("bootstrap-modal")
                    .occlude()
                    .w(px(520.0))
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
                            .h(px(40.0))
                            .px(px(14.0))
                            .border_b_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(tr(
                                        loc,
                                        "New Remote Workspace / Git Clone",
                                        "新建远端工作区 / Git Clone",
                                    )),
                            )
                            .child(
                                div()
                                    .id("btn-close-bootstrap-modal")
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(12.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child("×")
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        if !is_loading {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.bootstrap_modal.reset();
                                                g.input_target = ActiveInputTarget::TerminalDirect;
                                            }
                                            cx.notify();
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .p(px(14.0))
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(tr(loc, "TARGET REMOTE HOST", "目标远端主机")),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(6.0))
                                            .when(st.remote_hosts.is_empty(), |empty_el| {
                                                empty_el.child(
                                                    div()
                                                        .id("bootstrap-empty-hosts-hint")
                                                        .w_full()
                                                        .p(px(9.0))
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
                                                                .flex_1()
                                                                .min_w_0()
                                                                .text_size(px(11.0))
                                                                .text_color(rgb(palette::TEXT_MUTED))
                                                                .child(tr(
                                                                    loc,
                                                                    "No remote hosts paired yet. Sign in and run the 1-line daemon install on your Linux/Mac server first.",
                                                                    "当前暂无已绑定的远端主机。请先登录账号并在目标服务器上执行一行装机命令。",
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .id("bootstrap-open-account-nodes")
                                                                .occlude()
                                                                .flex_shrink_0()
                                                                .px(px(8.0))
                                                                .py(px(4.0))
                                                                .rounded(px(4.0))
                                                                .bg(rgb(palette::CHROME_BG))
                                                                .border_1()
                                                                .border_color(rgb(accent))
                                                                .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                                                .cursor_pointer()
                                                                .text_size(px(10.5))
                                                                .text_color(rgb(accent))
                                                                .child(tr(
                                                                    loc,
                                                                    "Account & Nodes",
                                                                    "账号与节点",
                                                                ))
                                                                .on_click(cx.listener(
                                                                    |this, _: &ClickEvent, _, cx| {
                                                                        let is_logged = if let Ok(mut g) =
                                                                            this.state.lock()
                                                                        {
                                                                            g.bootstrap_modal.reset();
                                                                            g.settings_open = true;
                                                                            g.settings_section =
                                                                                SettingsSection::Machines;
                                                                            if !g.is_logged_in {
                                                                                g.auth_form_expanded =
                                                                                    true;
                                                                                g.input_target =
                                                                                    ActiveInputTarget::AuthAccount;
                                                                            }
                                                                            g.is_logged_in
                                                                        } else {
                                                                            false
                                                                        };
                                                                        if is_logged {
                                                                            this.reload_account_settings_data();
                                                                        }
                                                                        cx.notify();
                                                                    },
                                                                )),
                                                        ),
                                                )
                                            })
                                            .children(st.remote_hosts.iter().map(|host| {
                                                let hid = host.id;
                                                let is_sel = bm.host_id == Some(hid);
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "bootstrap-host-{hid}"
                                                    )))
                                                    .occlude()
                                                    .px(px(8.0))
                                                    .py(px(4.0))
                                                    .rounded(px(5.0))
                                                    .bg(rgb(if is_sel {
                                                        palette::HOVER_BG
                                                    } else {
                                                        palette::SURFACE_BG
                                                    }))
                                                    .border_1()
                                                    .border_color(rgb(if is_sel {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(5.0))
                                                    .child(
                                                        div()
                                                            .w(px(6.0))
                                                            .h(px(6.0))
                                                            .rounded(px(99.0))
                                                            .bg(rgb(if host.is_online {
                                                                palette::DONE_GREEN
                                                            } else {
                                                                palette::TEXT_DIM
                                                            })),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(if is_sel {
                                                                palette::TEXT_PRIMARY
                                                            } else {
                                                                palette::TEXT_MUTED
                                                            }))
                                                            .child(host.name.clone()),
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            if let Ok(mut g) = this.state.lock() {
                                                                g.bootstrap_modal.host_id =
                                                                    Some(hid);
                                                                g.bootstrap_modal.error = None;
                                                            }
                                                            cx.notify();
                                                        },
                                                    ))
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child(tr(
                                                loc,
                                                "Parent Directory on Remote Host",
                                                "远端父目录路径",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("bootstrap-input-parent-dir")
                                            .occlude()
                                            .w_full()
                                            .px(px(9.0))
                                            .py(px(6.0))
                                            .rounded(px(5.0))
                                            .bg(rgb(palette::CANVAS_BG))
                                            .border_1()
                                            .border_color(rgb(if parent_focus {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .cursor_pointer()
                                            .font_family("Menlo")
                                            .text_size(px(11.5))
                                            .text_color(rgb(
                                                if bm.parent_dir.is_empty() && !parent_focus {
                                                    palette::TEXT_DIM
                                                } else {
                                                    palette::TEXT_PRIMARY
                                                },
                                            ))
                                            .truncate()
                                            .child(if bm.parent_dir.is_empty() {
                                                if parent_focus {
                                                    "▏".to_string()
                                                } else {
                                                    "~/workspace".to_string()
                                                }
                                            } else {
                                                format!(
                                                    "{}{}",
                                                    bm.parent_dir,
                                                    if parent_focus { "▏" } else { "" }
                                                )
                                            })
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.input_selected_all = false;
                                                        g.input_target =
                                                            ActiveInputTarget::BootstrapParentDir;
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child(tr(
                                                loc,
                                                "Git Clone URL (Optional — leave blank to create an empty folder)",
                                                "Git Clone 地址（选填 — 留空则新建空文件夹）",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("bootstrap-input-git-url")
                                            .occlude()
                                            .w_full()
                                            .px(px(9.0))
                                            .py(px(6.0))
                                            .rounded(px(5.0))
                                            .bg(rgb(palette::CANVAS_BG))
                                            .border_1()
                                            .border_color(rgb(if url_focus {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .cursor_pointer()
                                            .font_family("Menlo")
                                            .text_size(px(11.5))
                                            .text_color(rgb(
                                                if bm.git_clone_url.is_empty() && !url_focus {
                                                    palette::TEXT_DIM
                                                } else {
                                                    palette::TEXT_PRIMARY
                                                },
                                            ))
                                            .truncate()
                                            .child(if bm.git_clone_url.is_empty() {
                                                if url_focus {
                                                    "▏".to_string()
                                                } else {
                                                    "https://github.com/org/repo.git".to_string()
                                                }
                                            } else {
                                                format!(
                                                    "{}{}",
                                                    bm.git_clone_url,
                                                    if url_focus { "▏" } else { "" }
                                                )
                                            })
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.input_selected_all = false;
                                                        g.input_target =
                                                            ActiveInputTarget::BootstrapGitCloneUrl;
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child(tr(
                                                loc,
                                                "Workspace Folder Name",
                                                "工作区文件夹名称",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("bootstrap-input-folder-name")
                                            .occlude()
                                            .w_full()
                                            .px(px(9.0))
                                            .py(px(6.0))
                                            .rounded(px(5.0))
                                            .bg(rgb(palette::CANVAS_BG))
                                            .border_1()
                                            .border_color(rgb(if folder_focus {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .cursor_pointer()
                                            .font_family("Menlo")
                                            .text_size(px(11.5))
                                            .text_color(rgb(
                                                if bm.folder_name.is_empty() && !folder_focus {
                                                    palette::TEXT_DIM
                                                } else {
                                                    palette::TEXT_PRIMARY
                                                },
                                            ))
                                            .truncate()
                                            .child(if bm.folder_name.is_empty() {
                                                if folder_focus {
                                                    "▏".to_string()
                                                } else if let Some(auto_name) = derived_folder {
                                                    format!(
                                                        "{auto_name} ({})",
                                                        tr(loc, "auto from URL", "自动取自仓库名")
                                                    )
                                                } else {
                                                    "my-project".to_string()
                                                }
                                            } else {
                                                format!(
                                                    "{}{}",
                                                    bm.folder_name,
                                                    if folder_focus { "▏" } else { "" }
                                                )
                                            })
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.input_selected_all = false;
                                                        g.input_target =
                                                            ActiveInputTarget::BootstrapFolderName;
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                    ),
                            )
                            .when_some(bm.error.clone(), |el, err| {
                                el.child(
                                    div()
                                        .px(px(9.0))
                                        .py(px(6.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(palette::CANVAS_BG))
                                        .border_1()
                                        .border_color(rgb(palette::ERROR_RED))
                                        .text_size(px(11.0))
                                        .text_color(rgb(palette::ERROR_RED))
                                        .child(err),
                                )
                            })
                            .child(
                                div()
                                    .pt(px(4.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(self.render_setting_pill(
                                        "btn-bootstrap-local-picker",
                                        tr(loc, "Browse Local Mac Folder…", "选择本机文件夹…"),
                                        false,
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.bootstrap_modal.reset();
                                                g.input_target = ActiveInputTarget::TerminalDirect;
                                            }
                                            this.add_local_workspace_folder();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(self.render_setting_pill(
                                                "btn-bootstrap-cancel",
                                                tr(loc, "Cancel", "取消"),
                                                false,
                                                accent,
                                                cx.listener(
                                                    move |this, _: &ClickEvent, _, cx| {
                                                        if !is_loading {
                                                            if let Ok(mut g) = this.state.lock() {
                                                                g.bootstrap_modal.reset();
                                                                g.input_target = ActiveInputTarget::TerminalDirect;
                                                            }
                                                            cx.notify();
                                                        }
                                                    },
                                                ),
                                            ))
                                            .child(
                                                div()
                                                    .id("btn-bootstrap-submit")
                                                    .occlude()
                                                    .px(px(12.0))
                                                    .py(px(5.0))
                                                    .rounded(px(5.0))
                                                    .bg(rgb(accent))
                                                    .hover(|h| h.opacity(0.92))
                                                    .cursor_pointer()
                                                    .text_size(px(11.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(0x09090b))
                                                    .child(if bm.loading {
                                                        tr(loc, "Working…", "正在处理…")
                                                    } else if bm.git_clone_url.trim().is_empty() {
                                                        tr(loc, "Create Folder", "创建文件夹")
                                                    } else {
                                                        tr(loc, "Clone & Open", "克隆并进入")
                                                    })
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            this.submit_remote_bootstrap_workspace();
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    ),
            )
    }
}
