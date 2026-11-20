use anyhow::{Context as _, Result};
use fastman_core::{
    ApiRequest, Auth, Collection, Folder, HttpBody, HttpEngine, HttpResponse, KeyValue, Project,
    ProjectStore, Workspace,
};
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::IndexPath;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState, Textarea, TextareaState},
    menu::{DropdownMenu as _, PopupMenuItem},
    resizable::{h_resizable, resizable_panel, v_resizable},
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{collections::HashSet, sync::Arc};
use tokio::{runtime::Runtime, task::AbortHandle};

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const SECTIONS: &[&str] = &["Params", "Auth", "Headers", "Body", "Scripts", "Settings"];
const AUTH_TYPES: &[&str] = &[
    "No Auth",
    "Basic Auth",
    "Bearer Token",
    "API Key (Header)",
    "API Key (Query)",
];
const BODY_TYPES: &[&str] = &["None", "JSON", "Text", "Form URL Encoded"];

// Project selection uses stable UUIDs; two projects may share the same name.
#[derive(Clone)]
struct ProjectChoice {
    id: String,
    name: SharedString,
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

/// Retained window state: project model + editor entities + request lifecycle.
/// Only one project is active. Other projects remain persisted on disk.
pub struct WorkspaceView {
    workspace_store: ProjectStore,
    workspaces: Vec<Workspace>,
    active_workspace: usize,
    store: ProjectStore,
    projects: Vec<Project>,
    active_project: usize,
    project_selector: Entity<SelectState<SearchableVec<ProjectChoice>>>,
    workspace_name: Entity<InputState>,
    project_name: Entity<InputState>,
    creating_workspace: bool,
    creating_project: bool,
    editing_request_name: bool,
    selected_id: Option<String>,
    open_tabs: Vec<String>,
    expanded: HashSet<String>,
    target_folder: Option<String>,
    method: String,
    body_kind: usize,
    auth_kind: usize,
    section: usize,
    url: Entity<InputState>,
    request_name: Entity<InputState>,
    params: Entity<TextareaState>,
    headers: Entity<TextareaState>,
    body: Entity<TextareaState>,
    timeout: Entity<InputState>,
    auth_user: Entity<InputState>,
    auth_password: Entity<InputState>,
    auth_token: Entity<InputState>,
    api_key_name: Entity<InputState>,
    api_key_value: Entity<InputState>,
    runtime: Arc<Runtime>,
    engine: HttpEngine,
    run_generation: u64,
    pending_abort: Option<AbortHandle>,
    pending: Option<Task<()>>,
    response: Option<HttpResponse>,
    response_error: Option<String>,
    status: String,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Result<Self> {
        let store = ProjectStore::system_default()?;
        let workspaces = store.workspaces()?;
        let mut projects = store.list()?;
        if projects.is_empty() {
            let mut project = store.create("My API Project")?;
            let mut folder = Folder::new("Examples");
            // This sample is never sent automatically.
            folder.requests.push(ApiRequest::new(
                "GET",
                "Example GET",
                "https://httpbin.org/get",
            ));
            project.collections[0].folders.push(folder);
            store.save(&project)?;
            projects.push(project);
        }
        let first = projects[0].first_request_id();
        let project_selector = cx.new(|cx| {
            SelectState::new(
                project_choices(&projects),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(
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
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("Build networking runtime")?;
        let engine = HttpEngine::new().context("Build HTTP client")?;
        let mut view = Self {
            workspace_store: store.clone(),
            workspaces,
            active_workspace: 0,
            store,
            projects,
            active_project: 0,
            project_selector,
            workspace_name: cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name")),
            project_name: cx.new(|cx| InputState::new(window, cx).placeholder("Project name")),
            creating_workspace: false,
            creating_project: false,
            editing_request_name: false,
            selected_id: first.clone(),
            open_tabs: first.into_iter().collect(),
            expanded: HashSet::new(),
            target_folder: None,
            method: "GET".into(),
            body_kind: 0,
            auth_kind: 0,
            section: 0,
            url: cx.new(|cx| InputState::new(window, cx).placeholder("https://api.example.com")),
            request_name: cx.new(|cx| InputState::new(window, cx).placeholder("Request name")),
            params: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(6)
                    .placeholder("key=value (one per line)")
            }),
            headers: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(6)
                    .placeholder("Header: value (one per line)")
            }),
            body: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(8)
                    .placeholder("Request body")
            }),
            timeout: cx.new(|cx| InputState::new(window, cx).placeholder("30000")),
            auth_user: cx.new(|cx| InputState::new(window, cx).placeholder("Username")),
            auth_password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Password")
                    .masked(true)
            }),
            auth_token: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Bearer token")
                    .masked(true)
            }),
            api_key_name: cx.new(|cx| InputState::new(window, cx).placeholder("Key name")),
            api_key_value: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("API key")
                    .masked(true)
            }),
            runtime: Arc::new(runtime),
            engine,
            run_generation: 0,
            pending_abort: None,
            pending: None,
            response: None,
            response_error: None,
            status: "Ready · local-first".into(),
            _subscriptions: vec![subscription],
        };
        view.expand_all();
        view.load_editor(window, cx);

        Ok(view)
    }

    fn current(&self) -> &Project {
        &self.projects[self.active_project]
    }

    fn current_request(&self) -> Option<&ApiRequest> {
        self.selected_id
            .as_deref()
            .and_then(|id| self.current().find_request(id))
    }

    fn expand_all(&mut self) {
        fn expand_folders(items: &[Folder], expanded: &mut HashSet<String>) {
            for folder in items {
                expanded.insert(folder.id.clone());
                expand_folders(&folder.folders, expanded);
            }
        }
        self.expanded.clear();
        let collections = self.current().collections.clone();
        for collection in &collections {
            self.expanded.insert(collection.id.clone());
            expand_folders(&collection.folders, &mut self.expanded);
        }
    }

    fn set_input(
        state: &Entity<InputState>,
        value: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    fn set_textarea(
        state: &Entity<TextareaState>,
        value: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    fn load_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_request_name = false;
        let request = self.current_request().cloned();
        if let Some(request) = request {
            self.method = request.method;
            Self::set_input(&self.request_name, request.name, window, cx);
            Self::set_input(&self.url, request.url, window, cx);
            Self::set_textarea(&self.params, format_kvs(&request.params, '='), window, cx);
            Self::set_textarea(&self.headers, format_kvs(&request.headers, ':'), window, cx);
            let (body_kind, body) = match request.body {
                HttpBody::None => (0, String::new()),
                HttpBody::Json(text) => (1, text),
                HttpBody::Text(text) => (2, text),
                HttpBody::FormUrlEncoded(pairs) => (3, format_kvs(&pairs, '=')),
            };
            self.body_kind = body_kind;
            Self::set_textarea(&self.body, body, window, cx);
            Self::set_input(&self.timeout, request.timeout_ms.to_string(), window, cx);
            self.auth_kind = match &request.auth {
                Auth::None => 0,
                Auth::Basic { .. } => 1,
                Auth::Bearer { .. } => 2,
                Auth::ApiKeyHeader { .. } => 3,
                Auth::ApiKeyQuery { .. } => 4,
            };
            // Clear all credential inputs first, avoiding carryover between requests.
            for field in [
                &self.auth_user,
                &self.auth_password,
                &self.auth_token,
                &self.api_key_name,
                &self.api_key_value,
            ] {
                Self::set_input(field, "", window, cx);
            }
            match request.auth {
                Auth::Basic { username, password } => {
                    Self::set_input(&self.auth_user, username, window, cx);
                    Self::set_input(&self.auth_password, password, window, cx);
                }
                Auth::Bearer { token } => Self::set_input(&self.auth_token, token, window, cx),
                Auth::ApiKeyHeader { name, value } | Auth::ApiKeyQuery { name, value } => {
                    Self::set_input(&self.api_key_name, name, window, cx);
                    Self::set_input(&self.api_key_value, value, window, cx);
                }
                Auth::None => {}
            }
        } else {
            self.method = "GET".into();
            self.auth_kind = 0;
            self.body_kind = 0;
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
            for field in [&self.params, &self.headers, &self.body] {
                Self::set_textarea(field, "", window, cx);
            }
            Self::set_input(&self.timeout, "30000", window, cx);
        }
        self.response = None;
        self.response_error = None;
        cx.notify();
    }

    /// Synchronize active editor state back to the in-memory request.
    fn apply_editor(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let Some(id) = self.selected_id.as_deref() else {
            return Ok(());
        };
        let url = self.url.read(cx).value().to_string();
        let name = self.request_name.read(cx).value().to_string();
        let params = parse_kvs(&self.params.read(cx).value(), '=');
        let headers = parse_kvs(&self.headers.read(cx).value(), ':');
        let body_text = self.body.read(cx).value().to_string();
        let timeout = self
            .timeout
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .context("Timeout must be an integer number of milliseconds")?;
        let body = match self.body_kind {
            1 => HttpBody::Json(body_text),
            2 => HttpBody::Text(body_text),
            3 => HttpBody::FormUrlEncoded(parse_kvs(&body_text, '=')),
            _ => HttpBody::None,
        };
        let auth = match self.auth_kind {
            1 => Auth::Basic {
                username: self.auth_user.read(cx).value().to_string(),
                password: self.auth_password.read(cx).value().to_string(),
            },
            2 => Auth::Bearer {
                token: self.auth_token.read(cx).value().to_string(),
            },
            3 => Auth::ApiKeyHeader {
                name: self.api_key_name.read(cx).value().to_string(),
                value: self.api_key_value.read(cx).value().to_string(),
            },
            4 => Auth::ApiKeyQuery {
                name: self.api_key_name.read(cx).value().to_string(),
                value: self.api_key_value.read(cx).value().to_string(),
            },
            _ => Auth::None,
        };
        let method = self.method.clone();
        if let Some(request) = self.projects[self.active_project].find_request_mut(id) {
            request.name = name;
            request.method = method;
            request.url = url;
            request.params = params;
            request.headers = headers;
            request.body = body;
            request.auth = auth;
            request.timeout_ms = timeout;
        }
        Ok(())
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let result = self
            .apply_editor(cx)
            .and_then(|()| self.store.save(self.current()));
        self.status = match result {
            Ok(()) => "Saved locally".into(),
            Err(e) => format!("Save failed: {e:#}"),
        };
        cx.notify();
    }

    fn open_request(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_id.as_ref() == Some(&id) {
            return;
        }
        // Fail safe: do not silently discard edits when switching tabs.
        if let Err(error) = self
            .apply_editor(cx)
            .and_then(|()| self.store.save(self.current()))
        {
            self.status = format!("Cannot switch: {error:#}");
            cx.notify();
            return;
        }
        self.cancel(cx);
        self.selected_id = Some(id.clone());
        if !self.open_tabs.contains(&id) {
            self.open_tabs.push(id);
        }
        self.load_editor(window, cx);
    }

    fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_id.as_deref() == Some(id) {
            if let Err(error) = self
                .apply_editor(cx)
                .and_then(|()| self.store.save(self.current()))
            {
                self.status = format!("Cannot close tab: {error:#}");
                cx.notify();
                return;
            }
            self.cancel(cx);
        }
        self.open_tabs.retain(|tab| tab != id);
        if self.selected_id.as_deref() == Some(id) {
            self.selected_id = self.open_tabs.last().cloned();
            self.load_editor(window, cx);
        }
        cx.notify();
    }

    fn switch_project(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.active_project || index >= self.projects.len() {
            return;
        }
        if let Err(error) = self
            .apply_editor(cx)
            .and_then(|()| self.store.save(self.current()))
        {
            self.status = format!("Cannot switch: {error:#}");
            return;
        }
        self.cancel(cx);
        self.active_project = index;
        self.selected_id = self.current().first_request_id();
        self.open_tabs = self.selected_id.clone().into_iter().collect();
        self.target_folder = None;
        self.expand_all();
        self.load_editor(window, cx);
    }

    fn switch_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.active_workspace || index >= self.workspaces.len() {
            return;
        }
        let result = (|| -> Result<(ProjectStore, Vec<Project>)> {
            self.apply_editor(cx)?;
            self.store.save(self.current())?;
            let store = self
                .workspace_store
                .for_workspace(&self.workspaces[index].id)?;
            let mut projects = store.list()?;
            if projects.is_empty() {
                projects.push(store.create("Project 1")?);
            }
            Ok((store, projects))
        })();
        match result {
            Ok((store, projects)) => {
                self.cancel(cx);
                self.store = store;
                self.projects = projects;
                self.active_workspace = index;
                self.active_project = 0;
                self.target_folder = None;
                self.selected_id = self.current().first_request_id();
                self.open_tabs = self.selected_id.clone().into_iter().collect();
                let choices = project_choices(&self.projects);
                self.project_selector.update(cx, |selector, cx| {
                    selector.set_items(choices, window, cx);
                    selector.set_selected_index(Some(IndexPath::default()), window, cx);
                });
                self.expand_all();
                self.load_editor(window, cx);
                self.status = format!("Workspace: {}", self.workspaces[index].name);
            }
            Err(error) => self.status = format!("Cannot switch workspace: {error:#}"),
        }
        cx.notify();
    }

    fn add_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.workspace_name.read(cx).value().to_string();
        match self.workspace_store.create_workspace(&name) {
            Ok(workspace) => {
                self.workspaces.push(workspace);
                self.creating_workspace = false;
                Self::set_input(&self.workspace_name, "", window, cx);
                self.switch_workspace(self.workspaces.len() - 1, window, cx);
            }
            Err(error) => self.status = format!("Create workspace failed: {error:#}"),
        }
        cx.notify();
    }

    fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self
            .apply_editor(cx)
            .and_then(|()| self.store.save(self.current()))
        {
            self.status = format!("Cannot add project: {error:#}");
            return;
        }
        let name = self.project_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.status = "Project name is required".into();
            cx.notify();
            return;
        }
        match self.store.create(name) {
            Ok(project) => {
                self.creating_project = false;
                Self::set_input(&self.project_name, "", window, cx);
                self.projects.push(project);
                let last = self.projects.len() - 1;
                let choices = project_choices(&self.projects);
                self.project_selector.update(cx, |selector, cx| {
                    selector.set_items(choices, window, cx);
                    selector.set_selected_index(Some(IndexPath::default().row(last)), window, cx);
                });
                self.switch_project(last, window, cx);
            }
            Err(error) => self.status = format!("Create project failed: {error:#}"),
        }
        cx.notify();
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let folder = Folder::new("New Folder");
        let id = folder.id.clone();
        if let Some(collection) = self.projects[self.active_project].collections.first_mut() {
            collection.folders.push(folder);
            self.expanded.insert(id.clone());
            self.target_folder = Some(id);
            self.save(cx);
        }
    }

    fn add_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self
            .apply_editor(cx)
            .and_then(|()| self.store.save(self.current()))
        {
            self.status = format!("Cannot add request: {error:#}");
            return;
        }
        let request = ApiRequest::new("GET", "New Request", "");
        let id = request.id.clone();
        let mut pending = Some(request);
        if let Some(target) = self.target_folder.as_deref() {
            fn add_to_folder(
                folders: &mut [Folder],
                target: &str,
                pending: &mut Option<ApiRequest>,
            ) {
                for folder in folders {
                    if folder.id == target {
                        if let Some(request) = pending.take() {
                            folder.requests.push(request);
                        }
                        return;
                    }
                    add_to_folder(&mut folder.folders, target, pending);
                    if pending.is_none() {
                        return;
                    }
                }
            }
            for collection in &mut self.projects[self.active_project].collections {
                add_to_folder(&mut collection.folders, target, &mut pending);
                if pending.is_none() {
                    break;
                }
            }
        }
        if let Some(request) = pending {
            if self.projects[self.active_project].collections.is_empty() {
                self.projects[self.active_project]
                    .collections
                    .push(Collection::new("Default Collection"));
            }
            self.projects[self.active_project].collections[0]
                .requests
                .push(request);
        }
        match self.store.save(self.current()) {
            Ok(()) => self.open_request(id, window, cx),
            Err(error) => self.status = format!("Create request failed: {error:#}"),
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.run_generation = self.run_generation.wrapping_add(1);
        if let Some(abort) = self.pending_abort.take() {
            abort.abort();
        }
        self.pending = None;
        self.status = "Request cancelled / idle".into();
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self
            .apply_editor(cx)
            .and_then(|()| self.store.save(self.current()))
        {
            self.status = format!("Cannot send: {error:#}");
            cx.notify();
            return;
        }
        let Some(request) = self.current_request().cloned() else {
            return;
        };
        let variables = self.current().variables.clone();
        self.cancel(cx);
        self.response = None;
        self.response_error = None;
        self.status = format!("Sending {}...", request.method);
        let generation = self.run_generation;
        let engine = self.engine.clone();
        let handle = self
            .runtime
            .spawn(async move { engine.send(&request, &variables).await });
        self.pending_abort = Some(handle.abort_handle());
        self.pending = Some(cx.spawn(async move |this, cx| {
            let result = handle.await;
            _ = this.update(cx, |view, cx| {
                if view.run_generation != generation {
                    return;
                }
                view.pending_abort = None;
                match result {
                    Ok(Ok(response)) => {
                        view.status = format!(
                            "{} {} · {} ms · {} bytes{}",
                            response.status,
                            response.reason,
                            response.elapsed_ms,
                            response.bytes_received,
                            if response.truncated {
                                " · truncated"
                            } else {
                                ""
                            }
                        );
                        view.response = Some(response);
                    }
                    Ok(Err(error)) => {
                        view.status = "Request failed".into();
                        view.response_error = Some(error.to_string());
                    }
                    Err(error) => {
                        view.status = "Request interrupted".into();
                        view.response_error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn render_folder(
        &self,
        folder: &Folder,
        depth: f32,
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
    ) {
        let id = folder.id.clone();
        let open = self.expanded.contains(&id);
        let is_target = self.target_folder.as_ref() == Some(&id);
        let label = folder.name.clone();
        out.push(
            div()
                .pl(px(depth * 14.))
                .child(
                    Button::new(format!("folder-{id}"))
                        .ghost()
                        .small()
                        .w_full()
                        .accessibility_label(label.clone())
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_start()
                                .gap_2()
                                .child(
                                    gpui_kit::component::Icon::default()
                                        .path(if open {
                                            "icons/folder-open.svg"
                                        } else {
                                            "icons/folder-closed.svg"
                                        })
                                        .small(),
                                )
                                .child(label),
                        )
                        .selected(is_target)
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if !view.expanded.insert(id.clone()) {
                                view.expanded.remove(&id);
                            }
                            view.target_folder = Some(id.clone());
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        if open {
            for sub in &folder.folders {
                self.render_folder(sub, depth + 1., cx, out);
            }
            for req in &folder.requests {
                self.render_request_link(req, depth + 1., cx, out);
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
        let method_color = match method.as_str() {
            "GET" => 0x79c9a5,
            "POST" => 0xe4bb72,
            "PUT" | "PATCH" => 0x86aee8,
            "DELETE" => 0xe68f8f,
            _ => 0xb9bfca,
        };
        out.push(
            div()
                .bg(if selected {
                    rgb(0x303239)
                } else {
                    rgb(0x181d24)
                })
                .pl(px(depth * 14.))
                .child(
                    Button::new(format!("request-{id}"))
                        .ghost()
                        .small()
                        .w_full()
                        .accessibility_label(format!("{method} {title}"))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_start()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(52.))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(method_color))
                                        .child(method),
                                )
                                .child(div().child(title)),
                        )
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.open_request(id.clone(), window, cx)
                        })),
                )
                .into_any_element(),
        );
    }

    fn render_explorer(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        for collection in &self.current().collections {
            let id = collection.id.clone();
            let open = self.expanded.contains(&id);
            let title = collection.name.clone();
            out.push(
                Button::new(format!("collection-{id}"))
                    .ghost()
                    .small()
                    .w_full()
                    .accessibility_label(title.clone())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_start()
                            .gap_2()
                            .child(
                                gpui_kit::component::Icon::default()
                                    .path(if open {
                                        "icons/folder-open.svg"
                                    } else {
                                        "icons/folder-closed.svg"
                                    })
                                    .small(),
                            )
                            .child(title),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if !view.expanded.insert(id.clone()) {
                            view.expanded.remove(&id);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if open {
                for folder in &collection.folders {
                    self.render_folder(folder, 1., cx, &mut out);
                }
                for req in &collection.requests {
                    self.render_request_link(req, 1., cx, &mut out);
                }
            }
        }
        out
    }

    fn render_section(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.section {
            0 => div()
                .flex()
                .flex_col()
                .gap_2()
                .child("Query parameters · key=value, one per line")
                .child(Textarea::new(&self.params).h(px(152.)))
                .into_any_element(),
            1 => {
                let kind_label = AUTH_TYPES[self.auth_kind];
                let view = cx.weak_entity();
                let mut panel = div().flex().flex_col().gap_2().child(
                    Button::new("auth-kind")
                        .label(format!("{kind_label} ▾"))
                        .dropdown_menu(move |mut menu, _, _| {
                            for (i, label) in AUTH_TYPES.iter().enumerate() {
                                let view = view.clone();
                                menu = menu.item(PopupMenuItem::new(*label).on_click(
                                    move |_, _, cx| {
                                        let _ = view.update(cx, |view, cx| {
                                            view.auth_kind = i;
                                            cx.notify();
                                        });
                                    },
                                ));
                            }
                            menu
                        }),
                );
                panel = match self.auth_kind {
                    1 => panel
                        .child(Input::new(&self.auth_user))
                        .child(Input::new(&self.auth_password)),
                    2 => panel.child(Input::new(&self.auth_token)),
                    3 | 4 => panel
                        .child(Input::new(&self.api_key_name))
                        .child(Input::new(&self.api_key_value)),
                    _ => panel.child("No credentials sent"),
                };
                panel
                    .child("Credentials are kept in memory, not in saved project JSON.")
                    .into_any_element()
            }
            2 => div()
                .flex()
                .flex_col()
                .gap_2()
                .child("Headers · Header: value, one per line")
                .child(Textarea::new(&self.headers).h(px(152.)))
                .into_any_element(),
            3 => {
                let kind_label = BODY_TYPES[self.body_kind];
                let view = cx.weak_entity();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Button::new("body-kind")
                            .label(format!("{kind_label} ▾"))
                            .dropdown_menu(move |mut menu, _, _| {
                                for (i, label) in BODY_TYPES.iter().enumerate() {
                                    let view = view.clone();
                                    menu = menu.item(PopupMenuItem::new(*label).on_click(
                                        move |_, _, cx| {
                                            let _ = view.update(cx, |view, cx| {
                                                view.body_kind = i;
                                                cx.notify();
                                            });
                                        },
                                    ));
                                }
                                menu
                            }),
                    )
                    .child(Textarea::new(&self.body).h(px(152.)))
                    .into_any_element()
            }
            4 => div()
                .child(format!(
                    "Pre-request and post-response scripts are planned and are not executed in v{}.",
                    env!("CARGO_PKG_VERSION")
                ))
                .into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .gap_2()
                .child("Timeout (milliseconds, 100–300000)")
                .child(Input::new(&self.timeout))
                .child("TLS verification enabled. Redirects limited to 10.")
                .into_any_element(),
        }
    }

    fn render_response(&self) -> AnyElement {
        if let Some(response) = &self.response {
            let pretty = serde_json::from_str::<serde_json::Value>(&response.body)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok());
            let output = pretty.unwrap_or_else(|| response.body.clone());
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(format!(
                    "{} {}    ·    {} ms    ·    {} bytes{}",
                    response.status,
                    response.reason,
                    response.elapsed_ms,
                    response.bytes_received,
                    if response.truncated {
                        "    ·    truncated"
                    } else {
                        ""
                    }
                ))
                .child(div().text_color(rgb(0x9da7b5)).text_sm().child(format!(
                    "{} headers  ·  {}",
                    response.headers.len(),
                    response.final_url
                )))
                .child(
                    div()
                        .id("response-body")
                        .overflow_y_scroll()
                        .h(px(250.))
                        .p_2()
                        .bg(rgb(0x11151b))
                        .text_color(rgb(0xe5e8ef))
                        .child(output),
                )
                .into_any_element()
        } else if let Some(error) = &self.response_error {
            div()
                .text_color(rgb(0xf08b87))
                .child(format!("Request failed: {error}"))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .py_4()
                .child("No response yet")
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x9da7b5))
                        .child("Send a request to see its status, timing and response body."),
                )
                .into_any_element()
        }
    }
}

