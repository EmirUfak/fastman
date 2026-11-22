use crate::response_body_view::{
    CodeLines, HighlightedLine, MAX_HIGHLIGHTED_JSON_LINES, MAX_PRETTY_JSON_SOURCE_BYTES,
    precompute_json_lines, precompute_plain_lines, render_virtual_code_lines,
};
use crate::theme::{brand_accent, count_pill, status_badge, svg_icon, tab_icon};
use fastman_core::HttpResponse;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    dialog::DialogEntrance,
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    input::Input,
    notification::Notification,
    spinner::Spinner,
    switch::Switch,
    tab::{Tab, TabBar},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::Path,
    sync::Arc,
};

use super::WorkspaceView;

const UI_SETTINGS_FILE: &str = "ui-settings.json";
const MAX_CACHED_RESPONSES: usize = 16;
const MAX_TOTAL_CACHE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ResponsePosition {
    Bottom,
    Left,
    #[default]
    Right,
}

impl ResponsePosition {
    const ALL: [Self; 3] = [Self::Bottom, Self::Left, Self::Right];

    fn as_str(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    fn from_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bottom" => Some(Self::Bottom),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Bottom => "Bottom",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }

    fn layout_hint(self) -> &'static str {
        match self {
            Self::Bottom => "Stacked",
            Self::Left | Self::Right => "Vertical split",
        }
    }

    fn icon_path(self) -> &'static str {
        match self {
            Self::Bottom => "icons/panel-bottom.svg",
            Self::Left => "icons/panel-left.svg",
            Self::Right => "icons/panel-right.svg",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct UiSettings {
    pub response_position: ResponsePosition,
    pub response_raw: bool,
    pub response_wrap: bool,
}

impl UiSettings {
    pub(super) fn load_from_root(root: &Path) -> Self {
        let path = root.join(UI_SETTINGS_FILE);
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Self::default();
        };
        Self {
            response_position: value
                .get("response_position")
                .and_then(|v| v.as_str())
                .and_then(ResponsePosition::from_str)
                .unwrap_or_default(),
            response_raw: value
                .get("response_raw")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            response_wrap: value
                .get("response_wrap")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    }

    pub(super) fn save_to_root(self, root: &Path) {
        if fs::create_dir_all(root).is_err() {
            return;
        }
        let payload = serde_json::json!({
            "response_position": self.response_position.as_str(),
            "response_raw": self.response_raw,
            "response_wrap": self.response_wrap,
        });
        if let Ok(encoded) = serde_json::to_vec_pretty(&payload) {
            let _ = fs::write(root.join(UI_SETTINGS_FILE), encoded);
        }
    }
}

struct ResponseSettingsDialogView {
    workspace: WeakEntity<WorkspaceView>,
}

impl ResponseSettingsDialogView {
    fn new(workspace: WeakEntity<WorkspaceView>) -> Self {
        Self { workspace }
    }
}

impl Render for ResponseSettingsDialogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (position, response_raw, response_wrap) = self
            .workspace
            .upgrade()
            .map(|entity| {
                let view = entity.read(cx);
                (
                    view.response_position,
                    view.response_raw,
                    view.response_wrap,
                )
            })
            .unwrap_or_default();

        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let secondary_bg = cx.theme().secondary;
        let is_dark = cx.theme().is_dark();
        let accent = brand_accent(is_dark);

        let position_cards = ResponsePosition::ALL.into_iter().map(|pos| {
            let selected = pos == position;
            let weak_ws = self.workspace.clone();
            let weak_modal = cx.weak_entity();
            div()
                .id(SharedString::from(format!("response-pos-{}", pos.as_str())))
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .p_2p5()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(if selected { accent } else { border })
                .bg(if selected {
                    accent.opacity(0.10)
                } else {
                    secondary_bg.opacity(0.45)
                })
                .hover(|s| {
                    s.bg(if selected {
                        accent.opacity(0.14)
                    } else {
                        secondary_bg.opacity(0.75)
                    })
                })
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .on_click(move |_, _, cx| {
                    let _ = weak_ws.update(cx, |view, cx| {
                        view.set_response_position(pos, cx);
                    });
                    let _ = weak_modal.update(cx, |_, cx| cx.notify());
                })
                .child(svg_icon(pos.icon_path()).small().text_color(if selected {
                    accent
                } else {
                    muted_fg
                }))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if selected {
                            cx.theme().foreground
                        } else {
                            muted_fg
                        })
                        .child(pos.label()),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(muted_fg)
                        .child(pos.layout_hint()),
                )
        });

        let weak_ws_pretty = self.workspace.clone();
        let weak_modal_pretty = cx.weak_entity();
        let weak_ws_wrap = self.workspace.clone();
        let weak_modal_wrap = cx.weak_entity();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Position"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(muted_fg)
                                    .child("Choose where the response panel is anchored."),
                            ),
                    )
                    .child(div().flex().items_center().gap_2().children(position_cards)),
            )
            .child(div().h(px(1.)).w_full().bg(border))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Display"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("Pretty-print JSON"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(muted_fg)
                                            .child("Format valid JSON responses automatically."),
                                    ),
                            )
                            .child(
                                Switch::new("response-setting-pretty")
                                    .checked(!response_raw)
                                    .on_click(move |_, _, cx| {
                                        let _ = weak_ws_pretty.update(cx, |view, cx| {
                                            view.toggle_response_raw(cx);
                                        });
                                        let _ = weak_modal_pretty.update(cx, |_, cx| cx.notify());
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("Word wrap"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(muted_fg)
                                            .child("Wrap long lines inside the response body."),
                                    ),
                            )
                            .child(
                                Switch::new("response-setting-wrap")
                                    .checked(response_wrap)
                                    .on_click(move |_, _, cx| {
                                        let _ = weak_ws_wrap.update(cx, |view, cx| {
                                            view.toggle_response_wrap(cx);
                                        });
                                        let _ = weak_modal_wrap.update(cx, |_, cx| cx.notify());
                                    }),
                            ),
                    ),
            )
    }
}

