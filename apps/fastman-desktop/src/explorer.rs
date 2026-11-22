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

#[derive(Clone)]
struct DraggedRequest {
    id: String,
    method: String,
    name: String,
}

struct DraggedRequestPreview(DraggedRequest);

impl Render for DraggedRequestPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2p5()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().drag_border)
            .shadow_md()
            .flex()
            .items_center()
            .gap_2()
            .child(method_badge(&self.0.method, Some(px(42.)), cx))
            .child(
                div()
                    .max_w(px(180.))
                    .truncate()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .child(self.0.name.clone()),
            )
    }
}

#[derive(Clone)]
struct DraggedFolder {
    id: String,
    name: String,
}

struct DraggedFolderPreview(DraggedFolder);

impl Render for DraggedFolderPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2p5()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().drag_border)
            .shadow_md()
            .flex()
            .items_center()
            .gap_1p5()
            .child(
                svg_icon("icons/folder-closed.svg")
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .max_w(px(180.))
                    .truncate()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .child(self.0.name.clone()),
            )
    }
}

fn drag_highlight_box(style: StyleRefinement, cx: &App) -> StyleRefinement {
    let accent = crate::theme::brand_accent(cx.theme().is_dark());
    style
        .bg(accent.opacity(0.14))
        .border_1()
        .border_color(accent)
}

fn drag_line_top(style: StyleRefinement, cx: &App) -> StyleRefinement {
    let accent = crate::theme::brand_accent(cx.theme().is_dark());
    style
        .border_t_2()
        .border_color(accent)
        .bg(accent.opacity(0.08))
}

fn drag_line_bottom(style: StyleRefinement, cx: &App) -> StyleRefinement {
    let accent = crate::theme::brand_accent(cx.theme().is_dark());
    style
        .border_b_2()
        .border_color(accent)
        .bg(accent.opacity(0.08))
}

fn root_explorer_context_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    weak_view: WeakEntity<WorkspaceView>,
) -> gpui_kit::component::menu::PopupMenu {
    let v1 = weak_view.clone();
    let v2 = weak_view;
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
}

