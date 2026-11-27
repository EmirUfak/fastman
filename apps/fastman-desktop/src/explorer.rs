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
use std::sync::Arc;

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

#[derive(PartialEq, Eq)]
pub(super) struct ExplorerCacheKey {
    project_id: String,
    edit_rev: u64,
    epoch: u64,
    filter: String,
    selected_id: Option<String>,
    target_folder: Option<String>,
    expanded_len: usize,
}

#[derive(Clone)]
pub(super) struct ExplorerRow {
    kind: ExplorerRowKind,
    id: String,
    depth: f32,
    ancestors: Vec<String>,
    parent_folder: Option<String>,
    bounds: (usize, usize),
    count: usize,
    label: String,
    method: String,
    selected: bool,
    is_target: bool,
    is_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplorerRowKind {
    Collection,
    Folder,
    Request,
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

fn view_listener<E>(
    view: &WeakEntity<WorkspaceView>,
    listener: impl Fn(&mut WorkspaceView, &E, &mut Window, &mut Context<WorkspaceView>) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |event, window, cx| {
        let _ = view.update(cx, |view, cx| listener(view, event, window, cx));
    }
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
        let rows = self.cached_explorer_rows();
        let has_matches = !rows.is_empty();
        let weak_view = cx.weak_entity();
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
            .when(has_matches, |el| {
                let rows = Arc::clone(&rows);
                let weak_view = weak_view.clone();
                let item_count = rows.len();
                let empty_space_view = cx.weak_entity();
                el.child(
                    gpui::uniform_list("explorer-list", item_count, move |range, window, cx| {
                        range
                            .map(|index| render_explorer_row(&rows[index], &weak_view, window, cx))
                            .collect()
                    })
                    .track_scroll(&self.explorer_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full(),
                )
                .child(
                    div()
                        .id("explorer-root-drop")
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
                        .drag_over::<DraggedFolder>(|style, _, _, cx| drag_highlight_box(style, cx))
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
                                    .description(EmptyDescription::new().child(if empty_filter {
                                        "Add a request or folder above, or right-click here."
                                    } else {
                                        "Try another search term or add a request."
                                    })),
                            ),
                        ),
                )
            })
            .into_any_element()
    }