fn format_kvs(pairs: &[KeyValue], separator: char) -> String {
    pairs
        .iter()
        .map(|row| {
            format!(
                "{}{}{}{}",
                if row.enabled { "" } else { "# " },
                row.key,
                separator,
                row.value
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_kvs(text: &str, separator: char) -> Vec<KeyValue> {
    text.lines()
        .filter_map(|line| {
            if line.trim().is_empty() {
                return None;
            }
            let (enabled, content) = match line.trim_start().strip_prefix("# ") {
                Some(other) => (false, other),
                None => (true, line),
            };
            let (key, value) = content.split_once(separator)?;
            if key.trim().is_empty() {
                return None;
            }
            Some(KeyValue {
                enabled,
                key: key.trim().into(),
                value: value.trim().into(),
            })
        })
        .collect()
}

impl Render for WorkspaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = cx.theme().foreground;
        let background = rgb(0x161a20);
        let secondary = rgb(0x1b2028);
        let border = rgb(0x303741);
        let explorer = self.render_explorer(cx);

        let method_view = cx.weak_entity();
        let workspace_view = cx.weak_entity();
        let workspace_choices: Vec<String> =
            self.workspaces.iter().map(|w| w.name.clone()).collect();
        let mut tabs = Vec::new();
        for id in &self.open_tabs {
            if let Some(req) = self.current().find_request(id) {
                let id = id.clone();
                let label = format!("{} {}", req.method, req.name);
                let selected = self.selected_id.as_ref() == Some(&id);
                let close_id = id.clone();
                tabs.push(
                    div()
                        .flex()
                        .items_center()
                        .bg(if selected { rgb(0x303741) } else { background })
                        .rounded_md()
                        .child(if selected && self.editing_request_name {
                            div()
                                .w(px(180.))
                                .child(Input::new(&self.request_name).small())
                                .into_any_element()
                        } else {
                            Button::new(format!("tab-{id}"))
                                .ghost()
                                .small()
                                .label(label)
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.open_request(id.clone(), window, cx)
                                }))
                                .into_any_element()
                        })
                        .when(selected, |row| {
                            row.child(
                                Button::new("edit-request-name")
                                    .ghost()
                                    .small()
                                    .tooltip("Edit request name")
                                    .label(if self.editing_request_name {
                                        "Done"
                                    } else {
                                        "✎"
                                    })
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        if view.editing_request_name {
                                            match view.apply_editor(cx) {
                                                Ok(()) => view.editing_request_name = false,
                                                Err(error) => {
                                                    view.status =
                                                        format!("Cannot edit name: {error:#}")
                                                }
                                            }
                                        } else {
                                            view.editing_request_name = true;
                                        }
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            Button::new(format!("close-tab-{close_id}"))
                                .ghost()
                                .small()
                                .label("×")
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_tab(&close_id, window, cx)
                                })),
                        )
                        .into_any_element(),
                );
            }
        }
        let section_buttons: Vec<AnyElement> = SECTIONS
            .iter()
            .enumerate()
            .map(|(i, title)| {
                div()
                    .bg(if self.section == i {
                        rgb(0x303741)
                    } else {
                        background
                    })
                    .rounded_md()
                    .child(
                        Button::new(format!("section-{i}"))
                            .ghost()
                            .small()
                            .label(title.to_string())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.section = i;
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(background)
            .text_size(px(13.))
            .text_color(foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h(px(44.))
                    .flex_shrink_0()
                    .bg(secondary)
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_color(rgb(0xff966c))
                            .font_weight(FontWeight::BOLD)
                            .child("◆"),
                    )
                    .child(div().font_weight(FontWeight::BOLD).child("Fastman"))
                    .child(
                        Button::new("workspace-selector")
                            .ghost()
                            .small()
                            .label(format!("{} ▾", self.workspaces[self.active_workspace].name))
                            .dropdown_menu(move |mut menu, _, _| {
                                for (index, name) in workspace_choices.iter().enumerate() {
                                    let view = workspace_view.clone();
                                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                        move |_, window, cx| {
                                            let _ = view.update(cx, |view, cx| {
                                                view.switch_workspace(index, window, cx)
                                            });
                                        },
                                    ));
                                }
                                menu
                            }),
                    )
                    .child(
                        div()
                            .w(px(180.))
                            .when(self.creating_workspace, |el| {
                                el.child(Input::new(&self.workspace_name))
                            })
                            .when(!self.creating_workspace, |el| el.hidden()),
                    )
                    .child(
                        Button::new("create-workspace")
                            .ghost()
                            .small()
                            .label(if self.creating_workspace {
                                "Create"
                            } else {
                                "+ Workspace"
                            })
                            .on_click(cx.listener(|view, _, window, cx| {
                                if view.creating_workspace {
                                    view.add_workspace(window, cx);
                                } else {
                                    view.creating_workspace = true;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("save-global")
                            .small()
                            .label("Save request")
                            .on_click(cx.listener(|view, _, _, cx| view.save(cx))),
                    ),
            );

        root.child(
            div().flex_1().min_h_0().child(h_resizable("workspace-panels")
                .child(
                    resizable_panel().size(px(252.)).size_range(px(180.)..px(520.)).child(div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .bg(rgb(0x181d24))
                        .border_r_1()
                        .border_color(border)
                        .child(div().text_sm().text_color(rgb(0xaeb8c5)).child("Project"))
                        .child(
                            div()
                                .flex_shrink_0()
                                .child(Select::new(&self.project_selector).small()),
                        )
                        .child(
                            div()
                                .when(self.creating_project, |el| {
                                    el.child(Input::new(&self.project_name))
                                })
                                .when(!self.creating_project, |el| el.hidden()),
                        )
                        .child(
                            Button::new("create-project")
                                .small()
                                .label(if self.creating_project {
                                    "Create project"
                                } else {
                                    "+ New project"
                                })
                                .on_click(cx.listener(|view, _, window, cx| {
                                    if view.creating_project {
                                        view.add_project(window, cx);
                                    } else {
                                        view.creating_project = true;
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(div().h(px(1.)).bg(border))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("create-folder")
                                        .small()
                                        .label("+ Folder")
                                        .on_click(
                                            cx.listener(|view, _, _, cx| view.add_folder(cx)),
                                        ),
                                )
                                .child(
                                    Button::new("create-request")
                                        .small()
                                        .label("+ Request")
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.add_request(window, cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .pt_2()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Collections"),
                        )
                        .child(
                            div()
                                .id("explorer-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(explorer),
                        )
                        .into_any_element()),
                )
                .child(
                    resizable_panel().child(div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .child(
                            div()
                                .id("tabs-scroll")
                                .flex()
                                .gap_2()
                                .overflow_x_scroll()
                                .children(tabs),
                        )
                        .child(
                            div().flex_1().min_h_0().child(v_resizable("request-response-panels")
                            .child(resizable_panel().child(div().size_full().flex().flex_col().gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new("method")
                                        .small()
                                        .label(format!("{} ▾", self.method))
                                        .dropdown_menu(move |mut menu, _, _| {
                                            for method in METHODS {
                                                let view = method_view.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::new(*method).on_click(
                                                        move |_, _, cx| {
                                                            let _ = view.update(cx, |view, cx| {
                                                                view.method = (*method).into();
                                                                cx.notify();
                                                            });
                                                        },
                                                    ),
                                                );
                                            }
                                            menu
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Input::new(&self.url).small()),
                                )
                                .child(
                                    Button::new("send")
                                        .small()
                                        .primary()
                                        .disabled(
                                            self.selected_id.is_none()
                                                || self.pending_abort.is_some(),
                                        )
                                        .label("Send")
                                        .on_click(cx.listener(|view, _, _, cx| view.send(cx))),
                                )
                                .child(
                                    Button::new("cancel")
                                        .disabled(self.pending_abort.is_none())
                                        .label("Cancel")
                                        .on_click(cx.listener(|view, _, _, cx| view.cancel(cx))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .border_b_1()
                                .border_color(border)
                                .children(section_buttons),
                        )
                        .child(
                            div()
                                .id("section-scroll")
                                .flex_1()
                                .min_h_0()
                                .pt_2()
                                .overflow_y_scroll()
                                .child(self.render_section(cx)),
                        )
                        .into_any_element()))
                        .child(resizable_panel().child(div().size_full().flex().flex_col().gap_2()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("Response"))
                        .child(
                            div()
                                .id("response-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(self.render_response()),
                        ).into_any_element())))
                        )
                        .into_any_element()),
                )),
        )
        .child(
            div()
                .h(px(32.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px_3()
                .bg(secondary)
                .border_t_1()
                .border_color(border)
                .text_sm()
                .text_color(rgb(0xaeb8c5))
                .child(self.status.clone()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn key_value_editor_roundtrip() {
        let entries = vec![
            KeyValue::new("Accept", "application/json"),
            KeyValue {
                enabled: false,
                key: "X-Optional".into(),
                value: "value".into(),
            },
        ];
        assert_eq!(parse_kvs(&format_kvs(&entries, ':'), ':'), entries);
    }
}