pub(super) fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub(super) fn format_duration(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.2} s", ms as f64 / 1000.0)
    }
}

pub(super) fn format_timing_breakdown(
    ttfb_ms: u128,
    download_ms: u128,
    elapsed_ms: u128,
) -> String {
    format!(
        "TTFB {} · Download {} · Total {}",
        format_duration(ttfb_ms),
        format_duration(download_ms),
        format_duration(elapsed_ms),
    )
}

#[derive(Debug, Clone)]
pub(super) struct PrecomputedResponse {
    status: u16,
    reason: SharedString,
    timing_label: SharedString,
    size_label: SharedString,
    truncated: bool,
    final_url: SharedString,
    pub(super) raw_body: SharedString,
    pub(super) pretty_body: Option<SharedString>,
    pretty_lines: Arc<Vec<HighlightedLine>>,
    raw_lines: Arc<Vec<SharedString>>,
    headers: Arc<Vec<(SharedString, SharedString)>>,
    cookies: Arc<Vec<ParsedCookie>>,
}

#[derive(Debug, Clone)]
struct ParsedCookie {
    name: SharedString,
    value: SharedString,
    attributes: SharedString,
}

#[derive(Debug, Clone)]
pub(super) struct ResponseCacheEntry {
    pub(super) response: Option<PrecomputedResponse>,
    pub(super) error: Option<SharedString>,
    pub(super) sent_method: SharedString,
    pub(super) sent_url: SharedString,
    approx_bytes: usize,
}

#[derive(Default)]
pub(super) struct ResponseCache {
    entries: HashMap<String, ResponseCacheEntry>,
    order: VecDeque<String>,
}

impl ResponseCache {
    pub(super) fn get(&self, request_id: &str) -> Option<&ResponseCacheEntry> {
        self.entries.get(request_id)
    }