    pub(super) fn scroll_explorer_to_id(&self, target_id: &str) {
        if let Some(index) = self
            .cached_explorer_rows()
            .iter()
            .position(|row| row.id == target_id)
        {
            self.explorer_scroll
                .scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    pub(super) fn cached_explorer_rows(&self) -> Arc<Vec<ExplorerRow>> {
        let project_id = self.current().id.clone();
        let key = ExplorerCacheKey {
            edit_rev: self.current_edit_rev(&project_id),
            project_id,
            epoch: self.explorer_epoch,
            filter: self.explorer_filter_lower.clone(),
            selected_id: self.selected_id.clone(),
            target_folder: self.target_folder.clone(),
            expanded_len: self.expanded.len(),
        };
        if let Some((cached_key, cached_rows)) = self.explorer_cache.borrow().as_ref()
            && *cached_key == key
        {
            return Arc::clone(cached_rows);
        }
        let rows = Arc::new(self.explorer_rows());
        *self.explorer_cache.borrow_mut() = Some((key, Arc::clone(&rows)));
        rows
    }

    fn explorer_rows(&self) -> Vec<ExplorerRow> {
        let query = self.explorer_filter_lower.as_str();
        let collections = &self.current().collections;
        let mut rows = Vec::new();
        let mut ancestors = Vec::new();

        if collections.len() <= 1 {
            if let Some(collection) = collections.first() {
                self.push_tree_rows(
                    &collection.folders,
                    &collection.requests,
                    0.,
                    &mut ancestors,
                    query,
                    &mut rows,
                );
            }
            return rows;
        }

        for (index, collection) in collections.iter().enumerate() {
            let open = !query.is_empty() || self.expanded.contains(&collection.id);
            let row_index = rows.len();
            rows.push(ExplorerRow {
                kind: ExplorerRowKind::Collection,
                id: collection.id.clone(),
                depth: 0.,
                ancestors: Vec::new(),
                parent_folder: None,
                bounds: (index, collections.len()),
                count: 0,
                label: collection.name.clone(),
                method: String::new(),
                selected: false,
                is_target: false,
                is_open: open,
            });
            if open {
                let count = self.push_tree_rows(
                    &collection.folders,
                    &collection.requests,
                    1.,
                    &mut ancestors,
                    query,
                    &mut rows,
                );
                let has_child_match = rows.len() > row_index + 1;
                if !query.is_empty()
                    && !has_child_match
                    && !contains_case_insensitive(&collection.name, query)
                {
                    rows.pop();
                } else {
                    rows[row_index].count = count;
                }
            } else {
                rows[row_index].count = collection.request_count();
            }
        }
        rows
    }

    fn push_tree_rows(
        &self,
        folders: &[Folder],
        requests: &[ApiRequest],
        depth: f32,
        ancestors: &mut Vec<String>,
        query: &str,
        rows: &mut Vec<ExplorerRow>,
    ) -> usize {
        let mut total_requests = requests.len();
        for (index, folder) in folders.iter().enumerate() {
            if depth as usize > fastman_core::model::MAX_FOLDER_DEPTH {
                total_requests += folder.request_count();
                continue;
            }
            let open = !query.is_empty() || self.expanded.contains(&folder.id);
            let row_index = rows.len();
            rows.push(ExplorerRow {
                kind: ExplorerRowKind::Folder,
                id: folder.id.clone(),
                depth,
                ancestors: ancestors.clone(),
                parent_folder: ancestors.last().cloned(),
                bounds: (index, folders.len()),
                count: 0,
                label: folder.name.clone(),
                method: String::new(),
                selected: false,
                is_target: self.target_folder.as_ref() == Some(&folder.id),
                is_open: open,
            });
            if open {
                ancestors.push(folder.id.clone());
                let folder_count = self.push_tree_rows(
                    &folder.folders,
                    &folder.requests,
                    depth + 1.,
                    ancestors,
                    query,
                    rows,
                );
                ancestors.pop();
                total_requests += folder_count;
                let has_child_match = rows.len() > row_index + 1;
                if !query.is_empty()
                    && !has_child_match
                    && !contains_case_insensitive(&folder.name, query)
                {
                    rows.pop();
                } else {
                    rows[row_index].count = folder_count;
                }
            } else {
                let folder_count = folder.request_count();
                total_requests += folder_count;
                rows[row_index].count = folder_count;
            }
        }
        let parent_folder = ancestors.last().cloned();
        for (index, request) in requests.iter().enumerate() {
            if !query.is_empty() && !request_matches(request, query) {
                continue;
            }
            rows.push(ExplorerRow {
                kind: ExplorerRowKind::Request,
                id: request.id.clone(),
                depth,
                ancestors: ancestors.clone(),
                parent_folder: parent_folder.clone(),
                bounds: (index, requests.len()),
                count: 0,
                label: request.name.clone(),
                method: request.method.clone(),
                selected: self.selected_id.as_ref() == Some(&request.id),
                is_target: false,
                is_open: false,
            });
        }
        total_requests
    }
}

fn render_explorer_row(
    row: &ExplorerRow,
    view: &WeakEntity<WorkspaceView>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match row.kind {
        ExplorerRowKind::Collection => render_collection_row(row, view, cx),
        ExplorerRowKind::Folder => render_folder_row(row, view, window, cx),
        ExplorerRowKind::Request => render_request_row(row, view, window, cx),
    }
}

fn render_collection_row(
    row: &ExplorerRow,
    view: &WeakEntity<WorkspaceView>,
    cx: &mut App,
) -> AnyElement {
    let id = row.id.clone();
    let open = row.is_open;
    let can_move_up = row.bounds.0 > 0;
    let can_move_down = row.bounds.0 + 1 < row.bounds.1;
    let view_ctx = view.clone();
    let id_menu = id.clone();

    div()
        .id(format!("collection-row-{id}"))
        .w_full()
        .h(px(30.))
        .px_1p5()
        .rounded(cx.theme().radius)
        .flex()
        .items_center()
        .gap_1p5()
        .cursor_pointer()
        .hover(|s| s.bg(cx.theme().list_hover))
        .on_click(view_listener(view, {
            let id = id.clone();
            move |view, _, _, cx| {
                if !view.expanded.insert(id.clone()) {
                    view.expanded.remove(&id);
                }
                view.bump_explorer_epoch();
                view.target_folder = None;
                cx.notify();
            }
        }))
        .drag_over::<DraggedRequest>(|style, _, _, cx| drag_highlight_box(style, cx))
        .on_drop(view_listener(view, {
            let id = id.clone();
            move |view, drag: &DraggedRequest, window, cx| {
                view.move_request_to_collection(&drag.id, &id, window, cx);
            }
        }))
        .drag_over::<DraggedFolder>(|style, _, _, cx| drag_highlight_box(style, cx))
        .on_drop(view_listener(view, {
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
                .child(row.label.clone()),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(cx.theme().muted_foreground)
                .child(row.count.to_string()),
        )
        .into_any_element()
}

fn render_folder_row(
    row: &ExplorerRow,
    view: &WeakEntity<WorkspaceView>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = row.id.clone();
    let open = row.is_open;
    let is_target = row.is_target;
    let label = row.label.clone();
    let can_move_up = row.bounds.0 > 0;
    let can_move_down = row.bounds.0 + 1 < row.bounds.1;
    let parent_folder = row.parent_folder.clone();
    let mut disallowed_drag_ids = row.ancestors.clone();
    disallowed_drag_ids.push(id.clone());
    let show_folder_drop_zones = cx.has_active_drag();
    let drag_payload = DraggedFolder {
        id: id.clone(),
        name: label.clone(),
    };
    let view_ctx = view.clone();
    let id_menu = id.clone();

    div()
        .pl(px(row.depth * 12.))
        .child(
            div()
                .id(format!("folder-row-{id}"))
                .relative()
                .w_full()
                .h(px(30.))
                .px_1p5()
                .rounded(cx.theme().radius)
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .when(is_target, |el| el.bg(cx.theme().accent))
                .hover(|s| s.bg(cx.theme().list_hover))
                .on_click(view_listener(view, {
                    let id = id.clone();
                    move |view, _, _, cx| {
                        if !view.expanded.insert(id.clone()) {
                            view.expanded.remove(&id);
                        }
                        view.bump_explorer_epoch();
                        view.target_folder = Some(id.clone());
                        cx.notify();
                    }
                }))
                .on_drag(drag_payload, |drag, _, _, cx| {
                    cx.new(|_| DraggedFolderPreview(drag.clone()))
                })
                .drag_over::<DraggedRequest>(|style, _, _, cx| drag_highlight_box(style, cx))
                .on_drop(view_listener(view, {
                    let id = id.clone();
                    move |view, drag: &DraggedRequest, window, cx| {
                        view.move_request_to_folder(&drag.id, Some(&id), window, cx);
                    }
                }))
                .when(show_folder_drop_zones, |el| {
                    let disallowed_top = disallowed_drag_ids.clone();
                    let disallowed_mid = disallowed_drag_ids.clone();
                    let disallowed_bot = disallowed_drag_ids;
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
                            .on_drop(view_listener(
                                view,
                                move |view, drag: &DraggedFolder, window, cx| {
                                    view.move_folder_relative(&drag.id, &id_top, false, window, cx);
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
                            .on_drop(view_listener(
                                view,
                                move |view, drag: &DraggedFolder, window, cx| {
                                    view.move_folder_to_folder(&drag.id, Some(&id_mid), window, cx);
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
                            .on_drop(view_listener(
                                view,
                                move |view, drag: &DraggedFolder, window, cx| {
                                    view.move_folder_relative(&drag.id, &id_bot, true, window, cx);
                                },
                            )),
                    )
                })
                .context_menu(move |menu, window, cx| {
                    let targets = view_ctx
                        .read_with(cx, |view, _| {
                            let project = view.current();
                            project
                                .folder_targets()
                                .into_iter()
                                .filter(|(folder_id, _)| {
                                    folder_id != &id_menu
                                        && !project.is_folder_descendant(&id_menu, folder_id)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
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
                                            view.move_folder_to_folder(&id_sub, dest, window, cx);
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
                                        view.move_folder_to_folder(&id_out, None, window, cx);
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
                            .on_click(view_listener(view, move |view, _, window, cx| {
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
                            .child(row.count.to_string()),
                    )
                }),
        )
        .into_any_element()
}

fn render_request_row(
    row: &ExplorerRow,
    view: &WeakEntity<WorkspaceView>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = row.id.clone();
    let selected = row.selected;
    let method = row.method.clone();
    let title = row.label.clone();
    let can_move_up = row.bounds.0 > 0;
    let can_move_down = row.bounds.0 + 1 < row.bounds.1;
    let parent_folder = row.parent_folder.clone();
    let disallowed_folder_ids = row.ancestors.clone();
    let show_request_drop_zones = cx.has_active_drag();
    let drag_payload = DraggedRequest {
        id: id.clone(),
        method: method.clone(),
        name: title.clone(),
    };
    let view_ctx = view.clone();
    let id_menu = id.clone();

    div()
        .pl(px(row.depth * 12.))
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
                .on_click(view_listener(view, {
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
                .on_drop(view_listener(view, {
                    let target_parent = parent_folder.clone();
                    move |view, drag: &DraggedFolder, window, cx| {
                        view.move_folder_to_folder(&drag.id, target_parent.as_deref(), window, cx);
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
                            .on_drop(view_listener(
                                view,
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
                            .on_drop(view_listener(
                                view,
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
                    let targets = view_ctx
                        .read_with(cx, |view, _| view.current().folder_targets())
                        .unwrap_or_default();
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
                                        view.copy_request_as_curl(Some(&id3), false, window, cx);
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
                                            view.move_request_to_folder(&id_sub, dest, window, cx);
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
                                        view.move_request_to_folder(&id_out, None, window, cx);
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
        .into_any_element()
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

fn contains_case_insensitive(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if needle_lower.is_ascii() {
        let needle = needle_lower.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle));
    }
    haystack.to_lowercase().contains(needle_lower)
}

fn request_matches(request: &ApiRequest, query: &str) -> bool {
    contains_case_insensitive(&request.name, query)
        || contains_case_insensitive(&request.method, query)
        || contains_case_insensitive(&request.url, query)
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceView, request_matches};
    use fastman_core::{ApiRequest, Project, storage::ProjectStore};
    use gpui::{AnyWindowHandle, Entity, TestAppContext};
    use gpui_kit::AppContext as _;
    use std::time::{Duration, Instant};

    fn install_project(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<WorkspaceView>,
        project: Project,
    ) {
        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, _| {
                view.projects = vec![project];
                view.active_project = 0;
                view.expanded.clear();
            });
        })
        .unwrap();
    }

    fn filter_scan(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<WorkspaceView>,
    ) -> (Duration, usize) {
        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, _| {
                let started = Instant::now();
                let hits = view.current().collections[0]
                    .requests
                    .iter()
                    .filter(|request| request_matches(request, "needle"))
                    .count();
                (started.elapsed(), hits)
            })
        })
        .unwrap()
    }

    fn prepare_tree(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<WorkspaceView>,
    ) {
        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, _| {
                drop(view.explorer_rows());
            });
        })
        .unwrap();
    }

    fn render_visible_rows(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<WorkspaceView>,
    ) {
        cx.update_window(window, |_, window, cx| {
            let weak = view.downgrade();
            let rows = view.read(cx).explorer_rows();
            for row in rows.iter().take(40) {
                drop(super::render_explorer_row(row, &weak, window, cx));
            }
        })
        .unwrap();
    }

    #[test]
    fn explorer_rows_preserve_order_depth_filter_and_selection() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store, window, cx).unwrap())
                })
            })
            .unwrap();

        let mut project = Project::new("Tree");
        let folder = fastman_core::Folder::new("Folder");
        let folder_id = folder.id.clone();
        let nested = ApiRequest::new("GET", "Needle request", "https://example.com/nested");
        let nested_id = nested.id.clone();
        let mut folder = folder;
        folder.requests.push(nested);
        let root_request = ApiRequest::new("POST", "Other request", "https://example.com/root");
        let root_id = root_request.id.clone();
        project.collections[0].folders.push(folder);
        project.collections[0].requests.push(root_request);
        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, _| {
                view.projects = vec![project];
                view.active_project = 0;
                view.expanded.insert(folder_id.clone());
                view.selected_id = Some(nested_id.clone());
                view.target_folder = Some(folder_id.clone());
            });
        })
        .unwrap();

        let rows = cx
            .update_window(window, |_, _, cx| {
                view.update(cx, |view, _| view.explorer_rows())
            })
            .unwrap();
        let kinds: Vec<_> = rows
            .iter()
            .map(|row| (row.kind, row.id.as_str(), row.depth))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (super::ExplorerRowKind::Folder, folder_id.as_str(), 0.),
                (super::ExplorerRowKind::Request, nested_id.as_str(), 1.),
                (super::ExplorerRowKind::Request, root_id.as_str(), 0.),
            ]
        );
        assert!(rows[0].is_target);
        assert_eq!(rows[1].parent_folder.as_deref(), Some(folder_id.as_str()));
        assert_eq!(rows[1].bounds, (0, 1));
        assert!(rows[1].selected);
        assert!(rows[2].parent_folder.is_none());

        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, _| {
                view.explorer_filter_lower = "needle".into();
            });
        })
        .unwrap();
        let filtered = cx
            .update_window(window, |_, _, cx| {
                view.update(cx, |view, _| view.explorer_rows())
            })
            .unwrap();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].count, 1);
        assert_eq!(filtered[1].id, nested_id);

        // Matching a folder name directly keeps the folder row (with its total request count)
        // even when none of its child requests match the query.
        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, _| {
                view.explorer_filter_lower = "folder".into();
            });
        })
        .unwrap();
        let folder_only = cx
            .update_window(window, |_, _, cx| {
                view.update(cx, |view, _| view.explorer_rows())
            })
            .unwrap();
        assert_eq!(folder_only.len(), 1);
        assert_eq!(folder_only[0].id, folder_id);
        assert_eq!(folder_only[0].count, 1);

        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, _| {
                view.explorer_filter_lower.clear();
                view.expanded.clear();
            });
        })
        .unwrap();
        let collapsed = cx
            .update_window(window, |_, _, cx| {
                view.update(cx, |view, _| view.explorer_rows())
            })
            .unwrap();
        assert_eq!(collapsed.len(), 2);
        assert_eq!(collapsed[0].kind, super::ExplorerRowKind::Folder);
        assert_eq!(collapsed[1].id, root_id);
    }

    #[test]
    #[ignore = "scale benchmark: run in release mode with --ignored --nocapture"]
    fn explorer_filter_and_tree_preparation_baseline() {
        run_benchmark();
    }

    fn run_benchmark() {
        const ITERATIONS: u32 = 3;
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store, window, cx).unwrap())
                })
            })
            .unwrap();

        for size in [100, 1_000, 10_000] {
            let mut project = Project::new("Explorer baseline");
            project.collections[0].requests = (0..size)
                .map(|i| {
                    let name = if i % 10 == 0 {
                        format!("request {i} needle")
                    } else {
                        format!("request {i}")
                    };
                    ApiRequest::new("GET", name, format!("https://example.com/{i}"))
                })
                .collect();
            install_project(&mut cx, window, &view, project);
            println!("\n{size} requests");

            let (elapsed, hits) = filter_scan(&mut cx, window, &view);
            println!("filter scan: {elapsed:?}, {hits} matches");

            prepare_tree(&mut cx, window, &view);
            let started = Instant::now();
            for _ in 0..ITERATIONS {
                prepare_tree(&mut cx, window, &view);
            }
            let metadata = started.elapsed() / ITERATIONS;

            render_visible_rows(&mut cx, window, &view);
            let started = Instant::now();
            for _ in 0..ITERATIONS {
                render_visible_rows(&mut cx, window, &view);
            }
            let visible = started.elapsed() / ITERATIONS;
            println!("row metadata: {metadata:?}, 40 visible rows: {visible:?}");
        }
    }
}
