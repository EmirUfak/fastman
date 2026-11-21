use crate::kv_editor::render_kv_table;
use crate::theme::{
    brand_accent, count_pill, dot_indicator, method_badge, method_mini_label, svg_icon,
};
use anyhow::{Context as _, Result};
use fastman_core::{ApiRequest, Auth, HttpBody, KeyValue};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    empty::{
        Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
        EmptyTitle,
    },
    input::{Editor, Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    tab::{Tab, TabBar},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::workspace::{AUTH_TYPES, BODY_TYPES, METHODS, WorkspaceView};

const COMMON_HEADERS: &[(&str, &str)] = &[
    ("Accept", "application/json"),
    ("Content-Type", "application/json"),
    ("Cache-Control", "no-cache"),
    ("User-Agent", "Fastman/0.1"),
];

pub(super) struct RequestDraft {
    name: String,
    method: String,
    url: String,
    params: Vec<KeyValue>,
    headers: Vec<KeyValue>,
    body: HttpBody,
    auth: Auth,
    timeout_ms: u64,
}

impl RequestDraft {
    pub(super) fn apply_to(self, request: &mut ApiRequest) {
        if !self.name.is_empty() {
            request.name = self.name;
        }
        request.method = self.method;
        request.url = self.url;
        request.params = self.params;
        request.headers = self.headers;
        request.body = self.body;
        request.auth = self.auth;
        request.timeout_ms = self.timeout_ms;
    }
}

impl WorkspaceView {
    pub(super) fn read_request_draft(&self, cx: &App) -> Result<RequestDraft> {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let timeout_ms = text(&self.timeout)
            .trim()
            .parse()
            .context("Timeout must be an integer number of milliseconds")?;
        let body = match self.body_kind {
            1 => HttpBody::Json(self.body_editor.read(cx).value().to_string()),
            2 => HttpBody::Text(self.body_editor.read(cx).value().to_string()),
            3 => HttpBody::FormUrlEncoded(self.form_table.to_key_values(cx)),
            _ => HttpBody::None,
        };
        let auth = match self.auth_kind {
            1 => Auth::Basic {
                username: text(&self.auth_user),
                password: text(&self.auth_password),
            },
            2 => Auth::Bearer {
                token: text(&self.auth_token),
            },
            3 => Auth::ApiKeyHeader {
                name: text(&self.api_key_name),
                value: text(&self.api_key_value),
            },
            4 => Auth::ApiKeyQuery {
                name: text(&self.api_key_name),
                value: text(&self.api_key_value),
            },
            _ => Auth::None,
        };
        Ok(RequestDraft {
            name: text(&self.request_name).trim().to_owned(),
            method: self.method.clone(),
            url: text(&self.url),
            params: self.params_table.to_key_values(cx),
            headers: self.headers_table.to_key_values(cx),
            body,
            auth,
            timeout_ms,
        })
    }

    pub(super) fn render_request_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let bg = cx.theme().background;
        let tab_bar_bg = cx.theme().tab_bar;
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let is_dark = cx.theme().is_dark();

        let mut tabs = Vec::new();
        for id in &self.open_tabs {
            if let Some(req) = self.current().find_request(id) {
                let id = id.clone();
                let close_id = id.clone();
                let selected = self.selected_id.as_ref() == Some(&id);
                let method = if selected {
                    self.method.as_str()
                } else {
                    req.method.as_str()
                };
                let title = if selected {
                    let live = self.request_name.read(cx).value().to_string();
                    if live.trim().is_empty() {
                        req.name.clone()
                    } else {
                        live
                    }
                } else {
                    req.name.clone()
                };

                tabs.push(
                    div()
                        .id(format!("doc-tab-{id}"))
                        .relative()
                        .h_full()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2p5()
                        .border_r_1()
                        .border_color(border)
                        .bg(if selected { bg } else { tab_bar_bg })
                        .when(!selected, |el| el.hover(|s| s.bg(cx.theme().list_hover)))
                        .when(selected, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .right_0()
                                    .h(px(2.))
                                    .bg(brand_accent(is_dark)),
                            )
                        })
                        .child(method_mini_label(method, cx))
                        .child(if selected && self.editing_request_name {
                            div()
                                .w(px(150.))
                                .child(Input::new(&self.request_name).xsmall())
                                .into_any_element()
                        } else {
                            div()
                                .id(format!("tab-open-{id}"))
                                .max_w(px(160.))
                                .truncate()
                                .cursor_pointer()
                                .text_size(px(12.))
                                .font_weight(if selected {
                                    FontWeight::MEDIUM
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(if selected {
                                    cx.theme().foreground
                                } else {
                                    muted_fg
                                })
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |view, _, window, cx| {
                                        view.open_request(id.clone(), window, cx);
                                    }
                                }))
                                .child(title)
                                .into_any_element()
                        })
                        .when(selected && self.dirty, |el| {
                            el.child(dot_indicator(brand_accent(is_dark)))
                        })
                        .when(selected, |el| {
                            el.child(
                                Button::new("edit-request-name")
                                    .ghost()
                                    .xsmall()
                                    .icon(
                                        svg_icon(if self.editing_request_name {
                                            "icons/check.svg"
                                        } else {
                                            "icons/pencil.svg"
                                        })
                                        .xsmall(),
                                    )
                                    .tooltip(if self.editing_request_name {
                                        "Finish renaming"
                                    } else {
                                        "Rename request"
                                    })
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.toggle_rename_request(window, cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new(format!("close-tab-{close_id}"))
                                .ghost()
                                .xsmall()
                                .icon(svg_icon("icons/x.svg").xsmall())
                                .tooltip("Close tab (Ctrl+W)")
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_tab(&close_id, window, cx);
                                })),
                        )
                        .into_any_element(),
                );
            }
        }

        let tab_strip = div()
            .h(px(36.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(tab_bar_bg)
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .id("tabs-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .child(
                div().px_1p5().flex_shrink_0().child(
                    Button::new("tab-new-request")
                        .ghost()
                        .xsmall()
                        .icon(svg_icon("icons/plus.svg").xsmall())
                        .tooltip("New request (Ctrl+N)")
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.add_request(window, cx);
                        })),
                ),
            );

        if self.selected_id.is_none() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .bg(bg)
                .child(tab_strip)
                .child(
                    div().flex_1().min_h_0().flex().items_center().justify_center().p_6().child(
                        Empty::new()
                            .header(
                                EmptyHeader::new()
                                    .media(
                                        EmptyMedia::new()
                                            .with_variant(EmptyMediaVariant::Icon)
                                            .child(svg_icon("icons/zap.svg")),
                                    )
                                    .title(EmptyTitle::new().child("No request open"))
                                    .description(EmptyDescription::new().child(
                                        "Select a request from the explorer, create a new one, or import a Collection / cURL.",
                                    )),
                            )
                            .content(
                                EmptyContent::new().child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Button::new("empty-new-request")
                                                .small()
                                                .primary()
                                                .icon(svg_icon("icons/file-plus.svg").small())
                                                .label("New Request")
                                                .on_click(cx.listener(|view, _, window, cx| {
                                                    view.add_request(window, cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("empty-import")
                                                .small()
                                                .outline()
                                                .icon(svg_icon("icons/import.svg").small())
                                                .label("Import")
                                                .on_click(cx.listener(|view, _, window, cx| {
                                                    view.open_import_dialog(window, cx);
                                                })),
                                        ),
                                ),
                            ),
                    ),
                )
                .into_any_element();
        }

        let method_view = cx.weak_entity();
        let current_method = self.method.clone();
        let in_flight = self.in_flight_request_id.is_some();
        let params_count = self.params_active_count;
        let headers_count = self.headers_active_count;
        let mono = cx.theme().mono_font_family.clone();

        let url_bar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .pt_2p5()
            .pb_2()
            .flex_shrink_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(32.))
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().input)
                    .bg(cx.theme().input_background())
                    .child(
                        Button::new("method")
                            .ghost()
                            .small()
                            .dropdown_caret(true)
                            .child(method_badge(&self.method, None, cx))
                            .dropdown_menu(move |mut menu, _, _| {
                                for method in METHODS {
                                    let view = method_view.clone();
                                    let checked = current_method == *method;
                                    menu =
                                        menu.item(
                                            PopupMenuItem::element(move |_, cx| {
                                                method_badge(method, Some(px(58.)), cx)
                                            })
                                            .checked(checked)
                                            .on_click(move |_, _, cx| {
                                                let _ = view.update(cx, |view, cx| {
                                                    view.method = (*method).into();
                                                    view.mark_dirty(cx);
                                                });
                                            }),
                                        );
                                }
                                menu
                            }),
                    )
                    .child(div().w(px(1.)).h(px(18.)).bg(border).flex_shrink_0())
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.url)
                                .small()
                                .appearance(false)
                                .font_family(mono)
                                .text_size(px(12.5)),
                        ),
                    ),
            )
            .child(if in_flight {
                Button::new("cancel")
                    .danger()
                    .icon(svg_icon("icons/square.svg").small())
                    .label("Cancel")
                    .tooltip("Cancel in-flight request")
                    .on_click(cx.listener(|view, _, _, cx| view.cancel(cx)))
            } else {
                Button::new("send")
                    .primary()
                    .icon(svg_icon("icons/send.svg").small())
                    .label("Send")
                    .tooltip("Send request (Ctrl+Enter)")
                    .on_click(cx.listener(|view, _, window, cx| view.send(Some(window), cx)))
            })
            .child(
                Button::new("save-request")
                    .outline()
                    .icon(svg_icon("icons/save.svg").small())
                    .label("Save")
                    .tooltip("Save request (Ctrl+S)")
                    .on_click(cx.listener(|view, _, window, cx| view.save_with_toast(window, cx))),
            )
            .child({
                let curl_view = cx.weak_entity();
                let has_sensitive = self.dirty
                    || self.auth_kind != 0
                    || self.current_request().is_some_and(|req| {
                        fastman_core::curl::to_curl_export(req, &self.current().variables, true)
                            .is_ok_and(|res| res.had_secrets || res.had_variables)
                    });
                Button::new("copy-curl")
                    .outline()
                    .dropdown_caret(true)
                    .icon(svg_icon("icons/terminal.svg").small())
                    .tooltip("Copy as cURL")
                    .dropdown_menu(move |mut menu, _, _| {
                        let v_safe = curl_view.clone();
                        menu = menu.item(
                            PopupMenuItem::new(if has_sensitive {
                                "Copy Redacted cURL (Safe)"
                            } else {
                                "Copy as cURL"
                            })
                            .icon(svg_icon(if has_sensitive {
                                "icons/shield-check.svg"
                            } else {
                                "icons/terminal.svg"
                            }))
                            .on_click(move |_, window, cx| {
                                let _ = v_safe.update(cx, |view, cx| {
                                    view.copy_request_as_curl(None, false, window, cx);
                                });
                            }),
                        );
                        if has_sensitive {
                            let v_full = curl_view.clone();
                            menu = menu.item(
                                PopupMenuItem::new("Copy Full cURL (Includes Secrets)...")
                                    .icon(svg_icon("icons/triangle-alert.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v_full.update(cx, |view, cx| {
                                            view.copy_request_as_curl(None, true, window, cx);
                                        });
                                    }),
                            );
                        }
                        menu
                    })
            });

        let body_tag = match self.body_kind {
            1 => Some("JSON"),
            2 => Some("Text"),
            3 => Some("Form"),
            _ => None,
        };
        let section_tabs = div()
            .px_3()
            .border_b_1()
            .border_color(border)
            .flex_shrink_0()
            .child(
                TabBar::new("request-sections")
                    .underline()
                    .small()
                    .selected_index(self.section)
                    .on_click(cx.listener(|view, idx, _, cx| {
                        view.section = *idx;
                        cx.notify();
                    }))
                    .child(
                        Tab::new()
                            .prefix(tab_icon("icons/sliders-horizontal.svg"))
                            .label("Params")
                            .when(params_count > 0, |t| t.suffix(count_pill(params_count, cx))),
                    )
                    .child(
                        Tab::new()
                            .prefix(tab_icon("icons/lock.svg"))
                            .label("Auth")
                            .when(self.auth_kind != 0, |t| {
                                t.suffix(dot_indicator(cx.theme().success))
                            }),
                    )
                    .child(
                        Tab::new()
                            .prefix(tab_icon("icons/layers.svg"))
                            .label("Headers")
                            .when(headers_count > 0, |t| {
                                t.suffix(count_pill(headers_count, cx))
                            }),
                    )
                    .child(
                        Tab::new()
                            .prefix(tab_icon("icons/braces.svg"))
                            .label("Body")
                            .when_some(body_tag, |t, tag| {
                                t.suffix(
                                    div()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(cx.theme().muted)
                                        .text_color(muted_fg)
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(tag),
                                )
                            }),
                    )
                    .child(
                        Tab::new()
                            .prefix(tab_icon("icons/code.svg"))
                            .label("Scripts"),
                    )
                    .child(
                        Tab::new()
                            .prefix(tab_icon("icons/settings.svg"))
                            .label("Settings"),
                    ),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .child(tab_strip)
            .child(url_bar)
            .child(section_tabs)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .flex()
                    .flex_col()
                    .child(self.render_request_section(cx)),
            )
            .into_any_element()
    }

    fn render_request_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted_fg = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let mono = cx.theme().mono_font_family.clone();

        match self.section {
            0 => {
                let has_query = self.url.read(cx).value().contains('?');
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(muted_fg)
                                    .child("Query Parameters (interpolates {{variables}})"),
                            )
                            .when(has_query, |el| {
                                el.child(
                                    Button::new("extract-url-params")
                                        .ghost()
                                        .xsmall()
                                        .icon(svg_icon("icons/sliders-horizontal.svg").xsmall())
                                        .label("Extract from URL")
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.extract_url_query_params(window, cx);
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("params-table-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(render_kv_table(
                                "params",
                                &self.params_table,
                                cx,
                                |view, idx, cx| {
                                    view.params_table.toggle_row(idx);
                                    view.refresh_kv_counts(cx);
                                    view.mark_dirty(cx);
                                },
                                |view, idx, window, cx| {
                                    view.remove_kv_row(0, idx, window, cx);
                                },
                                |view, window, cx| {
                                    view.add_kv_row(0, window, cx);
                                },
                            )),
                    )
                    .into_any_element()
            }
            1 => {
                let kind_label = AUTH_TYPES[self.auth_kind];
                let current_auth = self.auth_kind;
                let view = cx.weak_entity();
                div()
                    .id("auth-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(110.))
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(muted_fg)
                                    .child("Auth Type"),
                            )
                            .child(
                                div().w(px(220.)).child(
                                    Button::new("auth-kind")
                                        .small()
                                        .outline()
                                        .w_full()
                                        .dropdown_caret(true)
                                        .icon(svg_icon("icons/key-round.svg").small())
                                        .label(kind_label)
                                        .dropdown_menu(move |mut menu, _, _| {
                                            for (i, label) in AUTH_TYPES.iter().enumerate() {
                                                let view = view.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::new(*label)
                                                        .checked(current_auth == i)
                                                        .on_click(move |_, _, cx| {
                                                            let _ = view.update(cx, |view, cx| {
                                                                view.auth_kind = i;
                                                                view.mark_dirty(cx);
                                                            });
                                                        }),
                                                );
                                            }
                                            menu
                                        }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .max_w(px(540.))
                            .p_3()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(border)
                            .bg(cx.theme().secondary.opacity(0.35))
                            .flex()
                            .flex_col()
                            .gap_2p5()
                            .child(match self.auth_kind {
                                1 => div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(labeled_field(
                                        "Username",
                                        Input::new(&self.auth_user)
                                            .small()
                                            .font_family(mono.clone()),
                                        muted_fg,
                                    ))
                                    .child(labeled_field(
                                        "Password",
                                        Input::new(&self.auth_password)
                                            .small()
                                            .mask_toggle()
                                            .font_family(mono),
                                        muted_fg,
                                    ))
                                    .into_any_element(),
                                2 => labeled_field(
                                    "Bearer Token",
                                    Input::new(&self.auth_token)
                                        .small()
                                        .mask_toggle()
                                        .font_family(mono),
                                    muted_fg,
                                ),
                                3 | 4 => div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(labeled_field(
                                        if self.auth_kind == 3 {
                                            "Header Name"
                                        } else {
                                            "Query Param"
                                        },
                                        Input::new(&self.api_key_name)
                                            .small()
                                            .font_family(mono.clone()),
                                        muted_fg,
                                    ))
                                    .child(labeled_field(
                                        "API Key Value",
                                        Input::new(&self.api_key_value)
                                            .small()
                                            .mask_toggle()
                                            .font_family(mono),
                                        muted_fg,
                                    ))
                                    .into_any_element(),
                                _ => div()
                                    .text_size(px(12.5))
                                    .text_color(muted_fg)
                                    .child("No authentication headers or query credentials will be attached.")
                                    .into_any_element(),
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(11.5))
                            .text_color(muted_fg)
                            .child(
                                svg_icon("icons/shield-check.svg")
                                    .xsmall()
                                    .text_color(cx.theme().success),
                            )
                            .child("Session-only: Auth fields stay in memory and are excluded from saved project JSON (avoid putting secrets inside saved {{variables}})."),
                    )
                    .into_any_element()
            }
            2 => {
                let view = cx.weak_entity();
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(muted_fg)
                                    .child("Request Headers"),
                            )
                            .child(
                                Button::new("common-header-preset")
                                    .ghost()
                                    .xsmall()
                                    .dropdown_caret(true)
                                    .icon(svg_icon("icons/plus.svg").xsmall())
                                    .label("Common header")
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for (key, val) in COMMON_HEADERS {
                                            let view = view.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(format!("{key}: {val}"))
                                                    .on_click(move |_, window, cx| {
                                                        let _ = view.update(cx, |view, cx| {
                                                            view.append_header_preset(
                                                                key, val, window, cx,
                                                            );
                                                        });
                                                    }),
                                            );
                                        }
                                        menu
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("headers-table-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(render_kv_table(
                                "headers",
                                &self.headers_table,
                                cx,
                                |view, idx, cx| {
                                    view.headers_table.toggle_row(idx);
                                    view.refresh_kv_counts(cx);
                                    view.mark_dirty(cx);
                                },
                                |view, idx, window, cx| {
                                    view.remove_kv_row(1, idx, window, cx);
                                },
                                |view, window, cx| {
                                    view.add_kv_row(1, window, cx);
                                },
                            )),
                    )
                    .into_any_element()
            }
            3 => {
                let json_valid = self.body_kind != 1 || self.body_json_valid;

                let body_pills: Vec<AnyElement> = BODY_TYPES
                    .iter()
                    .enumerate()
                    .map(|(i, label)| {
                        let active = self.body_kind == i;
                        let mut btn = Button::new(format!("body-kind-{i}"))
                            .xsmall()
                            .label(*label)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.body_kind = i;
                                view.refresh_body_json_valid(cx);
                                view.mark_dirty(cx);
                            }));
                        btn = if active { btn.secondary() } else { btn.ghost() };
                        btn.into_any_element()
                    })
                    .collect();

                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .p(px(2.))
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().tab_bar_segmented)
                                    .flex()
                                    .items_center()
                                    .gap_0p5()
                                    .children(body_pills),
                            )
                            .when(self.body_kind == 1, |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .text_size(px(11.5))
                                                .text_color(if json_valid {
                                                    cx.theme().success
                                                } else {
                                                    cx.theme().danger
                                                })
                                                .child(
                                                    svg_icon(if json_valid {
                                                        "icons/check.svg"
                                                    } else {
                                                        "icons/triangle-alert.svg"
                                                    })
                                                    .xsmall(),
                                                )
                                                .child(if json_valid {
                                                    "Valid JSON"
                                                } else {
                                                    "Invalid JSON"
                                                }),
                                        )
                                        .child(
                                            Button::new("format-body-json")
                                                .ghost()
                                                .xsmall()
                                                .icon(svg_icon("icons/braces.svg").xsmall())
                                                .label("Format")
                                                .on_click(cx.listener(|view, _, window, cx| {
                                                    view.format_body_json(window, cx);
                                                })),
                                        ),
                                )
                            }),
                    )
                    .child(match self.body_kind {
                        0 => div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(border)
                            .child(
                                Empty::new().header(
                                    EmptyHeader::new()
                                        .media(
                                            EmptyMedia::new()
                                                .with_variant(EmptyMediaVariant::Icon)
                                                .child(svg_icon("icons/braces.svg")),
                                        )
                                        .title(EmptyTitle::new().child("No request body"))
                                        .description(EmptyDescription::new().child(
                                            "Choose JSON, Text, or Form URL Encoded above to attach a payload.",
                                        )),
                                ),
                            )
                            .into_any_element(),
                        3 => div()
                            .id("form-table-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(render_kv_table(
                                "form",
                                &self.form_table,
                                cx,
                                |view, idx, cx| {
                                    view.form_table.toggle_row(idx);
                                    view.mark_dirty(cx);
                                },
                                |view, idx, window, cx| {
                                    view.remove_kv_row(2, idx, window, cx);
                                },
                                |view, window, cx| {
                                    view.add_kv_row(2, window, cx);
                                },
                            ))
                            .into_any_element(),
                        _ => div()
                            .flex_1()
                            .min_h(px(120.))
                            .child(Editor::new(&self.body_editor).small().h(relative(1.)))
                            .into_any_element(),
                    })
                    .into_any_element()
            }
            4 => div()
                .max_w(px(620.))
                .child(
                    Alert::info(
                        "scripts-planned",
                        format!(
                            "Pre-request and post-response scripts are intentionally inert in v{} so imported collections never run untrusted code automatically.",
                            env!("CARGO_PKG_VERSION")
                        ),
                    )
                    .title("Scripts & Assertions (Planned Sandbox)"),
                )
                .into_any_element(),
            _ => div()
                .id("settings-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .max_w(px(540.))
                        .p_3()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(border)
                        .bg(cx.theme().secondary.opacity(0.35))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(labeled_field(
                            "Timeout (ms)",
                            Input::new(&self.timeout).small().font_family(mono),
                            muted_fg,
                        ))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(muted_fg)
                                .pl(px(118.))
                                .child("Clamped between 100 ms and 300,000 ms (5 minutes)."),
                        )
                        .child(div().h(px(1.)).bg(border))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(12.))
                                .child(div().text_color(muted_fg).child("TLS Certificate Verification"))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .text_color(cx.theme().success)
                                        .child(svg_icon("icons/shield-check.svg").xsmall())
                                        .child("Enabled"),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(12.))
                                .child(div().text_color(muted_fg).child("Redirect Policy"))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("Up to 10 same-origin hops"),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(12.))
                                .child(
                                    div()
                                        .text_color(muted_fg)
                                        .child("Cross-Origin & HTTPS Downgrade"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .text_color(cx.theme().success)
                                        .child(svg_icon("icons/shield-check.svg").xsmall())
                                        .child("Blocked"),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .text_size(px(12.))
                                .child(div().text_color(muted_fg).child("Environment Proxy"))
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(11.5))
                                        .child(
                                            fastman_core::http::configured_env_proxy_summary()
                                                .unwrap_or_else(|| {
                                                    "None in env (OS proxy may apply)".to_owned()
                                                }),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        }
    }
}

fn tab_icon(path: &'static str) -> AnyElement {
    div()
        .pr_1()
        .flex()
        .items_center()
        .child(svg_icon(path).xsmall())
        .into_any_element()
}

fn labeled_field(label: &'static str, control: impl IntoElement, label_color: Hsla) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(106.))
                .flex_shrink_0()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(label_color)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
        .into_any_element()
}
