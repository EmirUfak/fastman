use crate::response_view::ResponsePosition;
use crate::theme::{method_badge, svg_icon};
use crate::workspace::{NameDialogKind, WorkspaceView};
use fastman_core::{Collection, Folder};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    kbd::Kbd,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PaletteSection {
    Requests,
    Folders,
    Projects,
    Workspaces,
    Actions,
}

impl PaletteSection {
    const ALL: [Self; 5] = [
        Self::Requests,
        Self::Folders,
        Self::Projects,
        Self::Workspaces,
        Self::Actions,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Requests => "Requests",
            Self::Folders => "Folders",
            Self::Projects => "Projects",
            Self::Workspaces => "Workspaces",
            Self::Actions => "Actions",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PaletteAction {
    SendRequest,
    SaveRequest,
    NewRequest,
    NewFolder,
    DuplicateRequest,
    RenameRequest,
    CloseActiveTab,
    NewProject,
    NewWorkspace,
    FocusUrl,
    FocusSearch,
    OpenVariables,
    OpenImport,
    ToggleTheme,
    CycleResponsePosition,
}

impl PaletteAction {
    fn label(self) -> &'static str {
        match self {
            Self::SendRequest => "Send Request",
            Self::SaveRequest => "Save Request",
            Self::NewRequest => "New Request",
            Self::NewFolder => "New Folder",
            Self::DuplicateRequest => "Duplicate Selected Request",
            Self::RenameRequest => "Rename Selected Request",
            Self::CloseActiveTab => "Close Active Tab",
            Self::NewProject => "New Project...",
            Self::NewWorkspace => "New Workspace...",
            Self::FocusUrl => "Focus URL Bar",
            Self::FocusSearch => "Search Explorer",
            Self::OpenVariables => "Edit Project Variables",
            Self::OpenImport => "Import Collection or cURL",
            Self::ToggleTheme => "Toggle Theme (Light / Dark)",
            Self::CycleResponsePosition => "Cycle Response Layout",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::SendRequest => "Execute the active HTTP request",
            Self::SaveRequest => "Persist draft changes to local project JSON",
            Self::NewRequest => "Create an HTTP request in the active folder or root",
            Self::NewFolder => "Create a folder inside the current collection",
            Self::DuplicateRequest => "Clone the active request into the same folder",
            Self::RenameRequest => "Edit the active request title inline",
            Self::CloseActiveTab => "Close the currently active request tab",
            Self::NewProject => "Create a project in the active workspace",
            Self::NewWorkspace => "Create a new isolated local workspace",
            Self::FocusUrl => "Jump cursor to the request URL bar",
            Self::FocusSearch => "Filter requests and folders in the sidebar",
            Self::OpenVariables => "Inspect or update {{template}} variables",
            Self::OpenImport => "Import Collection v2.1 JSON or paste cURL",
            Self::ToggleTheme => "Switch between Dark and Light appearance",
            Self::CycleResponsePosition => "Rotate response pane (Right / Bottom / Left)",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::SendRequest => "icons/zap.svg",
            Self::SaveRequest => "icons/save.svg",
            Self::NewRequest => "icons/file-plus.svg",
            Self::NewFolder => "icons/folder-plus.svg",
            Self::DuplicateRequest => "icons/copy.svg",
            Self::RenameRequest => "icons/pencil.svg",
            Self::CloseActiveTab => "icons/x.svg",
            Self::NewProject => "icons/plus.svg",
            Self::NewWorkspace => "icons/layers.svg",
            Self::FocusUrl => "icons/globe.svg",
            Self::FocusSearch => "icons/search.svg",
            Self::OpenVariables => "icons/variable.svg",
            Self::OpenImport => "icons/import.svg",
            Self::ToggleTheme => "icons/sun.svg",
            Self::CycleResponsePosition => "icons/panel-right.svg",
        }
    }

    fn keywords(self) -> &'static str {
        match self {
            Self::SendRequest => "send execute run request http",
            Self::SaveRequest => "save persist write project",
            Self::NewRequest => "new create add request http",
            Self::NewFolder => "new create folder group",
            Self::DuplicateRequest => "duplicate clone copy request",
            Self::RenameRequest => "rename edit title request",
            Self::CloseActiveTab => "close tab dismiss",
            Self::NewProject => "new create project",
            Self::NewWorkspace => "new create workspace",
            Self::FocusUrl => "focus url address bar input",
            Self::FocusSearch => "search filter explorer sidebar",
            Self::OpenVariables => "variables template env edit secrets",
            Self::OpenImport => "import curl postman collection json",
            Self::ToggleTheme => "theme dark light appearance mode",
            Self::CycleResponsePosition => "response layout split bottom left right panel",
        }
    }

