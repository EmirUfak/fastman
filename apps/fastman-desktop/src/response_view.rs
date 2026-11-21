use crate::json_highlight::{
    CodeLines, HighlightedLine, MAX_HIGHLIGHTED_JSON_LINES, MAX_PRETTY_JSON_SOURCE_BYTES,
    precompute_json_lines, precompute_plain_lines, render_virtual_code_lines,
};
use crate::theme::{count_pill, format_bytes, format_duration, status_badge, svg_icon};
use fastman_core::HttpResponse;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    input::Input,
    notification::Notification,
    spinner::Spinner,
    tab::{Tab, TabBar},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;

use super::WorkspaceView;

#[derive(Debug, Clone)]
pub struct PrecomputedResponse {
    pub status: u16,
    pub reason: SharedString,
    pub elapsed_label: SharedString,
    pub size_label: SharedString,
    pub truncated: bool,
    pub final_url: SharedString,
    pub raw_body: SharedString,
    pub pretty_body: Option<SharedString>,
    pub pretty_lines: Arc<Vec<HighlightedLine>>,
    pub raw_lines: Arc<Vec<SharedString>>,
    pub headers: Arc<Vec<(SharedString, SharedString)>>,
    pub cookies: Arc<Vec<ParsedCookie>>,
}

#[derive(Debug, Clone)]
pub struct ParsedCookie {
    pub name: SharedString,
    pub value: SharedString,
    pub attributes: SharedString,
}

#[derive(Debug, Clone)]
pub struct ResponseCacheEntry {
    pub response: Option<PrecomputedResponse>,
    pub error: Option<SharedString>,
    pub sent_method: SharedString,
    pub sent_url: SharedString,
    pub approx_bytes: usize,
}

impl ResponseCacheEntry {
    pub fn from_response(
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
                elapsed_label: SharedString::from(format_duration(response.elapsed_ms)),
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

    pub fn from_error(
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
            .h(px(38.))
            .px_3()
            .flex_shrink_0()
            .bg(tab_bar_bg)
            .border_t_1()
            .border_b_1()
            .border_color(border)
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
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
                            .prefix(
                                div()
                                    .pr_1()
                                    .flex()
                                    .items_center()
                                    .child(svg_icon("icons/braces.svg").xsmall()),
                            )
                            .label("Body"),
                    )
                    .child(
                        Tab::new()
                            .prefix(
                                div()
                                    .pr_1()
                                    .flex()
                                    .items_center()
                                    .child(svg_icon("icons/layers.svg").xsmall()),
                            )
                            .label("Headers")
                            .when(header_count > 0, |t| t.suffix(count_pill(header_count, cx))),
                    )
                    .child(
                        Tab::new()
                            .prefix(
                                div()
                                    .pr_1()
                                    .flex()
                                    .items_center()
                                    .child(svg_icon("icons/cookie.svg").xsmall()),
                            )
                            .label("Cookies")
                            .when(cookie_count > 0, |t| t.suffix(count_pill(cookie_count, cx))),
                    ),
            )
            .when_some(response, |bar, resp| {
                let copy_text = display_body.clone().unwrap_or_default();
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .flex_shrink_0()
                        .child(status_badge(resp.status, &resp.reason, cx))
                        .child(meta_pill(
                            Some("icons/clock.svg"),
                            resp.elapsed_label.clone(),
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
                                    .child(svg_icon("icons/triangle-alert.svg").xsmall())
                                    .child("2 MiB cap"),
                            )
                        })
                        .when(self.response_tab == 0, |row| {
                            row.when(has_valid_json, |r| {
                                let mut btn = Button::new("toggle-raw-pretty")
                                    .xsmall()
                                    .label(if self.response_raw { "Raw" } else { "Pretty" })
                                    .tooltip("Toggle formatted JSON / raw response")
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.response_raw = !view.response_raw;
                                        view.refresh_response_search_matches(cx);
                                        cx.notify();
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
                                        view.response_wrap = !view.response_wrap;
                                        cx.notify();
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
                                        view.response_find_open = !view.response_find_open;
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
                                        Notification::success("Response body copied to clipboard"),
                                        cx,
                                    );
                                }),
                        ),
                )
            });

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
                    let line_count = match &code_lines {
                        CodeLines::Json(lines) => lines.len(),
                        CodeLines::Plain(lines) => lines.len(),
                    };
                    let uses_virtual_list = !self.response_wrap || line_count > 400;
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
                    let body_container = if uses_virtual_list {
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .child(code_element)
                            .into_any_element()
                    } else {
                        div()
                            .id("response-body-wrapped-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(code_element)
                            .into_any_element()
                    };

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
