use crate::json_highlight::render_code_viewer;
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

use super::WorkspaceView;

#[derive(Debug, Clone)]
pub struct ResponseCacheEntry {
    pub response: Option<HttpResponse>,
    pub error: Option<String>,
    pub sent_method: String,
    pub sent_url: String,
}

#[derive(Debug, Clone)]
struct ParsedCookie {
    name: String,
    value: String,
    attributes: String,
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
        let cookies = response.map(parse_cookies).unwrap_or_default();
        let cookie_count = cookies.len();

        let pretty_json = response.and_then(|r| {
            serde_json::from_str::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
        });
        let has_valid_json = pretty_json.is_some();

        let display_body = response.map(|r| {
            if !self.response_raw {
                pretty_json.clone().unwrap_or_else(|| r.body.clone())
            } else {
                r.body.clone()
            }
        });

        let search_query = self.response_search.read(cx).value().to_string();
        let (code_element, match_count) = if let Some(body_text) = &display_body {
            render_code_viewer(
                body_text,
                has_valid_json && !self.response_raw,
                self.response_wrap,
                if self.response_find_open {
                    &search_query
                } else {
                    ""
                },
                cx,
            )
        } else {
            (div().into_any_element(), 0)
        };

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
                let copy_text: SharedString = display_body.clone().unwrap_or_default().into();
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .flex_shrink_0()
                        .child(status_badge(resp.status, &resp.reason, cx))
                        .child(meta_pill(
                            Some("icons/clock.svg"),
                            format_duration(resp.elapsed_ms),
                            cx,
                        ))
                        .child(meta_pill(None, format_bytes(resp.bytes_received), cx))
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
                    .when(!search_query.trim().is_empty(), |el| {
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
                    let show_effective_url = entry
                        .is_some_and(|e| !e.sent_url.is_empty() && e.sent_url != resp.final_url);
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
                                            .map(|e| e.sent_method.as_str())
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
                        .child(
                            div()
                                .id("response-body-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .when(!self.response_wrap, |el| el.overflow_x_scroll())
                                .child(code_element),
                        )
                        .into_any_element()
                }
                1 => render_response_headers_table(&resp.headers, cx),
                _ => render_response_cookies_table(&cookies, cx),
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

fn meta_pill(icon: Option<&'static str>, label: String, cx: &App) -> AnyElement {
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
        .child(label)
        .into_any_element()
}

fn render_response_headers_table(headers: &[(String, String)], cx: &App) -> AnyElement {
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

    let mono = cx.theme().mono_font_family.clone();
    let border = cx.theme().border;
    let row_border = cx.theme().table_row_border;
    let head_bg = cx.theme().table_head;
    let head_fg = cx.theme().table_head_foreground;

    let rows: Vec<AnyElement> = headers
        .iter()
        .enumerate()
        .map(|(i, (name, value))| {
            let val_copy: SharedString = value.clone().into();
            div()
                .w_full()
                .min_h(px(30.))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(row_border)
                .hover(|s| s.bg(cx.theme().table_hover))
                .child(
                    div()
                        .w(px(220.))
                        .flex_shrink_0()
                        .px_3()
                        .py_1()
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .truncate()
                        .child(name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_3()
                        .py_1()
                        .border_l_1()
                        .border_color(row_border)
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
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
                            Clipboard::new(SharedString::from(format!("copy-header-{i}")))
                                .xsmall()
                                .value(val_copy)
                                .tooltip("Copy header value"),
                        ),
                )
                .into_any_element()
        })
        .collect();

    div()
        .id("response-headers-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_3()
        .child(
            div()
                .w_full()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .child(
                    div()
                        .h(px(28.))
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
                .children(rows),
        )
        .into_any_element()
}

fn render_response_cookies_table(cookies: &[ParsedCookie], cx: &App) -> AnyElement {
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

    let mono = cx.theme().mono_font_family.clone();
    let border = cx.theme().border;
    let row_border = cx.theme().table_row_border;
    let head_bg = cx.theme().table_head;
    let head_fg = cx.theme().table_head_foreground;

    let rows: Vec<AnyElement> = cookies
        .iter()
        .map(|cookie| {
            div()
                .w_full()
                .min_h(px(30.))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(row_border)
                .child(
                    div()
                        .w(px(170.))
                        .flex_shrink_0()
                        .px_3()
                        .py_1()
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
                        .py_1()
                        .border_l_1()
                        .border_color(row_border)
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .child(cookie.value.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_3()
                        .py_1()
                        .border_l_1()
                        .border_color(row_border)
                        .text_size(px(11.5))
                        .text_color(cx.theme().muted_foreground)
                        .child(if cookie.attributes.is_empty() {
                            "—".to_owned()
                        } else {
                            cookie.attributes.clone()
                        }),
                )
                .into_any_element()
        })
        .collect();

    div()
        .id("response-cookies-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_3()
        .child(
            div()
                .w_full()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .child(
                    div()
                        .h(px(28.))
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
                .children(rows),
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
                name: name.trim().to_owned(),
                value: value.trim().to_owned(),
                attributes: attrs,
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
    } else if lower.contains("dns") || lower.contains("connect") {
        "Connection Failed"
    } else {
        "Request Failed"
    }
}
