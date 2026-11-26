use crate::WorkspaceView;
use fastman_core::{
    CaptureSource, ChainRunOptions, ChainStep, ChainStepOutcome, ChainStepReport, Folder, Project,
    RequestChain, chain_runner::MAX_CHAIN_STEPS, run_chain_with_progress,
};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    input::{Editor, EditorState, Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    select::{SearchableVec, Select, SelectItem, SelectState},
};
use gpui_kit::*;
use gpui_kit::{base::Disableable as _, prelude::FluentBuilder as _};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
struct Choice {
    id: String,
    label: SharedString,
}

impl SelectItem for Choice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.label.clone()
    }
    fn value(&self) -> &String {
        &self.id
    }
}

type Selector = Entity<SelectState<SearchableVec<Choice>>>;

struct StepDraft {
    step: ChainStep,
    assertions: Entity<EditorState>,
    captures: Entity<EditorState>,
}

impl StepDraft {
    fn new(step: ChainStep, window: &mut Window, cx: &mut App) -> Self {
        let assertions = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx);
            editor.set_value(
                serde_json::to_string_pretty(&step.assertions).unwrap(),
                window,
                cx,
            );
            editor
        });
        let captures = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx);
            editor.set_value(
                serde_json::to_string_pretty(&step.captures).unwrap(),
                window,
                cx,
            );
            editor
        });
        Self {
            step,
            assertions,
            captures,
        }
    }
}

struct ChainSheet {
    workspace: WeakEntity<WorkspaceView>,
    project_id: String,
    chain_id: String,
    saved: bool,
    confirming_delete: bool,
    name: Entity<InputState>,
    requests: Vec<Choice>,
    request: Selector,
    steps: Vec<StepDraft>,
    selected: Option<usize>,
    environment: Selector,
    origin: Entity<InputState>,
    trusted: Entity<InputState>,
    reports: Vec<ChainStepReport>,
    run_definition: Option<RequestChain>,
    cancel: Option<oneshot::Sender<()>>,
    running: bool,
    error: Option<String>,
}