    fn shortcut_kbd(self, window: &Window) -> Option<Kbd> {
        match self {
            Self::SendRequest => Kbd::global_binding_for_action(&crate::SendRequest, window),
            Self::SaveRequest => Kbd::global_binding_for_action(&crate::SaveRequest, window),
            Self::NewRequest => Kbd::global_binding_for_action(&crate::NewRequest, window),
            Self::CloseActiveTab => Kbd::global_binding_for_action(&crate::CloseActiveTab, window),
            Self::FocusUrl => Kbd::global_binding_for_action(&crate::FocusUrl, window),
            Self::FocusSearch => Kbd::global_binding_for_action(&crate::FocusSearch, window),
            Self::ToggleTheme => Kbd::global_binding_for_action(&crate::ToggleTheme, window),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PaletteTarget {
    Request {
        id: String,
    },
    Folder {
        id: String,
        ancestor_ids: Vec<String>,
    },
    Project {
        index: usize,
    },
    Workspace {
        index: usize,
    },
    Action(PaletteAction),
}

#[derive(Clone)]
pub(super) struct PaletteEntry {
    pub(super) section: PaletteSection,
    pub(super) title: SharedString,
    pub(super) subtitle: Option<SharedString>,
    pub(super) method: Option<SharedString>,
    pub(super) icon: &'static str,
    pub(super) active: bool,
    pub(super) search_text: String,
    pub(super) target: PaletteTarget,
}

pub(super) fn fuzzy_score(query: &str, haystack: &str) -> Option<i32> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Some(0);
    }

    let haystack_orig: Vec<char> = haystack.chars().collect();
    let haystack_lower: String = haystack.to_lowercase();
    let haystack_lower_chars: Vec<char> = haystack_lower.chars().collect();
    let mut total_score = 0;

    for token in trimmed.split_whitespace() {
        let token_lower = token.to_lowercase();
        if let Some(byte_idx) = haystack_lower.find(&token_lower) {
            let char_idx = haystack_lower[..byte_idx].chars().count();
            let mut score = 250 - (char_idx.min(180) as i32);
            if char_idx == 0 {
                score += 100;
            } else if is_word_boundary(
                haystack_orig.get(char_idx.wrapping_sub(1)).copied(),
                haystack_orig[char_idx],
            ) {
                score += 60;
            }
            total_score += score;
            continue;
        }

        let token_chars: Vec<char> = token_lower.chars().collect();
        let mut h_idx = 0usize;
        let mut prev_match: Option<usize> = None;
        let mut sub_score = 0i32;

        for tc in token_chars {
            let mut found = None;
            while h_idx < haystack_lower_chars.len() {
                if haystack_lower_chars[h_idx] == tc {
                    found = Some(h_idx);
                    h_idx += 1;
                    break;
                }
                h_idx += 1;
            }
            let idx = found?;
            sub_score += 10;
            if idx == 0 {
                sub_score += 30;
            } else {
                if is_word_boundary(Some(haystack_orig[idx - 1]), haystack_orig[idx]) {
                    sub_score += 25;
                }
                if let Some(prev) = prev_match {
                    if idx == prev + 1 {
                        sub_score += 18;
                    } else {
                        sub_score -= (idx - prev - 1).min(8) as i32;
                    }
                }
            }
            prev_match = Some(idx);
        }
        total_score += sub_score;
    }

    Some(total_score)
}

fn is_word_boundary(prev: Option<char>, current: char) -> bool {
    match prev {
        None => true,
        Some(p) if !p.is_alphanumeric() => true,
        Some(p) => p.is_lowercase() && current.is_uppercase(),
    }
}

fn collect_collection_entries(
    collection: &Collection,
    show_collection_prefix: bool,
    selected_id: Option<&str>,
    open_tabs: &[String],
    out: &mut Vec<PaletteEntry>,
) {
    let base_path: Vec<String> = if show_collection_prefix {
        vec![collection.name.clone()]
    } else {
        Vec::new()
    };

    for req in &collection.requests {
        let mut subtitle_parts = base_path.clone();
        if !req.url.trim().is_empty() {
            subtitle_parts.push(req.url.clone());
        }
        let subtitle = if subtitle_parts.is_empty() {
            None
        } else {
            Some(SharedString::from(subtitle_parts.join(" · ")))
        };
        let active = selected_id == Some(req.id.as_str());
        let open_hint = if open_tabs.contains(&req.id) {
            "open"
        } else {
            ""
        };
        let search_text = format!(
            "{} {} {} {} {} request",
            req.method,
            req.name,
            req.url,
            base_path.join(" / "),
            open_hint
        );
        out.push(PaletteEntry {
            section: PaletteSection::Requests,
            title: req.name.clone().into(),
            subtitle,
            method: Some(req.method.clone().into()),
            icon: "icons/globe.svg",
            active,
            search_text,
            target: PaletteTarget::Request { id: req.id.clone() },
        });
    }

    for folder in &collection.folders {
        collect_folder_entries(folder, &base_path, &[], selected_id, open_tabs, out);
    }
}

fn collect_folder_entries(
    folder: &Folder,
    parent_names: &[String],
    ancestor_ids: &[String],
    selected_id: Option<&str>,
    open_tabs: &[String],
    out: &mut Vec<PaletteEntry>,
) {
    let folder_subtitle = if parent_names.is_empty() {
        format!("{} request(s)", folder.requests.len())
    } else {
        format!(
            "{} · {} request(s)",
            parent_names.join(" / "),
            folder.requests.len()
        )
    };
    out.push(PaletteEntry {
        section: PaletteSection::Folders,
        title: folder.name.clone().into(),
        subtitle: Some(folder_subtitle.into()),
        method: None,
        icon: "icons/folder-closed.svg",
        active: false,
        search_text: format!("{} {} folder", parent_names.join(" "), folder.name),
        target: PaletteTarget::Folder {
            id: folder.id.clone(),
            ancestor_ids: ancestor_ids.to_vec(),
        },
    });

    let mut next_names = parent_names.to_vec();
    next_names.push(folder.name.clone());
    let path_label = next_names.join(" / ");

    for req in &folder.requests {
        let subtitle = if req.url.trim().is_empty() {
            path_label.clone()
        } else {
            format!("{path_label} · {}", req.url)
        };
        let active = selected_id == Some(req.id.as_str());
        let open_hint = if open_tabs.contains(&req.id) {
            "open"
        } else {
            ""
        };
        out.push(PaletteEntry {
            section: PaletteSection::Requests,
            title: req.name.clone().into(),
            subtitle: Some(subtitle.into()),
            method: Some(req.method.clone().into()),
            icon: "icons/globe.svg",
            active,
            search_text: format!(
                "{} {} {} {} {} request",
                req.method, req.name, req.url, path_label, open_hint
            ),
            target: PaletteTarget::Request { id: req.id.clone() },
        });
    }

    let mut next_ids = ancestor_ids.to_vec();
    next_ids.push(folder.id.clone());
    for sub in &folder.folders {
        collect_folder_entries(sub, &next_names, &next_ids, selected_id, open_tabs, out);
    }
}

pub(super) fn build_catalog(view: &WorkspaceView) -> Vec<PaletteEntry> {
    let mut entries = Vec::new();
    let project = view.current();
    let multi_collection = project.collections.len() > 1;

    for col in &project.collections {
        collect_collection_entries(
            col,
            multi_collection,
            view.selected_id.as_deref(),
            &view.open_tabs,
            &mut entries,
        );
    }

    for (index, proj) in view.projects.iter().enumerate() {
        let active = index == view.active_project;
        let req_count = proj.request_count();
        let subtitle = format!("{req_count} requests · {} variables", proj.variables.len());
        entries.push(PaletteEntry {
            section: PaletteSection::Projects,
            title: proj.name.clone().into(),
            subtitle: Some(subtitle.into()),
            method: None,
            icon: "icons/folder-open.svg",
            active,
            search_text: format!("{} project", proj.name),
            target: PaletteTarget::Project { index },
        });
    }

    for (index, ws) in view.workspaces.iter().enumerate() {
        let active = index == view.active_workspace;
        entries.push(PaletteEntry {
            section: PaletteSection::Workspaces,
            title: ws.name.clone().into(),
            subtitle: Some(
                if active {
                    "Current workspace"
                } else {
                    "Switch workspace"
                }
                .into(),
            ),
            method: None,
            icon: "icons/layers.svg",
            active,
            search_text: format!("{} workspace", ws.name),
            target: PaletteTarget::Workspace { index },
        });
    }

    let has_selected = view.selected_id.is_some();
    let action_list: Vec<PaletteAction> = [
        Some(PaletteAction::SendRequest),
        Some(PaletteAction::SaveRequest),
        Some(PaletteAction::NewRequest),
        Some(PaletteAction::NewFolder),
        has_selected.then_some(PaletteAction::DuplicateRequest),
        has_selected.then_some(PaletteAction::RenameRequest),
        has_selected.then_some(PaletteAction::CloseActiveTab),
        Some(PaletteAction::NewProject),
        Some(PaletteAction::NewWorkspace),
        Some(PaletteAction::FocusUrl),
        Some(PaletteAction::FocusSearch),
        Some(PaletteAction::OpenVariables),
        Some(PaletteAction::OpenImport),
        Some(PaletteAction::ToggleTheme),
        Some(PaletteAction::CycleResponsePosition),
    ]
    .into_iter()
    .flatten()
    .collect();

    for action in action_list {
        entries.push(PaletteEntry {
            section: PaletteSection::Actions,
            title: action.label().into(),
            subtitle: Some(action.subtitle().into()),
            method: None,
            icon: action.icon(),
            active: false,
            search_text: format!("{} {}", action.label(), action.keywords()),
            target: PaletteTarget::Action(action),
        });
    }

    entries
}

pub(super) fn filter_catalog<'a>(
    catalog: &'a [PaletteEntry],
    query: &str,
) -> Vec<(PaletteSection, Vec<&'a PaletteEntry>)> {
    let has_query = !query.trim().is_empty();
    let mut section_buckets: [Vec<(i32, usize, &'a PaletteEntry)>; 5] = Default::default();

    for (orig_ix, entry) in catalog.iter().enumerate() {
        let Some(score) = fuzzy_score(query, &entry.search_text) else {
            continue;
        };
        let bucket_ix = match entry.section {
            PaletteSection::Requests => 0,
            PaletteSection::Folders => 1,
            PaletteSection::Projects => 2,
            PaletteSection::Workspaces => 3,
            PaletteSection::Actions => 4,
        };
        section_buckets[bucket_ix].push((score, orig_ix, entry));
    }

    let mut groups = Vec::new();
    for (bucket_ix, section) in PaletteSection::ALL.into_iter().enumerate() {
        let mut items = std::mem::take(&mut section_buckets[bucket_ix]);
        if items.is_empty() {
            continue;
        }
        if has_query {
            items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        }
        let best_score = items[0].0;
        let entries = items.into_iter().map(|(_, _, entry)| entry).collect();
        groups.push((best_score, bucket_ix, section, entries));
    }

    if has_query {
        groups.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    }

    groups
        .into_iter()
        .map(|(_, _, section, entries)| (section, entries))
        .collect()
}

pub fn execute_palette_target(
    target: PaletteTarget,
    workspace: &WeakEntity<WorkspaceView>,
    window: &mut Window,
    cx: &mut App,
) {
    let _ = workspace.update(cx, |view, cx| match target {
        PaletteTarget::Request { id } => {
            view.open_request(id, window, cx);
        }
        PaletteTarget::Folder { id, ancestor_ids } => {
            view.focus_folder_from_palette(id, ancestor_ids, cx);
        }
        PaletteTarget::Project { index } => {
            view.switch_project(index, window, cx);
        }
        PaletteTarget::Workspace { index } => {
            view.switch_workspace(index, window, cx);
        }
        PaletteTarget::Action(action) => match action {
            PaletteAction::SendRequest => view.send(Some(window), cx),
            PaletteAction::SaveRequest => view.save_with_toast(window, cx),
            PaletteAction::NewRequest => view.add_request(window, cx),
            PaletteAction::NewFolder => view.add_folder(cx),
            PaletteAction::DuplicateRequest => {
                if let Some(id) = view.selected_id.clone() {
                    view.duplicate_request(&id, window, cx);
                }
            }
            PaletteAction::RenameRequest => view.toggle_rename_request(window, cx),
            PaletteAction::CloseActiveTab => {
                if let Some(id) = view.selected_id.clone() {
                    view.close_tab(&id, window, cx);
                }
            }
            PaletteAction::NewProject => {
                view.open_name_dialog(NameDialogKind::NewProject, window, cx);
            }
            PaletteAction::NewWorkspace => {
                view.open_name_dialog(NameDialogKind::NewWorkspace, window, cx);
            }
            PaletteAction::FocusUrl => {
                view.url.read(cx).focus_handle(cx).focus(window, cx);
            }
            PaletteAction::FocusSearch => {
                view.explorer_search
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx);
            }
            PaletteAction::OpenVariables => view.open_variables_sheet(window, cx),
            PaletteAction::OpenImport => view.open_import_dialog(window, cx),
            PaletteAction::ToggleTheme => view.toggle_theme(window, cx),
            PaletteAction::CycleResponsePosition => {
                let next = match view.response_position {
                    ResponsePosition::Right => ResponsePosition::Bottom,
                    ResponsePosition::Bottom => ResponsePosition::Left,
                    ResponsePosition::Left => ResponsePosition::Right,
                };
                view.set_response_position(next, cx);
                view.set_status(format!("Response layout: {}", next.label()), false, cx);
            }
        },
    });
}

pub struct CommandPaletteView {
    pub(super) command: Entity<CommandState>,
    pub(super) catalog: Vec<PaletteEntry>,
    pub(super) query: String,
    visible_targets: Vec<Vec<PaletteTarget>>,
    workspace: WeakEntity<WorkspaceView>,
}

impl CommandPaletteView {
    pub fn new(
        workspace_view: &WorkspaceView,
        workspace: WeakEntity<WorkspaceView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let command = cx.new(|cx| CommandState::new(window, cx));
        let catalog = build_catalog(workspace_view);
        Self {
            command,
            catalog,
            query: String::new(),
            visible_targets: Vec::new(),
            workspace,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.command.focus_handle(cx).focus(window, cx);
    }

    fn resolve_target(&self, path: IndexPath) -> Option<PaletteTarget> {
        self.visible_targets
            .get(path.section)
            .and_then(|rows| rows.get(path.row))
            .cloned()
    }
}

fn render_palette_row(entry: &PaletteEntry, shortcut: Option<Kbd>, cx: &App) -> gpui::Div {
    let muted = cx.theme().muted_foreground;
    let badge_bg = cx.theme().secondary;
    let badge_fg = cx.theme().secondary_foreground;
    let title = entry.title.clone();
    let subtitle = entry.subtitle.clone();
    let method = entry.method.clone();
    let icon = entry.icon;
    let active = entry.active;

    div()
        .w_full()
        .flex()
        .items_center()
        .gap_2p5()
        .child(match method {
            Some(method) => method_badge(method.as_ref(), Some(px(46.)), cx),
            None => div()
                .w(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(svg_icon(icon).small())
                .into_any_element(),
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(title),
                )
                .when_some(subtitle, |row, subtitle| {
                    row.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(subtitle),
                    )
                }),
        )
        .when(active, |row| {
            row.child(
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .bg(badge_bg)
                    .text_color(badge_fg)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Active"),
            )
        })
        .when_some(shortcut, |row, kbd| {
            row.child(div().flex_shrink_0().child(kbd))
        })
}

impl Render for CommandPaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filtered = filter_catalog(&self.catalog, &self.query);
        self.visible_targets = filtered
            .iter()
            .map(|(_, entries)| entries.iter().map(|entry| entry.target.clone()).collect())
            .collect();
        let total_matches: usize = filtered.iter().map(|(_, entries)| entries.len()).sum();

        let mut command = Command::new(&self.command)
            .bordered(false)
            .filterable(false)
            .max_h(px(360.))
            .placeholder("Jump to request, folder, project, workspace, or run command...")
            .on_query({
                let weak_self = cx.weak_entity();
                move |next_query, _, cx| {
                    let owned = next_query.to_string();
                    let _ = weak_self.update(cx, |view, cx| {
                        if view.query != owned {
                            view.query = owned;
                            cx.notify();
                        }
                    });
                }
            })
            .on_confirm({
                let weak_self = cx.weak_entity();
                let workspace = self.workspace.clone();
                move |path, window, cx| {
                    let target = weak_self
                        .upgrade()
                        .and_then(|entity| entity.read(cx).resolve_target(path));
                    let _ = workspace.update(cx, |view, _| {
                        view.active_command_palette = None;
                    });
                    window.close_dialog(cx);
                    if let Some(target) = target {
                        execute_palette_target(target, &workspace, window, cx);
                    }
                }
            })
            .on_cancel({
                let workspace = self.workspace.clone();
                move |_, cx| {
                    let _ = workspace.update(cx, |view, _| {
                        view.active_command_palette = None;
                    });
                }
            })
            .empty(|_, _, cx| {
                div()
                    .py_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.5))
                    .text_color(cx.theme().muted_foreground)
                    .child("No matching requests, folders, or commands")
            })
            .footer(move |_, _, cx| {
                div()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child("↑↓ Navigate · Enter Run · Esc Clear/Close")
                    .child(format!("{total_matches} result(s)"))
            });

        for (section, entries) in filtered {
            let mut group = CommandGroup::new().label(section.title());
            for entry in entries {
                let entry_owned = entry.clone();
                let shortcut = match entry.target {
                    PaletteTarget::Action(action) => action.shortcut_kbd(window),
                    _ => None,
                };
                group =
                    group.item(CommandItem::new().label(entry_owned.title.clone()).child(
                        move |_, cx| render_palette_row(&entry_owned, shortcut.clone(), cx),
                    ));
            }
            command = command.group(group);
        }

        let workspace_for_toggle = self.workspace.clone();
        div()
            .w_full()
            .on_action(
                move |_: &crate::OpenCommandPalette, window: &mut Window, cx: &mut App| {
                    let _ = workspace_for_toggle.update(cx, |view, _| {
                        view.active_command_palette = None;
                    });
                    window.close_dialog(cx);
                },
            )
            .child(command)
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn fuzzy_score_ranks_prefix_and_multi_word_queries() {
        let prefix = fuzzy_score("get", "GET List Users /v1/users").unwrap();
        let middle = fuzzy_score("get", "POST Create Target /v1/targets").unwrap();
        assert!(prefix > middle);

        assert!(fuzzy_score("post auth", "POST Login https://api.local/auth/token").is_some());
        assert!(fuzzy_score("gusr", "GET Users /v1/users").is_some());
        assert!(fuzzy_score("zzzz", "GET Users /v1/users").is_none());
    }
}