    pub(super) fn insert(&mut self, request_id: String, entry: ResponseCacheEntry) {
        self.order.retain(|id| id != &request_id);
        self.order.push_back(request_id.clone());
        self.entries.insert(request_id, entry);

        let mut total_bytes: usize = self.entries.values().map(|e| e.approx_bytes).sum();
        while (self.entries.len() > MAX_CACHED_RESPONSES || total_bytes > MAX_TOTAL_CACHE_BYTES)
            && self.entries.len() > 1
        {
            let Some(oldest_id) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest_id) {
                total_bytes = total_bytes.saturating_sub(removed.approx_bytes);
            }
        }
    }

    pub(super) fn remove(&mut self, request_id: &str) {
        self.entries.remove(request_id);
        self.order.retain(|id| id != request_id);
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.entries.retain(|id, _| keep(id));
        self.order.retain(|id| self.entries.contains_key(id));
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

impl ResponseCacheEntry {
    pub(super) fn from_response(
        response: HttpResponse,
        sent_method: impl Into<SharedString>,
        sent_url: impl Into<SharedString>,
    ) -> Self {
        let mut pretty_body = (response.body.len() <= MAX_PRETTY_JSON_SOURCE_BYTES)
            .then(|| {
                serde_json::from_str::<serde_json::Value>(&response.body)
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
            })
            .flatten();
        if let Some(ref pretty) = pretty_body {
            let too_many_lines = pretty.lines().nth(MAX_HIGHLIGHTED_JSON_LINES).is_some();
            if pretty.len() > 2 * 1024 * 1024 || too_many_lines {
                pretty_body = None;
            }
        }
        let raw_lines = Arc::new(precompute_plain_lines(&response.body));
        let pretty_lines = pretty_body
            .as_deref()
            .map(precompute_json_lines)
            .map(Arc::new)
            .unwrap_or_else(|| Arc::new(Vec::new()));
        let cookies = Arc::new(parse_cookies(&response));
        let headers: Arc<Vec<(SharedString, SharedString)>> = Arc::new(
            response
                .headers
                .into_iter()
                .map(|(k, v)| (SharedString::from(k), SharedString::from(v)))
                .collect(),
        );
        let approx_bytes = response.body.len()
            + pretty_body.as_ref().map_or(0, String::len)
            + raw_lines.len() * 32
            + pretty_lines.len() * 64
            + headers.len() * 64;

        Self {
            response: Some(PrecomputedResponse {
                status: response.status,
                reason: SharedString::from(response.reason),
                timing_label: SharedString::from(format_timing_breakdown(
                    response.ttfb_ms,
                    response.download_ms,
                    response.elapsed_ms,
                )),
                size_label: SharedString::from(format_bytes(response.bytes_received)),
                truncated: response.truncated,
                final_url: SharedString::from(response.final_url),
                raw_body: SharedString::from(response.body),
                pretty_body: pretty_body.map(SharedString::from),
                pretty_lines,
                raw_lines,
                headers,
                cookies,
            }),
            error: None,
            sent_method: sent_method.into(),
            sent_url: sent_url.into(),
            approx_bytes,
        }
    }

    pub(super) fn from_error(
        error: impl Into<SharedString>,
        sent_method: impl Into<SharedString>,
        sent_url: impl Into<SharedString>,
    ) -> Self {
        let error = error.into();
        let approx_bytes = error.len() + 128;
        Self {
            response: None,
            error: Some(error),
            sent_method: sent_method.into(),
            sent_url: sent_url.into(),
            approx_bytes,
        }
    }
}

impl WorkspaceView {
    pub(super) fn ui_settings(&self) -> UiSettings {
        UiSettings {
            response_position: self.response_position,
            response_raw: self.response_raw,
            response_wrap: self.response_wrap,
        }
    }

    pub(super) fn persist_ui_settings(&self) {
        self.ui_settings().save_to_root(self.workspace_store.root());
    }

    pub(super) fn set_response_position(
        &mut self,
        position: ResponsePosition,
        cx: &mut Context<Self>,
    ) {
        if self.response_position == position {
            return;
        }
        self.response_position = position;
        self.persist_ui_settings();
        self.set_status(
            format!("Response position: {}", position.label()),
            false,
            cx,
        );
        cx.notify();
    }

    pub(super) fn toggle_response_raw(&mut self, cx: &mut Context<Self>) {
        self.response_raw = !self.response_raw;
        self.refresh_response_search_matches(cx);
        self.persist_ui_settings();
        cx.notify();
    }

    pub(super) fn toggle_response_wrap(&mut self, cx: &mut Context<Self>) {
        self.response_wrap = !self.response_wrap;
        self.persist_ui_settings();
        cx.notify();
    }

    pub(super) fn open_response_settings_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_workspace = cx.weak_entity();
        let modal_view = cx.new(|_| ResponseSettingsDialogView::new(weak_workspace));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(380.))
                .centered()
                .entrance(DialogEntrance::Fade)
                .title("Response Area Settings")
                .close_button(true)
                .overlay_closable(true)
                .keyboard(true)
                .child(modal_view.clone())
                .footer(
                    div().w_full().flex().justify_end().child(
                        Button::new("response-settings-done")
                            .small()
                            .primary()
                            .label("Done")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                            }),
                    ),
                )
        });
    }

    pub(super) fn render_response_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let bg = cx.theme().background;
        let tab_bar_bg = cx.theme().tab_bar;
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;

        let is_in_flight =
            self.selected_id.is_some() && self.in_flight_request_id == self.selected_id;
        let entry = self.active_response_entry();
        let response = entry.and_then(|e| e.response.as_ref());
        let error = entry.and_then(|e| e.error.as_deref());

        let header_count = response.map_or(0, |r| r.headers.len());
        let cookie_count = response.map_or(0, |r| r.cookies.len());
        let has_valid_json = response.is_some_and(|r| r.pretty_body.is_some());

        let display_body = response.map(|r| {
            if !self.response_raw {
                r.pretty_body.clone().unwrap_or_else(|| r.raw_body.clone())
            } else {
                r.raw_body.clone()
            }
        });

        let match_count = self.response_search_matches;

        let toolbar = div()
            .h(px(36.))
            .px_3()
            .flex_shrink_0()
            .bg(tab_bar_bg)
            .border_b_1()
            .border_color(border)
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div().flex_shrink_0().child(
                    TabBar::new("response-tabs")
                        .underline()
                        .small()
                        .selected_index(self.response_tab)
                        .on_click(cx.listener(|view, idx, _, cx| {
                            view.response_tab = *idx;
                            cx.notify();
                        }))
                        .child(
                            Tab::new()
                                .prefix(tab_icon("icons/braces.svg"))
                                .label("Body"),
                        )
                        .child(
                            Tab::new()
                                .prefix(tab_icon("icons/layers.svg"))
                                .label("Headers")
                                .when(header_count > 0, |t| t.suffix(count_pill(header_count, cx))),
                        )
                        .child(
                            Tab::new()
                                .prefix(tab_icon("icons/cookie.svg"))
                                .label("Cookies")
                                .when(cookie_count > 0, |t| t.suffix(count_pill(cookie_count, cx))),
                        ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1p5()
                    .child(
                        div()
                            .id("response-toolbar-actions")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_1p5()
                            .overflow_x_scroll()
                            .when_some(response, |bar, resp| {
                                let copy_text = display_body.clone().unwrap_or_default();
                                bar.child(status_badge(resp.status, &resp.reason, cx))
                                    .child(meta_pill(
                                        Some("icons/clock.svg"),
                                        resp.timing_label.clone(),
                                        cx,
                                    ))
                                    .child(meta_pill(None, resp.size_label.clone(), cx))
                                    .when(resp.truncated, |row| {
                                        row.child(
                                            div()
                                                .h(px(22.))
                                                .px_2()
                                                .rounded(px(5.))
                                                .bg(cx.theme().warning.opacity(0.18))
                                                .text_color(cx.theme().warning)
                                                .text_size(px(11.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .flex_shrink_0()
                                                .child(
                                                    svg_icon("icons/triangle-alert.svg").xsmall(),
                                                )
                                                .child("2 MiB cap"),
                                        )
                                    })
                                    .when(self.response_tab == 0, |row| {
                                        row.when(has_valid_json, |r| {
                                            let mut btn = Button::new("toggle-raw-pretty")
                                                .xsmall()
                                                .label(if self.response_raw {
                                                    "Raw"
                                                } else {
                                                    "Pretty"
                                                })
                                                .tooltip("Toggle formatted JSON / raw response")
                                                .on_click(cx.listener(|view, _, _, cx| {
                                                    view.toggle_response_raw(cx);
                                                }));
                                            btn = if !self.response_raw {
                                                btn.secondary()
                                            } else {
                                                btn.ghost()
                                            };
                                            r.child(btn)
                                        })
                                        .child({
                                            let mut btn = Button::new("toggle-wrap")
                                                .xsmall()
                                                .icon(svg_icon("icons/text-wrap.svg").xsmall())
                                                .tooltip("Toggle word wrap")
                                                .on_click(cx.listener(|view, _, _, cx| {
                                                    view.toggle_response_wrap(cx);
                                                }));
                                            btn = if self.response_wrap {
                                                btn.secondary()
                                            } else {
                                                btn.ghost()
                                            };
                                            btn
                                        })
                                        .child({
                                            let mut btn = Button::new("toggle-find")
                                                .xsmall()
                                                .icon(svg_icon("icons/search.svg").xsmall())
                                                .tooltip("Search response")
                                                .on_click(cx.listener(|view, _, window, cx| {
                                                    view.response_find_open =
                                                        !view.response_find_open;
                                                    if view.response_find_open {
                                                        view.response_search
                                                            .read(cx)
                                                            .focus_handle(cx)
                                                            .focus(window, cx);
                                                    }
                                                    view.refresh_response_search_matches(cx);
                                                    cx.notify();
                                                }));
                                            btn = if self.response_find_open {
                                                btn.secondary()
                                            } else {
                                                btn.ghost()
                                            };
                                            btn
                                        })
                                    })
                                    .child(
                                        Clipboard::new("copy-response-body")
                                            .xsmall()
                                            .value(copy_text)
                                            .tooltip("Copy response body")
                                            .on_copied(|_, window, cx| {
                                                window.push_notification(
                                                    Notification::success(
                                                        "Response body copied to clipboard",
                                                    ),
                                                    cx,
                                                );
                                            }),
                                    )
                            }),
                    )
                    .child(
                        Button::new("response-settings")
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/settings.svg").xsmall())
                            .tooltip("Response area settings")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_response_settings_dialog(window, cx);
                            })),
                    ),
            );

        let find_bar = (self.response_find_open && response.is_some() && self.response_tab == 0)
            .then(|| {
                div()
                    .h(px(34.))
                    .px_3()
                    .flex_shrink_0()
                    .bg(tab_bar_bg.opacity(0.65))
                    .border_b_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().w(px(280.)).child(
                            Input::new(&self.response_search)
                                .xsmall()
                                .cleanable(true)
                                .prefix(svg_icon("icons/search.svg").xsmall().text_color(muted_fg)),
                        ),
                    )
                    .when(!self.response_search_lower.is_empty(), |el| {
                        el.child(div().text_size(px(11.5)).text_color(muted_fg).child(
                            if match_count == 1 {
                                "1 match".to_owned()
                            } else {
                                format!("{match_count} matches")
                            },
                        ))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("close-response-find")
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/x.svg").xsmall())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.response_find_open = false;
                                view.refresh_response_search_matches(cx);
                                cx.notify();
                            })),
                    )
            });

        let content: AnyElement = if is_in_flight {
            let active_url = self.url.read(cx).value().to_string();
            let method = self.method.clone();
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .p_6()
                .child(Spinner::new().large())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("Sending {method} request…")),
                        )
                        .when(!active_url.trim().is_empty(), |el| {
                            el.child(
                                div()
                                    .max_w(px(480.))
                                    .truncate()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.5))
                                    .text_color(muted_fg)
                                    .child(active_url),
                            )
                        }),
                )
                .child(
                    Button::new("loading-cancel")
                        .small()
                        .outline()
                        .icon(svg_icon("icons/square.svg").xsmall())
                        .label("Cancel request")
                        .on_click(cx.listener(|view, _, _, cx| view.cancel(cx))),
                )
                .into_any_element()
        } else if let Some(err) = error {
            let category = classify_error(err);
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .child(
                    div()
                        .w_full()
                        .max_w(px(560.))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(Alert::error("response-error", err.to_owned()).title(category))
                        .child(
                            div().flex().justify_end().child(
                                Button::new("retry-request")
                                    .small()
                                    .outline()
                                    .icon(svg_icon("icons/rotate-cw.svg").small())
                                    .label("Retry")
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.send(Some(window), cx);
                                    })),
                            ),
                        ),
                )
                .into_any_element()
        } else if let Some(resp) = response {
            match self.response_tab {
                0 => {
                    let show_effective_url = entry.is_some_and(|e| {
                        !e.sent_url.is_empty() && e.sent_url.as_ref() != resp.final_url.as_ref()
                    });
                    let code_lines = if !self.response_raw && resp.pretty_body.is_some() {
                        CodeLines::Json(resp.pretty_lines.clone())
                    } else {
                        CodeLines::Plain(resp.raw_lines.clone())
                    };
                    let search_lower = if self.response_find_open {
                        self.response_search_lower.as_str()
                    } else {
                        ""
                    };
                    let code_element = render_virtual_code_lines(
                        code_lines,
                        self.response_wrap,
                        search_lower,
                        &self.response_scroll,
                        cx,
                    );
                    let body_container = div()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(code_element)
                        .into_any_element();

                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .when(show_effective_url, |el| {
                            el.child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .flex_shrink_0()
                                    .bg(cx.theme().secondary.opacity(0.4))
                                    .border_b_1()
                                    .border_color(border)
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_size(px(11.5))
                                    .text_color(muted_fg)
                                    .child(svg_icon("icons/globe.svg").xsmall())
                                    .child(format!(
                                        "{} URL:",
                                        entry
                                            .map(|e| e.sent_method.as_ref())
                                            .unwrap_or("Effective")
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .child(resp.final_url.clone()),
                                    ),
                            )
                        })
                        .child(body_container)
                        .into_any_element()
                }
                1 => render_response_headers_table(&resp.headers, &self.headers_scroll, cx),
                _ => render_response_cookies_table(&resp.cookies, &self.cookies_scroll, cx),
            }
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .child(
                    Empty::new().header(
                        EmptyHeader::new()
                            .media(
                                EmptyMedia::new()
                                    .with_variant(EmptyMediaVariant::Icon)
                                    .child(svg_icon("icons/zap.svg")),
                            )
                            .title(EmptyTitle::new().child("No response yet"))
                            .description(EmptyDescription::new().child(
                                "Press Send or Ctrl+Enter to inspect status, timing, headers, and body.",
                            )),
                    ),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .child(toolbar)
            .when_some(find_bar, |el, bar| el.child(bar))
            .child(content)
            .into_any_element()
    }
}