impl WorkspaceView {
    pub(super) fn render_explorer_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let sidebar_bg = cx.theme().sidebar;
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let tree_items = self.render_explorer_tree(&self.explorer_filter_lower, cx);
        let has_matches = !tree_items.is_empty();
        let is_dragging = cx.has_active_drag();

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
            .child({
                let header_view = cx.weak_entity();
                div()
                    .id("collections-root-drop")
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_1()
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .when(is_dragging, |el| {
                        el.border_1().border_dashed().border_color(border)
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.target_folder = None;
                        cx.notify();
                    }))
                    .drag_over::<DraggedRequest>(|style, _, _, cx| drag_highlight_box(style, cx))
                    .on_drop(cx.listener(|view, drag: &DraggedRequest, window, cx| {
                        view.move_request_to_folder(&drag.id, None, window, cx);
                    }))
                    .drag_over::<DraggedFolder>(|style, _, _, cx| drag_highlight_box(style, cx))
                    .on_drop(cx.listener(|view, drag: &DraggedFolder, window, cx| {
                        view.move_folder_to_folder(&drag.id, None, window, cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        root_explorer_context_menu(menu, header_view.clone())
                    })
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted_fg)
                            .child("EXPLORER"),
                    )
                    .when(is_dragging && self.explorer_filter_lower.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(muted_fg)
                                .child("Drop here for root"),
                        )
                    })
                    .when(!self.explorer_filter_lower.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(muted_fg)
                                .child("Filtered"),
                        )
                    })
            })
            .child(
                div()
                    .id("explorer-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .when(has_matches, |el| {
                        let empty_space_view = cx.weak_entity();
                        el.children(tree_items).child(
                            div()
                                .id("explorer-root-drop")
                                .flex_1()
                                .min_h(px(40.))
                                .rounded(cx.theme().radius)
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(is_dragging, |el| {
                                    el.border_1().border_dashed().border_color(border)
                                })
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.target_folder = None;
                                    cx.notify();
                                }))
                                .drag_over::<DraggedRequest>(|style, _, _, cx| {
                                    drag_highlight_box(style, cx)
                                })
                                .on_drop(cx.listener(|view, drag: &DraggedRequest, window, cx| {
                                    view.move_request_to_folder(&drag.id, None, window, cx);
                                }))
                                .drag_over::<DraggedFolder>(|style, _, _, cx| {
                                    drag_highlight_box(style, cx)
                                })
                                .on_drop(cx.listener(|view, drag: &DraggedFolder, window, cx| {
                                    view.move_folder_to_folder(&drag.id, None, window, cx);
                                }))
                                .context_menu(move |menu, _, _| {
                                    root_explorer_context_menu(menu, empty_space_view.clone())
                                })
                                .when(is_dragging, |el| {
                                    el.child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(muted_fg)
                                            .child("Drop here to move to root"),
                                    )
                                }),
                        )
                    })
                    .when(!has_matches, |el| {
                        let no_match_view = cx.weak_entity();
                        let empty_filter = self.explorer_filter_lower.is_empty();
                        el.child(
                            div()
                                .id("explorer-empty-state")
                                .flex_1()
                                .size_full()
                                .flex()
                                .justify_center()
                                .items_center()
                                .context_menu(move |menu, _, _| {
                                    root_explorer_context_menu(menu, no_match_view.clone())
                                })
                                .child(
                                    Empty::new().header(
                                        EmptyHeader::new()
                                            .media(
                                                EmptyMedia::new()
                                                    .with_variant(EmptyMediaVariant::Icon)
                                                    .child(svg_icon(if empty_filter {
                                                        "icons/folder-open.svg"
                                                    } else {
                                                        "icons/search.svg"
                                                    })),
                                            )
                                            .title(EmptyTitle::new().child(if empty_filter {
                                                "No requests or folders yet"
                                            } else {
                                                "No matching requests"
                                            }))
                                            .description(EmptyDescription::new().child(
                                                if empty_filter {
                                                    "Add a request or folder above, or right-click here."
                                                } else {
                                                    "Try another search term or add a request."
                                                },
                                            )),
                                    ),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_tree_children(
        &self,
        folders: &[Folder],
        requests: &[ApiRequest],
        depth: f32,
        ancestors: &[String],
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
    ) {
        let query = self.explorer_filter_lower.as_str();
        for folder in folders {
            self.render_folder(folder, depth, ancestors, query, cx, out);
        }
        for req in requests {
            if query.is_empty() || request_matches(req, query) {
                self.render_request_link(req, depth, ancestors, cx, out);
            }
        }
    }

    fn render_explorer_tree(&self, query: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let collections = &self.current().collections;
        if collections.len() <= 1 {
            if let Some(collection) = collections.first() {
                self.render_tree_children(
                    &collection.folders,
                    &collection.requests,
                    0.,
                    &[],
                    cx,
                    &mut out,
                );
            }
            return out;
        }

        let coll_len = collections.len();
        for (coll_idx, collection) in collections.iter().enumerate() {
            let id = collection.id.clone();
            let open = !query.is_empty() || self.expanded.contains(&id);
            let count = collection.request_count();
            let title = collection.name.clone();
            let can_move_up = coll_idx > 0;
            let can_move_down = coll_idx + 1 < coll_len;
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
            let id_menu = id.clone();
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
                    .drag_over::<DraggedRequest>(|style, _, _, cx| drag_highlight_box(style, cx))
                    .on_drop(cx.listener({
                        let id = id.clone();
                        move |view, drag: &DraggedRequest, window, cx| {
                            view.move_request_to_collection(&drag.id, &id, window, cx);
                        }
                    }))
                    .drag_over::<DraggedFolder>(|style, _, _, cx| drag_highlight_box(style, cx))
                    .on_drop(cx.listener({
                        let id = id.clone();
                        move |view, drag: &DraggedFolder, window, cx| {
                            view.move_folder_to_collection(&drag.id, &id, window, cx);
                        }
                    }))
                    .context_menu(move |menu, _, _| {
                        let v1 = view_ctx.clone();
                        let v2 = view_ctx.clone();
                        let v_ren = view_ctx.clone();
                        let v_up = view_ctx.clone();
                        let v_down = view_ctx.clone();
                        let v_del = view_ctx.clone();
                        let id_ren = id_menu.clone();
                        let id_up = id_menu.clone();
                        let id_down = id_menu.clone();
                        let id_del = id_menu.clone();
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
                        .item(
                            PopupMenuItem::new("Rename Collection")
                                .icon(svg_icon("icons/pencil.svg"))
                                .on_click(move |_, window, cx| {
                                    let _ = v_ren.update(cx, |view, cx| {
                                        view.open_name_dialog(
                                            NameDialogKind::RenameCollection(id_ren.clone()),
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("Move Up")
                                .icon(svg_icon("icons/arrow-up.svg"))
                                .disabled(!can_move_up)
                                .on_click(move |_, window, cx| {
                                    let _ = v_up.update(cx, |view, cx| {
                                        view.move_collection_offset(&id_up, -1, window, cx);
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Move Down")
                                .icon(svg_icon("icons/arrow-down.svg"))
                                .disabled(!can_move_down)
                                .on_click(move |_, window, cx| {
                                    let _ = v_down.update(cx, |view, cx| {
                                        view.move_collection_offset(&id_down, 1, window, cx);
                                    });
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("Delete Collection")
                                .icon(svg_icon("icons/trash.svg"))
                                .on_click(move |_, window, cx| {
                                    let _ = v_del.update(cx, |view, cx| {
                                        view.confirm_delete_collection(id_del.clone(), window, cx);
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
                self.render_tree_children(
                    &collection.folders,
                    &collection.requests,
                    1.,
                    &[],
                    cx,
                    &mut out,
                );
            }
        }
        out
    }

    fn render_folder(
        &self,
        folder: &Folder,
        depth: f32,
        ancestors: &[String],
        query: &str,
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
    ) {
        if depth as usize > fastman_core::model::MAX_FOLDER_DEPTH {
            return;
        }
        if !query.is_empty() && !folder_matches(folder, query) {
            return;
        }
        let id = folder.id.clone();
        let open = !query.is_empty() || self.expanded.contains(&id);
        let is_target = self.target_folder.as_ref() == Some(&id);
        let label = folder.name.clone();
        let count = folder.request_count();
        let (folder_idx, folder_len) = self.current().folder_order_bounds(&id).unwrap_or((0, 1));
        let can_move_up = folder_idx > 0;
        let can_move_down = folder_idx + 1 < folder_len;
        let parent_folder = self.current().folder_parent_folder(&id);
        let valid_folder_targets: Vec<(String, String)> = self
            .current()
            .folder_targets()
            .into_iter()
            .filter(|(fid, _)| fid != &id && !self.current().is_folder_descendant(&id, fid))
            .collect();
        let mut disallowed_drag_ids = ancestors.to_vec();
        disallowed_drag_ids.push(id.clone());
        let drag_payload = DraggedFolder {
            id: id.clone(),
            name: label.clone(),
        };
        let show_folder_drop_zones = cx.has_active_drag();
        let view_ctx = cx.weak_entity();
        let id_menu = id.clone();

        out.push(
            div()
                .pl(px(depth * 12.))
                .child(
                    div()
                        .id(format!("folder-row-{id}"))
                        .relative()
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
                        .on_drag(drag_payload, |drag, _, _, cx| {
                            cx.new(|_| DraggedFolderPreview(drag.clone()))
                        })
                        .drag_over::<DraggedRequest>(|style, _, _, cx| {
                            drag_highlight_box(style, cx)
                        })
                        .on_drop(cx.listener({
                            let id = id.clone();
                            move |view, drag: &DraggedRequest, window, cx| {
                                view.move_request_to_folder(&drag.id, Some(&id), window, cx);
                            }
                        }))
                        .when(show_folder_drop_zones, |el| {
                            let disallowed_top = disallowed_drag_ids.clone();
                            let disallowed_mid = disallowed_drag_ids.clone();
                            let disallowed_bot = disallowed_drag_ids.clone();
                            let id_top = id.clone();
                            let id_mid = id.clone();
                            let id_bot = id.clone();
                            el.child(
                                div()
                                    .id(format!("folder-drop-before-{id}"))
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .right_0()
                                    .h(px(8.))
                                    .drag_over::<DraggedFolder>(move |style, drag, _, cx| {
                                        if disallowed_top.contains(&drag.id) {
                                            style
                                        } else {
                                            drag_line_top(style, cx)
                                        }
                                    })
                                    .on_drop(cx.listener(
                                        move |view, drag: &DraggedFolder, window, cx| {
                                            view.move_folder_relative(
                                                &drag.id, &id_top, false, window, cx,
                                            );
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id(format!("folder-drop-into-{id}"))
                                    .absolute()
                                    .top(px(8.))
                                    .bottom(px(8.))
                                    .left_0()
                                    .right_0()
                                    .rounded(cx.theme().radius)
                                    .drag_over::<DraggedFolder>(move |style, drag, _, cx| {
                                        if disallowed_mid.contains(&drag.id) {
                                            style
                                        } else {
                                            drag_highlight_box(style, cx)
                                        }
                                    })
                                    .on_drop(cx.listener(
                                        move |view, drag: &DraggedFolder, window, cx| {
                                            view.move_folder_to_folder(
                                                &drag.id,
                                                Some(&id_mid),
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id(format!("folder-drop-after-{id}"))
                                    .absolute()
                                    .bottom_0()
                                    .left_0()
                                    .right_0()
                                    .h(px(8.))
                                    .drag_over::<DraggedFolder>(move |style, drag, _, cx| {
                                        if disallowed_bot.contains(&drag.id) {
                                            style
                                        } else {
                                            drag_line_bottom(style, cx)
                                        }
                                    })
                                    .on_drop(cx.listener(
                                        move |view, drag: &DraggedFolder, window, cx| {
                                            view.move_folder_relative(
                                                &drag.id, &id_bot, true, window, cx,
                                            );
                                        },
                                    )),
                            )
                        })
                        .context_menu(move |menu, window, cx| {
                            let id1 = id_menu.clone();
                            let id2 = id_menu.clone();
                            let id3 = id_menu.clone();
                            let id4 = id_menu.clone();
                            let id_up = id_menu.clone();
                            let id_down = id_menu.clone();
                            let id_sub = id_menu.clone();
                            let id_out = id_menu.clone();
                            let v1 = view_ctx.clone();
                            let v2 = view_ctx.clone();
                            let v3 = view_ctx.clone();
                            let v4 = view_ctx.clone();
                            let v_up = view_ctx.clone();
                            let v_down = view_ctx.clone();
                            let v_sub = view_ctx.clone();
                            let v_out = view_ctx.clone();
                            let current_parent = parent_folder.clone();
                            let is_nested = current_parent.is_some();
                            let targets = valid_folder_targets.clone();

                            let menu = menu
                                .item(
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
                                    PopupMenuItem::new("Move Up")
                                        .icon(svg_icon("icons/arrow-up.svg"))
                                        .disabled(!can_move_up)
                                        .on_click(move |_, window, cx| {
                                            let _ = v_up.update(cx, |view, cx| {
                                                view.move_folder_offset(&id_up, -1, window, cx);
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Move Down")
                                        .icon(svg_icon("icons/arrow-down.svg"))
                                        .disabled(!can_move_down)
                                        .on_click(move |_, window, cx| {
                                            let _ = v_down.update(cx, |view, cx| {
                                                view.move_folder_offset(&id_down, 1, window, cx);
                                            });
                                        }),
                                )
                                .submenu_with_icon(
                                    Some(svg_icon("icons/folder-input.svg")),
                                    "Move to",
                                    window,
                                    cx,
                                    move |sub, _, _| {
                                        let v_sub = v_sub.clone();
                                        let id_sub = id_sub.clone();
                                        populate_move_to_menu(
                                            sub,
                                            current_parent.as_deref(),
                                            &targets,
                                            move |dest, window, cx| {
                                                let _ = v_sub.update(cx, |view, cx| {
                                                    view.move_folder_to_folder(
                                                        &id_sub, dest, window, cx,
                                                    );
                                                });
                                            },
                                        )
                                    },
                                );

                            let menu = if is_nested {
                                menu.item(
                                    PopupMenuItem::new("Move Out to Root")
                                        .icon(svg_icon("icons/folder-output.svg"))
                                        .on_click(move |_, window, cx| {
                                            let _ = v_out.update(cx, |view, cx| {
                                                view.move_folder_to_folder(
                                                    &id_out, None, window, cx,
                                                );
                                            });
                                        }),
                                )
                            } else {
                                menu
                            };

                            menu.separator().item(
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
            let mut next_ancestors = ancestors.to_vec();
            next_ancestors.push(id);
            self.render_tree_children(
                &folder.folders,
                &folder.requests,
                depth + 1.,
                &next_ancestors,
                cx,
                out,
            );
        }
    }

    fn render_request_link(
        &self,
        request: &ApiRequest,
        depth: f32,
        ancestors: &[String],
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
    ) {
        let id = request.id.clone();
        let selected = self.selected_id.as_ref() == Some(&id);
        let method = request.method.clone();
        let title = request.name.clone();
        let (req_idx, req_len) = self.current().request_order_bounds(&id).unwrap_or((0, 1));
        let can_move_up = req_idx > 0;
        let can_move_down = req_idx + 1 < req_len;
        let parent_folder = self.current().request_parent_folder(&id);
        let folder_targets = self.current().folder_targets();
        let disallowed_folder_ids = ancestors.to_vec();
        let drop_target_parent = parent_folder.clone();
        let show_request_drop_zones = cx.has_active_drag();
        let drag_payload = DraggedRequest {
            id: id.clone(),
            method: method.clone(),
            name: title.clone(),
        };
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
                        .on_drag(drag_payload, |drag, _, _, cx| {
                            cx.new(|_| DraggedRequestPreview(drag.clone()))
                        })
                        .drag_over::<DraggedFolder>(move |style, drag, _, cx| {
                            if disallowed_folder_ids.contains(&drag.id) {
                                style
                            } else {
                                drag_highlight_box(style, cx)
                            }
                        })
                        .on_drop(cx.listener({
                            let target_parent = drop_target_parent.clone();
                            move |view, drag: &DraggedFolder, window, cx| {
                                view.move_folder_to_folder(
                                    &drag.id,
                                    target_parent.as_deref(),
                                    window,
                                    cx,
                                );
                            }
                        }))
                        .when(show_request_drop_zones, |el| {
                            let row_id_top = id.clone();
                            let row_id_bot = id.clone();
                            let target_top = id.clone();
                            let target_bot = id.clone();
                            el.child(
                                div()
                                    .id(format!("request-drop-before-{id}"))
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .right_0()
                                    .h(px(15.))
                                    .drag_over::<DraggedRequest>(move |style, drag, _, cx| {
                                        if drag.id == row_id_top {
                                            style
                                        } else {
                                            drag_line_top(style, cx)
                                        }
                                    })
                                    .on_drop(cx.listener(
                                        move |view, drag: &DraggedRequest, window, cx| {
                                            view.move_request_relative(
                                                &drag.id,
                                                &target_top,
                                                false,
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id(format!("request-drop-after-{id}"))
                                    .absolute()
                                    .bottom_0()
                                    .left_0()
                                    .right_0()
                                    .h(px(15.))
                                    .drag_over::<DraggedRequest>(move |style, drag, _, cx| {
                                        if drag.id == row_id_bot {
                                            style
                                        } else {
                                            drag_line_bottom(style, cx)
                                        }
                                    })
                                    .on_drop(cx.listener(
                                        move |view, drag: &DraggedRequest, window, cx| {
                                            view.move_request_relative(
                                                &drag.id,
                                                &target_bot,
                                                true,
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                            )
                        })
                        .context_menu(move |menu, window, cx| {
                            let id1 = id_menu.clone();
                            let id2 = id_menu.clone();
                            let id3 = id_menu.clone();
                            let id3_full = id_menu.clone();
                            let id4 = id_menu.clone();
                            let id_up = id_menu.clone();
                            let id_down = id_menu.clone();
                            let id_sub = id_menu.clone();
                            let id_out = id_menu.clone();
                            let v1 = view_ctx.clone();
                            let v2 = view_ctx.clone();
                            let v3 = view_ctx.clone();
                            let v3_full = view_ctx.clone();
                            let v4 = view_ctx.clone();
                            let v_up = view_ctx.clone();
                            let v_down = view_ctx.clone();
                            let v_sub = view_ctx.clone();
                            let v_out = view_ctx.clone();
                            let current_parent = parent_folder.clone();
                            let in_folder = current_parent.is_some();
                            let targets = folder_targets.clone();

                            let menu = menu
                                .item(
                                    PopupMenuItem::new("Rename")
                                        .icon(svg_icon("icons/pencil.svg"))
                                        .on_click(move |_, window, cx| {
                                            let _ = v1.update(cx, |view, cx| {
                                                view.open_request(id1.clone(), window, cx);
                                                view.editing_request_name = true;
                                                view.request_name
                                                    .read(cx)
                                                    .focus_handle(cx)
                                                    .focus(window, cx);
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
                                    PopupMenuItem::new("Copy as cURL (Redacted)")
                                        .icon(svg_icon("icons/terminal.svg"))
                                        .on_click(move |_, window, cx| {
                                            let _ = v3.update(cx, |view, cx| {
                                                view.copy_request_as_curl(
                                                    Some(&id3),
                                                    false,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Copy Full cURL (With Secrets)...")
                                        .icon(svg_icon("icons/triangle-alert.svg"))
                                        .on_click(move |_, window, cx| {
                                            let _ = v3_full.update(cx, |view, cx| {
                                                view.copy_request_as_curl(
                                                    Some(&id3_full),
                                                    true,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Move Up")
                                        .icon(svg_icon("icons/arrow-up.svg"))
                                        .disabled(!can_move_up)
                                        .on_click(move |_, window, cx| {
                                            let _ = v_up.update(cx, |view, cx| {
                                                view.move_request_offset(&id_up, -1, window, cx);
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Move Down")
                                        .icon(svg_icon("icons/arrow-down.svg"))
                                        .disabled(!can_move_down)
                                        .on_click(move |_, window, cx| {
                                            let _ = v_down.update(cx, |view, cx| {
                                                view.move_request_offset(&id_down, 1, window, cx);
                                            });
                                        }),
                                )
                                .submenu_with_icon(
                                    Some(svg_icon("icons/folder-input.svg")),
                                    "Move to",
                                    window,
                                    cx,
                                    move |sub, _, _| {
                                        let v_sub = v_sub.clone();
                                        let id_sub = id_sub.clone();
                                        populate_move_to_menu(
                                            sub,
                                            current_parent.as_deref(),
                                            &targets,
                                            move |dest, window, cx| {
                                                let _ = v_sub.update(cx, |view, cx| {
                                                    view.move_request_to_folder(
                                                        &id_sub, dest, window, cx,
                                                    );
                                                });
                                            },
                                        )
                                    },
                                );

                            let menu = if in_folder {
                                menu.item(
                                    PopupMenuItem::new("Move Out of Folder")
                                        .icon(svg_icon("icons/folder-output.svg"))
                                        .on_click(move |_, window, cx| {
                                            let _ = v_out.update(cx, |view, cx| {
                                                view.move_request_to_folder(
                                                    &id_out, None, window, cx,
                                                );
                                            });
                                        }),
                                )
                            } else {
                                menu
                            };

                            menu.separator().item(
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

fn populate_move_to_menu(
    mut sub: gpui_kit::component::menu::PopupMenu,
    current_parent: Option<&str>,
    targets: &[(String, String)],
    on_move: impl Fn(Option<&str>, &mut Window, &mut App) + Clone + 'static,
) -> gpui_kit::component::menu::PopupMenu {
    let at_root = current_parent.is_none();
    let on_root = on_move.clone();
    sub = sub.scrollable(true).item(
        PopupMenuItem::new("Root")
            .icon(svg_icon("icons/folder-root.svg"))
            .checked(at_root)
            .disabled(at_root)
            .on_click(move |_, window, cx| {
                on_root(None, window, cx);
            }),
    );
    if !targets.is_empty() {
        sub = sub.separator();
    }
    for (target_id, target_path) in targets {
        let is_current = current_parent == Some(target_id.as_str());
        let dest_id = target_id.clone();
        let on_dest = on_move.clone();
        sub = sub.item(
            PopupMenuItem::new(target_path.clone())
                .icon(svg_icon("icons/folder-closed.svg"))
                .checked(is_current)
                .disabled(is_current)
                .on_click(move |_, window, cx| {
                    on_dest(Some(&dest_id), window, cx);
                }),
        );
    }
    sub
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