impl ChainSheet {
    fn new(
        workspace: WeakEntity<WorkspaceView>,
        project: &Project,
        chain: RequestChain,
        saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut requests = Vec::new();
        for collection in &project.collections {
            collect_requests(
                &collection.requests,
                &collection.folders,
                &collection.name,
                &mut requests,
            );
        }
        let request =
            cx.new(|cx| SelectState::new(SearchableVec::new(requests.clone()), None, window, cx));
        let mut environments = vec![Choice {
            id: String::new(),
            label: "Base variables".into(),
        }];
        environments.extend(project.environments.iter().map(|env| Choice {
            id: env.id.clone(),
            label: env.name.clone().into(),
        }));
        let env_index = project
            .active_environment
            .as_ref()
            .and_then(|id| environments.iter().position(|env| &env.id == id))
            .unwrap_or(0);
        let environment = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(environments),
                Some(IndexPath::default().row(env_index)),
                window,
                cx,
            )
        });
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Chain name");
            input.set_value(chain.name, window, cx);
            input
        });
        let steps: Vec<_> = chain
            .steps
            .into_iter()
            .map(|step| StepDraft::new(step, window, cx))
            .collect();
        let selected = (!steps.is_empty()).then_some(0);
        Self {
            workspace,
            project_id: project.id.clone(),
            chain_id: chain.id,
            saved,
            confirming_delete: false,
            name,
            requests,
            request,
            steps,
            selected,
            environment,
            origin: cx.new(|cx| InputState::new(window, cx).placeholder("https://api.example.com")),
            trusted: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Optional, space-separated origins")
            }),
            reports: Vec::new(),
            run_definition: None,
            cancel: None,
            running: false,
            error: None,
        }
    }

    fn build(&self, cx: &App) -> Result<RequestChain, String> {
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return Err("Enter a chain name.".into());
        }
        let mut steps = Vec::with_capacity(self.steps.len());
        for (index, draft) in self.steps.iter().enumerate() {
            let mut step = draft.step.clone();
            if !self
                .requests
                .iter()
                .any(|choice| choice.id == step.request_id)
            {
                return Err(format!(
                    "Step {} references a missing request. Replace or remove it.",
                    index + 1
                ));
            }
            step.assertions = serde_json::from_str(draft.assertions.read(cx).value().as_ref())
                .map_err(|error| {
                    format!(
                        "Step {} assertions: invalid rules at line {}, column {}.",
                        index + 1,
                        error.line(),
                        error.column()
                    )
                })?;
            step.captures = serde_json::from_str(draft.captures.read(cx).value().as_ref())
                .map_err(|error| {
                    format!(
                        "Step {} captures: invalid rules at line {}, column {}.",
                        index + 1,
                        error.line(),
                        error.column()
                    )
                })?;
            steps.push(step);
        }
        Ok(RequestChain {
            id: self.chain_id.clone(),
            name,
            steps,
        })
    }

    fn save(&mut self, delete: bool, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| {
            let chain = if delete { None } else { Some(self.build(cx)?) };
            self.workspace
                .update(cx, |view, cx| {
                    view.save_chain_definition(&self.project_id, &self.chain_id, chain, window, cx)
                })
                .map_err(|_| "Workspace closed.".to_owned())?
                .map_err(|error| error.to_string())
        })();
        match result {
            Ok(()) if delete => window.close_sheet(cx),
            Ok(()) => {
                self.saved = true;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let result = (|| {
            let chain = self.build(cx)?;
            let origin = self.origin.read(cx).value().trim().to_owned();
            if origin.is_empty() {
                return Err("Enter the primary origin to authorize this run.".into());
            }
            let trusted: Vec<_> = self
                .trusted
                .read(cx)
                .value()
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            let (project, secrets, runtime) = self
                .workspace
                .update(cx, |view, cx| {
                    view.chain_run_snapshot(&self.project_id, chain.clone(), cx)
                })
                .map_err(|_| "Workspace closed.".to_owned())?
                .map_err(|error| error.to_string())?;
            Ok((chain, origin, trusted, project, secrets, runtime))
        })();
        let (chain, origin, trusted, project, secrets, runtime) = match result {
            Ok(inputs) => inputs,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let env = self
            .environment
            .read(cx)
            .selected_value()
            .filter(|id| !id.is_empty())
            .cloned();
        let chain_id = chain.id.clone();
        let (cancel, cancelled) = oneshot::channel();
        let (progress, mut updates) = mpsc::unbounded_channel();
        self.cancel = Some(cancel);
        self.running = true;
        self.error = None;
        self.reports.clear();
        self.run_definition = Some(chain);
        let worker = runtime.spawn(async move {
            run_chain_with_progress(
                &project,
                &chain_id,
                env.as_deref(),
                secrets.as_ref(),
                ChainRunOptions {
                    primary_origin: &origin,
                    trusted_origins: &trusted,
                    timeout: Duration::from_secs(300),
                },
                async {
                    let _ = cancelled.await;
                },
                |report| {
                    let _ = progress.send(report.clone());
                },
            )
            .await
            .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            while let Some(report) = updates.recv().await {
                if this
                    .update(cx, |view, cx| {
                        view.reports.push(report);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = worker.await;
            let _ = this.update(cx, |view, cx| {
                view.running = false;
                view.cancel = None;
                view.error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(_) => Some("Run interrupted.".into()),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn edit_step(&mut self, edit: StepEdit, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.reports.clear();
        self.run_definition = None;
        match edit {
            StepEdit::Add => {
                if let Some(id) = self.request.read(cx).selected_value().cloned()
                    && self.steps.len() < MAX_CHAIN_STEPS
                {
                    self.selected = Some(self.steps.len());
                    self.steps
                        .push(StepDraft::new(ChainStep::new(id), window, cx));
                }
            }
            _ => {
                let Some(index) = self.selected else { return };
                match edit {
                    StepEdit::Replace => {
                        if let Some(id) = self.request.read(cx).selected_value() {
                            self.steps[index].step.request_id = id.clone();
                        }
                    }
                    StepEdit::Toggle => {
                        self.steps[index].step.enabled = !self.steps[index].step.enabled
                    }
                    StepEdit::Remove => {
                        self.steps.remove(index);
                        self.selected =
                            (!self.steps.is_empty()).then(|| index.min(self.steps.len() - 1));
                    }
                    StepEdit::Move(offset) => {
                        if let Some(next) = index
                            .checked_add_signed(offset)
                            .filter(|next| *next < self.steps.len())
                        {
                            self.steps.swap(index, next);
                            self.selected = Some(next);
                        }
                    }
                    StepEdit::Add => unreachable!(),
                }
            }
        }
        cx.notify();
    }

    fn render_steps(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected;
        let choose = |label, id, edit| {
            Button::new(id)
                .small()
                .outline()
                .label(label)
                .disabled(self.running)
                .on_click(cx.listener(move |view, _, window, cx| view.edit_step(edit, window, cx)))
        };
        let mut section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(format!("Steps · {}/{}", self.steps.len(), MAX_CHAIN_STEPS))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            Select::new(&self.request)
                                .small()
                                .placeholder("Choose a saved request"),
                        ),
                    )
                    .child(choose("Add step", "add-chain-step", StepEdit::Add))
                    .when(selected.is_some(), |el| {
                        el.child(choose(
                            "Replace selected",
                            "replace-chain-step",
                            StepEdit::Replace,
                        ))
                    }),
            );
        for (index, draft) in self.steps.iter().enumerate() {
            let request = self
                .requests
                .iter()
                .find(|choice| choice.id == draft.step.request_id);
            let state = self.reports.get(index).map_or(
                if draft.step.enabled {
                    ""
                } else {
                    " · Disabled"
                },
                step_state,
            );
            let name = request.map_or("Missing request", |choice| choice.label.as_ref());
            let button = Button::new(("chain-step", index))
                .small()
                .label(format!("{}. {name}{state}", index + 1))
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.selected = Some(index);
                    cx.notify();
                }));
            section = section.child(if selected == Some(index) {
                button.primary()
            } else {
                button.outline()
            });
        }
        if let Some(index) = selected {
            let draft = &self.steps[index];
            section = section.child(div().flex().gap_2()
                .child(choose("Up", "chain-step-up", StepEdit::Move(-1)))
                .child(choose("Down", "chain-step-down", StepEdit::Move(1)))
                .child(choose(if draft.step.enabled { "Disable" } else { "Enable" }, "toggle-chain-step", StepEdit::Toggle))
                .child(choose("Remove", "remove-chain-step", StepEdit::Remove)))
                .child("Assertions (JSON array): status, header, json_field, or latency")
                .child(div().text_sm().child(r#"[{"kind":"status","expected":200}]"#))
                .child(Editor::new(&draft.assertions).small().h(px(100.)))
                .child("Captures (JSON array): name, source, required, sensitive")
                .child(div().text_sm().child(r#"[{"name":"token","source":{"kind":"header","name":"X-Token"},"required":true,"sensitive":true}]"#))
                .child(div().text_sm().child("Use {{token}} in a later request. Source kinds: header, json_field, status."))
                .child(Editor::new(&draft.captures).small().h(px(100.)));
        }
        section.into_any_element()
    }

    fn render_result(&self) -> Option<AnyElement> {
        let index = self.selected?;
        let report = self.reports.get(index)?;
        let mut result = div().flex().flex_col().gap_1().child(format!(
            "Step {}{} · {} ms",
            index + 1,
            step_state(report),
            report.elapsed.as_millis()
        ));
        if let Some(failure) = report.failure {
            result = result.child(format!("Failure: {failure:?}"));
        }
        if let ChainStepOutcome::CheckFailed(error) = &report.outcome {
            result = result.child(error.to_string());
        }
        if let Some(response) = &report.response {
            result = result.child(format!(
                "HTTP {} · {} bytes{} · response content hidden",
                response.status,
                response.bytes_received,
                if response.truncated {
                    " (truncated)"
                } else {
                    ""
                },
            ));
        }
        if let Some(step) = self
            .run_definition
            .as_ref()
            .and_then(|chain| chain.steps.get(index))
        {
            for rule in report
                .captures
                .iter()
                .filter_map(|&rule| step.captures.get(rule))
            {
                let source = match &rule.source {
                    CaptureSource::Status => "HTTP status".to_owned(),
                    CaptureSource::Header { name } => format!("header {name}"),
                    CaptureSource::JsonField { pointer } => format!("JSON {pointer}"),
                };
                let sensitivity = if rule.sensitive { " · sensitive" } else { "" };
                result = result.child(format!("{} ← {source} · masked{sensitivity}", rule.name));
            }
        }
        Some(result.into_any_element())
    }
}

#[derive(Clone, Copy)]
enum StepEdit {
    Add,
    Replace,
    Toggle,
    Remove,
    Move(isize),
}

fn step_state(report: &ChainStepReport) -> &'static str {
    match report.outcome {
        ChainStepOutcome::Passed { .. } => " · Passed",
        ChainStepOutcome::RequestFailed | ChainStepOutcome::CheckFailed(_) => " · Failed",
        ChainStepOutcome::Disabled => " · Disabled",
        ChainStepOutcome::Skipped => " · Skipped",
    }
}

impl Render for ChainSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("chain-editor")
            .flex()
            .flex_col()
            .min_h_0()
            .gap_3()
            .overflow_y_scroll()
            .child("Chain name")
            .child(Input::new(&self.name).small())
            .when(self.saved, |el| {
                el.child(
                    div()
                        .text_sm()
                        .child(format!("Chain ID: {}", self.chain_id)),
                )
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("save-chain")
                            .small()
                            .primary()
                            .label("Save chain")
                            .disabled(self.running)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.save(false, window, cx)),
                            ),
                    )
                    .when(self.saved, |el| {
                        el.child(
                            Button::new("delete-chain")
                                .small()
                                .outline()
                                .disabled(self.running)
                                .label(if self.confirming_delete {
                                    "Confirm delete chain"
                                } else {
                                    "Delete chain"
                                })
                                .on_click(cx.listener(|view, _, window, cx| {
                                    if view.confirming_delete {
                                        view.save(true, window, cx);
                                    } else {
                                        view.confirming_delete = true;
                                        cx.notify();
                                    }
                                })),
                        )
                    }),
            )
            .child("Run environment")
            .child(Select::new(&self.environment).small())
            .child("Primary origin (required for each run)")
            .child(Input::new(&self.origin).small())
            .child("Additional trusted origins")
            .child(Input::new(&self.trusted).small())
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("run-chain")
                            .small()
                            .primary()
                            .label("Run chain")
                            .disabled(self.running)
                            .on_click(cx.listener(|view, _, _, cx| view.run(cx))),
                    )
                    .when(self.running, |el| {
                        el.child(
                            Button::new("cancel-chain")
                                .small()
                                .outline()
                                .label(if self.cancel.is_some() {
                                    "Cancel run"
                                } else {
                                    "Cancelling…"
                                })
                                .disabled(self.cancel.is_none())
                                .on_click(cx.listener(|view, _, _, cx| {
                                    if let Some(cancel) = view.cancel.take() {
                                        let _ = cancel.send(());
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(self.run_definition.is_some(), |el| {
                el.child(format!(
                    "{} / {} steps reported{}",
                    self.reports.len(),
                    self.run_definition
                        .as_ref()
                        .map_or(0, |chain| chain.steps.len()),
                    if self.running { " · Running" } else { "" },
                ))
            })
            .children(
                self.error
                    .as_ref()
                    .map(|error| div().text_color(cx.theme().danger).child(error.clone())),
            )
            .children(self.render_result())
            .child(self.render_steps(cx))
    }
}

pub(super) fn explorer_button(
    view: &WorkspaceView,
    cx: &Context<WorkspaceView>,
) -> impl IntoElement {
    let choices: Vec<_> = view
        .current()
        .chains
        .iter()
        .map(|chain| (chain.id.clone(), chain.name.clone()))
        .collect();
    let workspace = cx.weak_entity();
    Button::new("explorer-chains")
        .small()
        .outline()
        .label(format!("Chains ({})", choices.len()))
        .dropdown_menu(move |mut menu, _, _| {
            let workspace_new = workspace.clone();
            menu = menu.item(
                PopupMenuItem::new("New chain").on_click(move |_, window, cx| {
                    let _ = workspace_new.update(cx, |view, cx| open(view, None, window, cx));
                }),
            );
            for (id, name) in &choices {
                let (id, workspace) = (id.clone(), workspace.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                    move |_, window, cx| {
                        let _ = workspace.update(cx, |view, cx| open(view, Some(&id), window, cx));
                    },
                ));
            }
            menu
        })
}

fn open(
    view: &WorkspaceView,
    id: Option<&str>,
    window: &mut Window,
    cx: &mut Context<WorkspaceView>,
) -> Option<Entity<ChainSheet>> {
    let project = view.current();
    let chain = match id {
        Some(id) => project.chains.iter().find(|chain| chain.id == id)?.clone(),
        None => RequestChain::new("New chain"),
    };
    let workspace = cx.weak_entity();
    let editor = cx.new(|cx| ChainSheet::new(workspace, project, chain, id.is_some(), window, cx));
    window.open_sheet(cx, {
        let editor = editor.clone();
        move |sheet, _, _| {
            sheet
                .size(px(680.))
                .title("Request chain")
                .child(editor.clone())
        }
    });
    Some(editor)
}

fn collect_requests(
    requests: &[fastman_core::ApiRequest],
    folders: &[Folder],
    path: &str,
    choices: &mut Vec<Choice>,
) {
    choices.extend(requests.iter().map(|request| Choice {
        id: request.id.clone(),
        label: format!("{path} / {} {}", request.method, request.name).into(),
    }));
    for folder in folders {
        collect_requests(
            &folder.requests,
            &folder.folders,
            &format!("{path} / {}", folder.name),
            choices,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceView, open};
    use fastman_core::{ApiRequest, ChainStep, ChainStepOutcome, ProjectStore, RequestChain};
    use gpui::{AppContext as _, TestAppContext};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    fn sheet_runs_edited_rules_and_shows_a_masked_capture() {
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(check_sheet_run)
            .unwrap()
            .join()
            .unwrap();
    }

    fn check_sheet_run() {
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        let mut project = store.create("Fixture").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let request = ApiRequest::new("GET", "Capture", format!("{origin}/capture"));
        project.collections[0].requests.push(request.clone());
        let mut chain = RequestChain::new("Capture chain");
        chain.steps.push(ChainStep::new(request.id));
        project.chains.push(chain.clone());
        store.save(&project).unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 1024];
            assert_ne!(stream.read(&mut request).unwrap(), 0);
            stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\nX-Token: masked-canary\r\n\r\n").unwrap();
        });
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let (window, workspace) = cx
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| WorkspaceView::with_store(store, window, cx).unwrap())
                })
            })
            .unwrap();
        let sheet = cx
            .update_window(window, |_, window, cx| {
                workspace.update(cx, |view, cx| {
                    open(view, Some(&chain.id), window, cx).unwrap()
                })
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            sheet.update(cx, |view, cx| {
                view.origin
                    .update(cx, |input, cx| input.set_value(origin, window, cx));
                view.steps[0].assertions.update(cx, |input, cx| {
                    input.set_value(r#"[{"kind":"status","expected":200}]"#, window, cx);
                });
                view.steps[0].captures.update(cx, |input, cx| {
                    input.set_value(
                        r#"[{"name":"token","source":{"kind":"header","name":"X-Token"}}]"#,
                        window,
                        cx,
                    );
                });
                view.run(cx);
            });
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if !cx.update(|cx| sheet.read(cx).running) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "sheet did not receive execution progress"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.update(|cx| {
            let view = sheet.read(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(
                view.reports[0].outcome,
                ChainStepOutcome::Passed { status: 200 }
            );
            assert_eq!(view.reports[0].captures, [0]);
            assert!(!format!("{:?}", view.reports).contains("masked-canary"));
            assert!(view.render_result().is_some());
        });
        server.join().unwrap();
    }
}