fn meta_pill(icon: Option<&'static str>, label: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .h(px(22.))
        .px_2()
        .rounded(px(5.))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .text_color(cx.theme().muted_foreground)
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.))
        .flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .when_some(icon, |el, path| el.child(svg_icon(path).xsmall()))
        .child(label.into())
        .into_any_element()
}

fn render_response_headers_table(
    headers: &Arc<Vec<(SharedString, SharedString)>>,
    scroll_handle: &UniformListScrollHandle,
    cx: &App,
) -> AnyElement {
    if headers.is_empty() {
        return div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Empty::new().header(
                    EmptyHeader::new()
                        .title(EmptyTitle::new().child("No response headers"))
                        .description(
                            EmptyDescription::new()
                                .child("The server returned no HTTP headers for this response."),
                        ),
                ),
            )
            .into_any_element();
    }

    let border = cx.theme().border;
    let row_border = cx.theme().table_row_border;
    let head_bg = cx.theme().table_head;
    let head_fg = cx.theme().table_head_foreground;
    let headers_list = headers.clone();

    div()
        .flex_1()
        .min_h_0()
        .p_3()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(28.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .bg(head_bg)
                        .text_color(head_fg)
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(div().w(px(220.)).px_3().child("HEADER"))
                        .child(
                            div()
                                .flex_1()
                                .px_3()
                                .border_l_1()
                                .border_color(row_border)
                                .child("VALUE"),
                        )
                        .child(div().w(px(34.))),
                )
                .child(
                    div().flex_1().min_h_0().overflow_hidden().child(
                        uniform_list(
                            "response-headers-list",
                            headers.len(),
                            move |range, _, cx| {
                                let mono = cx.theme().mono_font_family.clone();
                                let fg = cx.theme().foreground;
                                let muted_fg = cx.theme().muted_foreground;
                                let row_border = cx.theme().table_row_border;
                                let hover_bg = cx.theme().table_hover;

                                range
                                    .map(|i| {
                                        let (name, value) = &headers_list[i];
                                        div()
                                            .w_full()
                                            .h(px(30.))
                                            .flex()
                                            .items_center()
                                            .border_t_1()
                                            .border_color(row_border)
                                            .hover(move |s| s.bg(hover_bg))
                                            .child(
                                                div()
                                                    .w(px(220.))
                                                    .flex_shrink_0()
                                                    .px_3()
                                                    .font_family(mono.clone())
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(fg)
                                                    .truncate()
                                                    .child(name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .px_3()
                                                    .border_l_1()
                                                    .border_color(row_border)
                                                    .font_family(mono.clone())
                                                    .text_size(px(12.))
                                                    .text_color(muted_fg)
                                                    .truncate()
                                                    .child(value.clone()),
                                            )
                                            .child(
                                                div()
                                                    .w(px(34.))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        Clipboard::new(SharedString::from(
                                                            format!("copy-header-{i}"),
                                                        ))
                                                        .xsmall()
                                                        .value(value.clone())
                                                        .tooltip("Copy header value"),
                                                    ),
                                            )
                                            .into_any_element()
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(scroll_handle)
                        .size_full(),
                    ),
                ),
        )
        .into_any_element()
}

fn render_response_cookies_table(
    cookies: &Arc<Vec<ParsedCookie>>,
    scroll_handle: &UniformListScrollHandle,
    cx: &App,
) -> AnyElement {
    if cookies.is_empty() {
        return div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Empty::new().header(
                    EmptyHeader::new()
                        .media(
                            EmptyMedia::new()
                                .with_variant(EmptyMediaVariant::Icon)
                                .child(svg_icon("icons/cookie.svg")),
                        )
                        .title(EmptyTitle::new().child("No cookies returned"))
                        .description(
                            EmptyDescription::new()
                                .child("No Set-Cookie headers were present in this response."),
                        ),
                ),
            )
            .into_any_element();
    }

    let border = cx.theme().border;
    let row_border = cx.theme().table_row_border;
    let head_bg = cx.theme().table_head;
    let head_fg = cx.theme().table_head_foreground;
    let cookies_list = cookies.clone();

    div()
        .flex_1()
        .min_h_0()
        .p_3()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(28.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .bg(head_bg)
                        .text_color(head_fg)
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(div().w(px(170.)).px_3().child("NAME"))
                        .child(
                            div()
                                .flex_1()
                                .px_3()
                                .border_l_1()
                                .border_color(row_border)
                                .child("VALUE"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .px_3()
                                .border_l_1()
                                .border_color(row_border)
                                .child("ATTRIBUTES"),
                        ),
                )
                .child(
                    div().flex_1().min_h_0().overflow_hidden().child(
                        uniform_list(
                            "response-cookies-list",
                            cookies.len(),
                            move |range, _, cx| {
                                let mono = cx.theme().mono_font_family.clone();
                                let muted_fg = cx.theme().muted_foreground;
                                let row_border = cx.theme().table_row_border;

                                range
                                    .map(|i| {
                                        let cookie = &cookies_list[i];
                                        let attrs = if cookie.attributes.is_empty() {
                                            SharedString::new_static("—")
                                        } else {
                                            cookie.attributes.clone()
                                        };
                                        div()
                                            .w_full()
                                            .h(px(30.))
                                            .flex()
                                            .items_center()
                                            .border_t_1()
                                            .border_color(row_border)
                                            .child(
                                                div()
                                                    .w(px(170.))
                                                    .flex_shrink_0()
                                                    .px_3()
                                                    .font_family(mono.clone())
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .truncate()
                                                    .child(cookie.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .px_3()
                                                    .border_l_1()
                                                    .border_color(row_border)
                                                    .font_family(mono.clone())
                                                    .text_size(px(12.))
                                                    .truncate()
                                                    .child(cookie.value.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .px_3()
                                                    .border_l_1()
                                                    .border_color(row_border)
                                                    .text_size(px(11.5))
                                                    .text_color(muted_fg)
                                                    .truncate()
                                                    .child(attrs),
                                            )
                                            .into_any_element()
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(scroll_handle)
                        .size_full(),
                    ),
                ),
        )
        .into_any_element()
}

fn parse_cookies(response: &HttpResponse) -> Vec<ParsedCookie> {
    response
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, raw)| {
            let mut segments = raw.split(';').map(str::trim);
            let first = segments.next()?;
            let (name, value) = first.split_once('=')?;
            let attrs = segments
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            Some(ParsedCookie {
                name: SharedString::from(name.trim().to_owned()),
                value: SharedString::from(value.trim().to_owned()),
                attributes: SharedString::from(attrs),
            })
        })
        .collect()
}

fn classify_error(err: &str) -> &'static str {
    let lower = err.to_ascii_lowercase();
    if lower.contains("unknown variable") || lower.contains("unclosed variable") {
        "Unresolved Variable"
    } else if lower.contains("invalid url") {
        "Invalid URL"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "Request Timed Out"
    } else if lower.contains("redirect") {
        "Redirect Blocked"
    } else if lower.contains("exceeds maximum") {
        "Payload Too Large"
    } else if lower.contains("dns") || lower.contains("connect") {
        "Connection Failed"
    } else {
        "Request Failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_entry(msg: &str) -> ResponseCacheEntry {
        ResponseCacheEntry::from_error(
            msg.to_owned(),
            SharedString::new_static("GET"),
            SharedString::new_static("https://api.example.com"),
        )
    }

    #[::core::prelude::v1::test]
    fn response_cache_evicts_oldest_entries_beyond_count_limit() {
        let mut cache = ResponseCache::default();
        for i in 0..(MAX_CACHED_RESPONSES + 2) {
            cache.insert(format!("req-{i}"), error_entry("err"));
        }
        assert!(cache.get("req-0").is_none());
        assert!(cache.get("req-1").is_none());
        assert!(cache.get("req-2").is_some());
        assert!(
            cache
                .get(&format!("req-{}", MAX_CACHED_RESPONSES + 1))
                .is_some()
        );
    }

    #[::core::prelude::v1::test]
    fn response_cache_remove_and_retain_keep_order_consistent() {
        let mut cache = ResponseCache::default();
        cache.insert("a".into(), error_entry("1"));
        cache.insert("b".into(), error_entry("2"));
        cache.insert("c".into(), error_entry("3"));

        cache.remove("b");
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some());

        cache.retain(|id| id == "c");
        assert!(cache.get("a").is_none());
        assert!(cache.get("c").is_some());
    }

    #[::core::prelude::v1::test]
    fn timing_breakdown_formats_ttfb_download_and_total() {
        assert_eq!(
            format_timing_breakdown(42, 8, 50),
            "TTFB 42 ms · Download 8 ms · Total 50 ms"
        );
        assert_eq!(
            format_timing_breakdown(250, 1250, 1500),
            "TTFB 250 ms · Download 1.25 s · Total 1.50 s"
        );

        let entry = ResponseCacheEntry::from_response(
            HttpResponse {
                status: 200,
                reason: "OK".into(),
                ttfb_ms: 15,
                download_ms: 5,
                elapsed_ms: 20,
                final_url: "https://api.example.com/health".into(),
                headers: Vec::new(),
                body: "ok".into(),
                bytes_received: 2,
                truncated: false,
            },
            SharedString::new_static("GET"),
            SharedString::new_static("https://api.example.com/health"),
        );
        assert_eq!(
            entry.response.unwrap().timing_label.as_ref(),
            "TTFB 15 ms · Download 5 ms · Total 20 ms"
        );
    }
}
