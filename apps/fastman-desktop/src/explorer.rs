use crate::theme::{method_badge, svg_icon};
use crate::workspace::NameDialogKind;
use fastman_core::{ApiRequest, Folder};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    input::Input,
    menu::{ContextMenuExt as _, PopupMenuItem},
    select::Select,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::WorkspaceView;

impl WorkspaceView {
    pub(super) fn render_explorer_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let sidebar_bg = cx.theme().sidebar;
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let tree_items = self.render_explorer_tree(&self.explorer_filter_lower, cx);
        let has_matches = !tree_items.is_empty();

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(sidebar_bg)
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted_fg)
                            .child("PROJECT"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted_fg)
                            .child(format!("{} req", self.cached_request_count)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Select::new(&self.project_selector).small()),
                    )
                    .child(
                        Button::new("create-project")
                            .small()
                            .outline()
                            .icon(svg_icon("icons/plus.svg").small())
                            .tooltip("New project")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_name_dialog(NameDialogKind::NewProject, window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .flex_shrink_0()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.explorer_search)
                                .small()
                                .cleanable(true)
                                .prefix(svg_icon("icons/search.svg").xsmall().text_color(muted_fg)),
                        ),
                    )
                    .child(
                        Button::new("create-request")
                            .small()
                            .outline()
                            .icon(svg_icon("icons/file-plus.svg").small())
                            .tooltip("New request (Ctrl+N)")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.add_request(window, cx);
                            })),
                    )
                    .child(
                        Button::new("create-folder")
                            .small()
                            .outline()
                            .icon(svg_icon("icons/folder-plus.svg").small())
                            .tooltip("New folder")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.add_folder(cx);
                            })),
                    ),
            )
            .child(div().h(px(1.)).bg(border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted_fg)
                            .child("COLLECTIONS"),
                    )
                    .when(!self.explorer_filter_lower.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(muted_fg)
                                .child("Filtered"),
                        )
                    }),
            )
            .child(
                div()
                    .id("explorer-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .when(has_matches, |el| el.children(tree_items))
                    .when(!has_matches, |el| {
                        el.justify_center().items_center().child(
                            Empty::new().header(
                                EmptyHeader::new()
                                    .media(
                                        EmptyMedia::new()
                                            .with_variant(EmptyMediaVariant::Icon)
                                            .child(svg_icon("icons/search.svg")),
                                    )
                                    .title(EmptyTitle::new().child("No matching requests"))
                                    .description(
                                        EmptyDescription::new()
                                            .child("Try another search term or add a request."),
                                    ),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_explorer_tree(&self, query: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        for collection in &self.current().collections {
            let id = collection.id.clone();
            let open = !query.is_empty() || self.expanded.contains(&id);
            let count = count_collection_requests(&collection.folders, &collection.requests);
            let title = collection.name.clone();
            let has_match = query.is_empty()
                || collection.name.to_ascii_lowercase().contains(query)
                || collection
                    .requests
                    .iter()
                    .any(|r| request_matches(r, query))
                || collection.folders.iter().any(|f| folder_matches(f, query));
            if !has_match {
                continue;
            }

            let view_ctx = cx.weak_entity();
            out.push(
                div()
                    .id(format!("collection-row-{id}"))
                    .w_full()
                    .h(px(28.))
                    .px_1p5()
                    .rounded(cx.theme().radius)
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().list_hover))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |view, _, _, cx| {
                            if !view.expanded.insert(id.clone()) {
                                view.expanded.remove(&id);
                            }
                            view.target_folder = None;
                            cx.notify();
                        }
                    }))
                    .context_menu(move |menu, _, _| {
                        let v1 = view_ctx.clone();
                        let v2 = view_ctx.clone();
                        menu.item(
                            PopupMenuItem::new("New Request")
                                .icon(svg_icon("icons/file-plus.svg"))
                                .on_click(move |_, window, cx| {
                                    let _ = v1.update(cx, |view, cx| {
                                        view.target_folder = None;
                                        view.add_request(window, cx);
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("New Folder")
                                .icon(svg_icon("icons/folder-plus.svg"))
                                .on_click(move |_, _, cx| {
                                    let _ = v2.update(cx, |view, cx| {
                                        view.target_folder = None;
                                        view.add_folder(cx);
                                    });
                                }),
                        )
                    })
                    .child(
                        svg_icon(if open {
                            "icons/chevron-down.svg"
                        } else {
                            "icons/chevron-right.svg"
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        svg_icon(if open {
                            "icons/folder-open.svg"
                        } else {
                            "icons/folder-closed.svg"
                        })
                        .small()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(count.to_string()),
                    )
                    .into_any_element(),
            );

            if open {
                for folder in &collection.folders {
                    self.render_folder(folder, 1., query, cx, &mut out);
                }
                for req in &collection.requests {
                    if query.is_empty() || request_matches(req, query) {
                        self.render_request_link(req, 1., cx, &mut out);
                    }
                }
            }
        }
        out
    }

    fn render_folder(
        &self,
        folder: &Folder,
        depth: f32,
        query: &str,
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
    ) {
        if !query.is_empty() && !folder_matches(folder, query) {
            return;
        }
        let id = folder.id.clone();
        let open = !query.is_empty() || self.expanded.contains(&id);
        let is_target = self.target_folder.as_ref() == Some(&id);
        let label = folder.name.clone();
        let count = count_collection_requests(&folder.folders, &folder.requests);
        let view_ctx = cx.weak_entity();
        let id_menu = id.clone();

        out.push(
            div()
                .pl(px(depth * 12.))
                .child(
                    div()
                        .id(format!("folder-row-{id}"))
                        .w_full()
                        .h(px(28.))
                        .px_1p5()
                        .rounded(cx.theme().radius)
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .when(is_target, |el| el.bg(cx.theme().accent))
                        .hover(|s| s.bg(cx.theme().list_hover))
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |view, _, _, cx| {
                                if !view.expanded.insert(id.clone()) {
                                    view.expanded.remove(&id);
                                }
                                view.target_folder = Some(id.clone());
                                cx.notify();
                            }
                        }))
                        .context_menu(move |menu, _, _| {
                            let id1 = id_menu.clone();
                            let id2 = id_menu.clone();
                            let id3 = id_menu.clone();
                            let id4 = id_menu.clone();
                            let v1 = view_ctx.clone();
                            let v2 = view_ctx.clone();
                            let v3 = view_ctx.clone();
                            let v4 = view_ctx.clone();
                            menu.item(
                                PopupMenuItem::new("New Request")
                                    .icon(svg_icon("icons/file-plus.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v1.update(cx, |view, cx| {
                                            view.target_folder = Some(id1.clone());
                                            view.add_request(window, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("New Subfolder")
                                    .icon(svg_icon("icons/folder-plus.svg"))
                                    .on_click(move |_, _, cx| {
                                        let _ = v2.update(cx, |view, cx| {
                                            view.add_subfolder(&id2, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Rename Folder")
                                    .icon(svg_icon("icons/pencil.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v3.update(cx, |view, cx| {
                                            view.open_name_dialog(
                                                NameDialogKind::RenameFolder(id3.clone()),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("Delete Folder")
                                    .icon(svg_icon("icons/trash.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v4.update(cx, |view, cx| {
                                            view.confirm_delete_folder(id4.clone(), window, cx);
                                        });
                                    }),
                            )
                        })
                        .child(
                            svg_icon(if open {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/chevron-right.svg"
                            })
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            svg_icon(if open {
                                "icons/folder-open.svg"
                            } else {
                                "icons/folder-closed.svg"
                            })
                            .small()
                            .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child(label),
                        )
                        .when(is_target, |el| {
                            let add_id = id.clone();
                            el.child(
                                Button::new(format!("folder-quick-add-{id}"))
                                    .ghost()
                                    .xsmall()
                                    .icon(svg_icon("icons/plus.svg").xsmall())
                                    .tooltip("Add request inside folder")
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        cx.stop_propagation();
                                        view.target_folder = Some(add_id.clone());
                                        view.add_request(window, cx);
                                    })),
                            )
                        })
                        .when(!is_target, |el| {
                            el.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(count.to_string()),
                            )
                        }),
                )
                .into_any_element(),
        );

        if open {
            for sub in &folder.folders {
                self.render_folder(sub, depth + 1., query, cx, out);
            }
            for req in &folder.requests {
                if query.is_empty() || request_matches(req, query) {
                    self.render_request_link(req, depth + 1., cx, out);
                }
            }
        }
    }

    fn render_request_link(
        &self,
        request: &ApiRequest,
        depth: f32,
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
    ) {
        let id = request.id.clone();
        let selected = self.selected_id.as_ref() == Some(&id);
        let method = request.method.clone();
        let title = request.name.clone();
        let view_ctx = cx.weak_entity();
        let id_menu = id.clone();

        out.push(
            div()
                .pl(px(depth * 12.))
                .child(
                    div()
                        .id(format!("request-row-{id}"))
                        .relative()
                        .w_full()
                        .h(px(30.))
                        .px_2()
                        .rounded(cx.theme().radius)
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .when(selected, |el| {
                            el.bg(cx.theme().list_active)
                                .border_1()
                                .border_color(cx.theme().list_active_border.opacity(0.4))
                        })
                        .when(!selected, |el| el.hover(|s| s.bg(cx.theme().list_hover)))
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |view, _, window, cx| {
                                view.open_request(id.clone(), window, cx);
                            }
                        }))
                        .context_menu(move |menu, _, _| {
                            let id1 = id_menu.clone();
                            let id2 = id_menu.clone();
                            let id3 = id_menu.clone();
                            let id4 = id_menu.clone();
                            let v1 = view_ctx.clone();
                            let v2 = view_ctx.clone();
                            let v3 = view_ctx.clone();
                            let v4 = view_ctx.clone();
                            menu.item(
                                PopupMenuItem::new("Rename")
                                    .icon(svg_icon("icons/pencil.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v1.update(cx, |view, cx| {
                                            view.open_request(id1.clone(), window, cx);
                                            view.editing_request_name = true;
                                            cx.notify();
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Duplicate")
                                    .icon(svg_icon("icons/copy.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v2.update(cx, |view, cx| {
                                            view.duplicate_request(&id2, window, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Copy as cURL")
                                    .icon(svg_icon("icons/terminal.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v3.update(cx, |view, cx| {
                                            view.copy_request_as_curl(Some(&id3), window, cx);
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("Delete")
                                    .icon(svg_icon("icons/trash.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v4.update(cx, |view, cx| {
                                            view.confirm_delete_request(id4.clone(), window, cx);
                                        });
                                    }),
                            )
                        })
                        .when(selected, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top(px(5.))
                                    .bottom(px(5.))
                                    .w(px(2.5))
                                    .rounded_full()
                                    .bg(crate::theme::brand_accent(cx.theme().is_dark())),
                            )
                        })
                        .child(method_badge(&method, Some(px(46.)), cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.5))
                                .font_weight(if selected {
                                    FontWeight::MEDIUM
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(if selected {
                                    cx.theme().foreground
                                } else {
                                    cx.theme().sidebar_foreground
                                })
                                .child(title),
                        ),
                )
                .into_any_element(),
        );
    }
}

fn request_matches(request: &ApiRequest, query: &str) -> bool {
    request.name.to_ascii_lowercase().contains(query)
        || request.method.to_ascii_lowercase().contains(query)
        || request.url.to_ascii_lowercase().contains(query)
}

fn folder_matches(folder: &Folder, query: &str) -> bool {
    folder.name.to_ascii_lowercase().contains(query)
        || folder.requests.iter().any(|r| request_matches(r, query))
        || folder.folders.iter().any(|f| folder_matches(f, query))
}

fn count_collection_requests(folders: &[Folder], requests: &[ApiRequest]) -> usize {
    requests.len()
        + folders
            .iter()
            .map(|f| count_collection_requests(&f.folders, &f.requests))
            .sum::<usize>()
}
