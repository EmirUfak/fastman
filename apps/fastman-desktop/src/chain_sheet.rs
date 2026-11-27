mod canvas;
mod inspector;
mod rules;

use crate::WorkspaceView;
use crate::theme::{dot_indicator, svg_icon};
use fastman_core::{
    CaptureRule, CaptureSource, ChainInput, ChainInputTarget, ChainRunOptions, ChainStep,
    ChainStepOutcome, ChainStepReport, Folder, Project, RequestChain,
    chain_runner::{MAX_CHAIN_STEPS, run_chain_observed},
    validate_chain_inputs,
};
use gpui_kit::base::Disableable as _;
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rules::{Rule, input};
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub(super) struct RequestChoice {
    pub(super) id: String,
    pub(super) label: SharedString,
    pub(super) name: SharedString,
    pub(super) method: SharedString,
    pub(super) url_preview: SharedString,
    pub(super) origin: Option<String>,
    pub(super) variables: Vec<String>,
}

impl SelectItem for RequestChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.label.clone()
    }
    fn value(&self) -> &String {
        &self.id
    }
}

#[derive(Clone)]
pub(super) struct EnvChoice {
    pub(super) id: String,
    pub(super) label: SharedString,
}

impl SelectItem for EnvChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.label.clone()
    }
    fn value(&self) -> &String {
        &self.id
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct StepRequestKey {
    pub(super) step_id: String,
    pub(super) request_id: String,
}

impl StepRequestKey {
    pub(super) fn new(step_id: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            step_id: step_id.into(),
            request_id: request_id.into(),
        }
    }
}

#[derive(Clone)]
pub(super) struct ResponseField {
    pub(super) step_id: String,
    pub(super) request_id: String,
    pub(super) pointer: String,
}

fn json_fields(value: &serde_json::Value, path: &str, fields: &mut Vec<String>) {
    if fields.len() >= 128 || path.len() > 512 {
        return;
    }
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                json_fields(
                    value,
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                    fields,
                );
                if fields.len() >= 128 {
                    break;
                }
            }
        }
        serde_json::Value::Array(array) => {
            for (index, value) in array.iter().enumerate() {
                json_fields(value, &format!("{path}/{index}"), fields);
                if fields.len() >= 128 {
                    break;
                }
            }
        }
        _ => fields.push(path.to_owned()),
    }
}

type RequestSelector = Entity<SelectState<SearchableVec<RequestChoice>>>;
type EnvSelector = Entity<SelectState<SearchableVec<EnvChoice>>>;

struct StepDraft {
    step: ChainStep,
    assertions: Vec<Rule>,
    captures: Vec<Rule>,
    inputs: Vec<InputDraft>,
}

impl StepDraft {
    fn new(mut step: ChainStep, window: &mut Window, cx: &mut App) -> Self {
        let assertions = std::mem::take(&mut step.assertions)
            .into_iter()
            .map(|rule| Rule::from_assertion(rule, window, cx))
            .collect();
        let (linked, manual): (Vec<_>, Vec<_>) = std::mem::take(&mut step.captures)
            .into_iter()
            .partition(|rule| matches!(rule.source, CaptureSource::LinkedJsonField { .. }));
        step.captures = linked;
        let captures = manual
            .into_iter()
            .map(|rule| Rule::from_capture(rule, window, cx))
            .collect();
        let inputs = std::mem::take(&mut step.inputs)
            .into_iter()
            .map(|binding| InputDraft::new(binding, window, cx))
            .collect();
        Self {
            step,
            assertions,
            captures,
            inputs,
        }
    }
}

pub(super) struct InputDraft {
    pub(super) binding: ChainInput,
    pub(super) variable: Entity<InputState>,
}

impl InputDraft {
    pub(super) fn new(binding: ChainInput, window: &mut Window, cx: &mut App) -> Self {
        let name = match &binding.target {
            ChainInputTarget::Variable { name } => name.clone(),
            _ => binding.capture_name.clone(),
        };
        Self {
            binding,
            variable: input("Request variable name", name, window, cx),
        }
    }

    fn build(&self, cx: &App) -> ChainInput {
        let mut binding = self.binding.clone();
        if let ChainInputTarget::Variable { name } = &mut binding.target {
            *name = self.variable.read(cx).value().trim().to_owned();
        }
        binding
    }
}

pub(super) struct ChainSheet {
    workspace: WeakEntity<WorkspaceView>,
    project_id: String,
    chain_id: String,
    saved: bool,
    original: RequestChain,
    confirming_delete: bool,
    confirming_discard: bool,
    name: Entity<InputState>,
    requests: Vec<RequestChoice>,
    request: RequestSelector,
    steps: Vec<StepDraft>,
    selected: Option<usize>,
    environment: EnvSelector,
    origin: Entity<InputState>,
    trusted: Entity<InputState>,
    show_run_settings: bool,
    canvas: Entity<canvas::ChainCanvas>,
    pub(super) reports: Vec<ChainStepReport>,
    pub(super) response_fields: BTreeMap<StepRequestKey, Vec<String>>,
    pub(super) run_definition: Option<RequestChain>,
    cancel: Option<oneshot::Sender<()>>,
    pub(super) running: bool,
    error: Option<String>,
    _request_subscription: Subscription,
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
        let detected_origin = chain
            .steps
            .iter()
            .filter(|step| step.enabled)
            .find_map(|step| {
                requests
                    .iter()
                    .find(|choice| choice.id == step.request_id)
                    .and_then(|choice| choice.origin.clone())
            })
            .or_else(|| requests.iter().find_map(|choice| choice.origin.clone()))
            .unwrap_or_default();

        let first_request = chain.steps.first().and_then(|step| {
            requests
                .iter()
                .position(|request| request.id == step.request_id)
        });
        let request = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(requests.clone()),
                first_request.map(|index| IndexPath::default().row(index)),
                window,
                cx,
            )
        });
        let request_subscription =
            cx.subscribe_in(&request, window, |view, request, event, window, cx| {
                if matches!(event, SelectEvent::Confirm(_))
                    && !view.running
                    && let Some(index) = view.selected
                    && let Some(id) = request.read(cx).selected_value().cloned()
                    && let Some(step) = view.steps.get_mut(index)
                {
                    step.step.request_id = id.clone();
                    view.maybe_prefill_origin(&id, window, cx);
                    view.clear_results(cx);
                }
            });
        let original = chain.clone();
        let mut environments = vec![EnvChoice {
            id: String::new(),
            label: "Base variables".into(),
        }];
        environments.extend(project.environments.iter().map(|env| EnvChoice {
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
        let origin = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("https://api.example.com");
            if !detected_origin.is_empty() {
                input.set_value(detected_origin, window, cx);
            }
            input
        });
        let steps: Vec<_> = chain
            .steps
            .into_iter()
            .map(|step| StepDraft::new(step, window, cx))
            .collect();
        let selected = (!steps.is_empty()).then_some(0);
        let sheet_entity = cx.entity();
        let canvas = cx.new(|cx| canvas::ChainCanvas::new(sheet_entity, cx));
        Self {
            workspace,
            project_id: project.id.clone(),
            chain_id: chain.id,
            saved,
            original,
            confirming_delete: false,
            confirming_discard: false,
            name,
            requests,
            request,
            steps,
            selected,
            environment,
            origin,
            trusted: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Optional, space-separated origins")
            }),
            show_run_settings: false,
            canvas,
            reports: Vec::new(),
            response_fields: BTreeMap::new(),
            run_definition: None,
            cancel: None,
            running: false,
            error: None,
            _request_subscription: request_subscription,
        }
    }

    fn maybe_prefill_origin(
        &mut self,
        request_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.origin.read(cx).value().trim().is_empty() {
            return;
        }
        if let Some(detected) = self
            .requests
            .iter()
            .find(|choice| choice.id == request_id)
            .and_then(|choice| choice.origin.clone())
        {
            self.origin.update(cx, |input, cx| {
                input.set_value(detected, window, cx);
            });
        }
    }

    fn build(&self, cx: &App) -> Result<RequestChain, String> {
        self.build_prefix(self.steps.len(), cx)
    }

    fn build_prefix(&self, count: usize, cx: &App) -> Result<RequestChain, String> {
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return Err("Enter a chain name.".into());
        }
        let mut steps = Vec::with_capacity(self.steps.len());
        for (index, draft) in self.steps.iter().take(count).enumerate() {
            let mut step = draft.step.clone();
            if !self
                .requests
                .iter()
                .any(|choice| choice.id == step.request_id)
            {
                return Err(format!(
                    "Node {} references a missing request. Replace or remove it.",
                    index + 1
                ));
            }
            step.assertions = draft
                .assertions
                .iter()
                .enumerate()
                .map(|(rule, draft)| {
                    draft
                        .assertion(cx)
                        .map_err(|error| format!("Node {}, check {}: {error}", index + 1, rule + 1))
                })
                .collect::<Result<_, _>>()?;
            let mut names = std::collections::BTreeSet::new();
            step.captures.retain(|rule| {
                self.steps.iter().take(count).any(|target| {
                    target.inputs.iter().any(|input| {
                        input.binding.source_step_id == draft.step.id
                            && input.binding.capture_name == rule.name
                    })
                })
            });
            names.extend(step.captures.iter().map(|rule| rule.name.clone()));
            step.captures.extend(
                draft
                    .captures
                    .iter()
                    .enumerate()
                    .map(|(rule, draft)| {
                        let capture = draft.capture(cx).map_err(|error| {
                            format!("Node {}, output {}: {error}", index + 1, rule + 1)
                        })?;
                        if !names.insert(capture.name.clone()) {
                            return Err(format!("Node {} has duplicate output names.", index + 1));
                        }
                        Ok(capture)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
            step.inputs = draft.inputs.iter().map(|input| input.build(cx)).collect();
            steps.push(step);
        }
        let chain = RequestChain {
            id: self.chain_id.clone(),
            name,
            steps,
        };
        validate_chain_inputs(&chain).map_err(|error| error.to_string())?;
        Ok(chain)
    }

    pub(super) fn has_changes(&self, cx: &App) -> bool {
        self.build(cx).map_or(true, |draft| draft != self.original)
    }

    pub(super) fn is_running(&self) -> bool {
        self.running
    }

    pub(super) fn matches_project(&self, id: &str) -> bool {
        self.project_id == id
    }

    fn save(&mut self, delete: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let result = (|| {
            let chain = if delete { None } else { Some(self.build(cx)?) };
            self.workspace
                .update(cx, |view, cx| {
                    view.save_chain_definition(
                        &self.project_id,
                        &self.chain_id,
                        chain.clone(),
                        window,
                        cx,
                    )
                })
                .map_err(|_| "Workspace closed.".to_owned())?
                .map_err(|error| error.to_string())?;
            Ok(chain)
        })();
        match result {
            Ok(None) => {
                let _ = self
                    .workspace
                    .update(cx, |view, cx| view.close_chain_editor(cx));
            }
            Ok(Some(chain)) => {
                self.saved = true;
                self.original = chain;
                self.confirming_discard = false;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        self.run_prefix(self.steps.len(), cx);
    }

    pub(super) fn run_prefix(&mut self, count: usize, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let result = (|| {
            let chain = self.build_prefix(count, cx)?;
            let origin = self.origin.read(cx).value().trim().to_owned();
            if origin.is_empty() {
                self.show_run_settings = true;
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
        self.response_fields.clear();
        self.run_definition = Some(chain);
        let worker = runtime.spawn(async move {
            run_chain_observed(
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
                |report, response| {
                    let fields = response
                        .filter(|response| !response.truncated)
                        .and_then(|response| serde_json::from_str(&response.body).ok())
                        .map_or_else(Vec::new, |value| {
                            let mut fields = Vec::new();
                            json_fields(&value, "", &mut fields);
                            fields
                        });
                    let _ = progress.send((report.clone(), fields));
                },
            )
            .await
            .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            while let Some((report, fields)) = updates.recv().await {
                if this
                    .update(cx, |view, cx| {
                        if let Some(step) = view
                            .run_definition
                            .as_ref()
                            .and_then(|chain| chain.steps.get(view.reports.len()))
                        {
                            view.response_fields
                                .insert(StepRequestKey::new(&step.id, &step.request_id), fields);
                        }
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

    pub(super) fn add_step_for_request(
        &mut self,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running || self.steps.len() >= MAX_CHAIN_STEPS {
            return;
        }
        self.maybe_prefill_origin(&request_id, window, cx);
        let next = self.steps.len();
        self.steps
            .push(StepDraft::new(ChainStep::new(request_id), window, cx));
        self.select_step(Some(next), window, cx);
        self.clear_results(cx);
    }

    pub(super) fn edit_step(
        &mut self,
        edit: StepEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running {
            return;
        }
        match edit {
            StepEdit::Add => {
                if self.steps.len() >= MAX_CHAIN_STEPS {
                    return;
                }
                self.selected = Some(self.steps.len());
                self.steps
                    .push(StepDraft::new(ChainStep::new(""), window, cx));
            }
            _ => {
                let Some(index) = self.selected.filter(|&i| i < self.steps.len()) else {
                    return;
                };
                match edit {
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
                    StepEdit::Duplicate => {
                        if self.steps.len() >= MAX_CHAIN_STEPS {
                            return;
                        }
                        if let Ok(built) = self.build_prefix(index + 1, cx)
                            && let Some(mut cloned) = built.steps.get(index).cloned()
                        {
                            cloned.id = ChainStep::new("").id;
                            let next = index + 1;
                            self.steps.insert(next, StepDraft::new(cloned, window, cx));
                            self.selected = Some(next);
                        }
                    }
                    StepEdit::Add => unreachable!(),
                }
            }
        }
        self.select_step(self.selected, window, cx);
        self.clear_results(cx);
    }

    pub(super) fn clear_results(&mut self, cx: &mut Context<Self>) {
        self.reports.clear();
        self.run_definition = None;
        self.error = None;
        cx.notify();
    }

    pub(super) fn select_step(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = index.filter(|&i| i < self.steps.len());
        self.selected = index;
        let request_index = index.and_then(|index| {
            let request_id = &self.steps[index].step.request_id;
            self.requests
                .iter()
                .position(|request| &request.id == request_id)
        });
        self.request.update(cx, |state, cx| {
            state.set_selected_index(
                request_index.map(|row| IndexPath::default().row(row)),
                window,
                cx,
            );
        });
        cx.notify();
    }

    pub(super) fn link_field(
        &mut self,
        index: usize,
        field: &ResponseField,
        target: ChainInputTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running
            || self.selected != Some(index)
            || !self.steps.get(index).is_some_and(|s| s.step.enabled)
        {
            return;
        }
        let Some(source) = self.steps[..index].iter().position(|step| {
            step.step.enabled
                && step.step.id == field.step_id
                && step.step.request_id == field.request_id
        }) else {
            return;
        };
        let source_kind = CaptureSource::LinkedJsonField {
            pointer: field.pointer.clone(),
        };
        let name = if let Some(rule) = self.steps[source]
            .step
            .captures
            .iter()
            .find(|rule| rule.source == source_kind)
        {
            rule.name.clone()
        } else {
            let name = (1..)
                .map(|suffix| format!("response_field_{suffix}"))
                .find(|name| {
                    self.steps.iter().all(|step| {
                        step.step.captures.iter().all(|rule| &rule.name != name)
                            && step
                                .captures
                                .iter()
                                .all(|rule| &rule.output_name(cx) != name)
                    })
                })
                .expect("finite capture names");
            self.steps[source].step.captures.push(CaptureRule {
                name: name.clone(),
                source: source_kind,
                required: true,
                sensitive: true,
            });
            name
        };
        let binding = ChainInput {
            source_step_id: field.step_id.clone(),
            capture_name: name,
            target,
        };
        let inputs = &mut self.steps[index].inputs;
        if let Some(existing) = inputs
            .iter_mut()
            .find(|input| input.build(cx).target == binding.target)
        {
            *existing = InputDraft::new(binding, window, cx);
        } else {
            inputs.push(InputDraft::new(binding, window, cx));
        }
        self.clear_results(cx);
    }

    fn rules_mut(&mut self, index: usize, outputs: bool) -> Option<&mut Vec<Rule>> {
        let draft = self.steps.get_mut(index)?;
        Some(if outputs {
            &mut draft.captures
        } else {
            &mut draft.assertions
        })
    }
}

#[derive(Clone, Copy)]
pub(super) enum StepEdit {
    Add,
    Toggle,
    Remove,
    Duplicate,
    Move(isize),
}

pub(super) fn step_state(report: &ChainStepReport) -> &'static str {
    match report.outcome {
        ChainStepOutcome::Passed { .. } => "Passed",
        ChainStepOutcome::RequestFailed | ChainStepOutcome::CheckFailed(_) => "Failed",
        ChainStepOutcome::Disabled => "Disabled",
        ChainStepOutcome::Skipped => "Skipped",
    }
}

pub(super) fn target_label(target: &ChainInputTarget) -> &'static str {
    match target {
        ChainInputTarget::Variable { .. } => "Request variable",
        ChainInputTarget::BearerToken => "Bearer token",
        ChainInputTarget::BasicUsername => "Basic username",
        ChainInputTarget::BasicPassword => "Basic password",
    }
}

pub(super) fn target_short_label(target: &ChainInputTarget, var_name: &str) -> String {
    match target {
        ChainInputTarget::Variable { .. } => {
            let trimmed = var_name.trim();
            if trimmed.is_empty() {
                "{{var}}".to_owned()
            } else {
                format!("{{{{{trimmed}}}}}")
            }
        }
        ChainInputTarget::BearerToken => "Bearer".to_owned(),
        ChainInputTarget::BasicUsername => "Basic user".to_owned(),
        ChainInputTarget::BasicPassword => "Basic pass".to_owned(),
    }
}

impl Render for ChainSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.has_changes(cx);
        let border = cx.theme().border;
        let has_trusted = !self.trusted.read(cx).value().trim().is_empty();
        let controls = div()
            .flex_shrink_0()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(border)
            .bg(cx.theme().background)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(280.))
                            .min_w_0()
                            .child(Input::new(&self.name).small()),
                    )
                    .child(div().w(px(1.)).h(px(18.)).bg(border))
                    .child(
                        div()
                            .w(px(148.))
                            .min_w_0()
                            .child(Select::new(&self.environment).small()),
                    )
                    .child(
                        div()
                            .w(px(210.))
                            .min_w_0()
                            .child(Input::new(&self.origin).small()),
                    )
                    .child(
                        Button::new("run-settings")
                            .ghost()
                            .small()
                            .icon(svg_icon("icons/globe.svg").xsmall())
                            .label(if self.show_run_settings {
                                "Hide origins"
                            } else {
                                "Origins"
                            })
                            .when(has_trusted, |btn| {
                                btn.child(dot_indicator(crate::theme::brand_accent(
                                    cx.theme().is_dark(),
                                )))
                            })
                            .tooltip("Configure additional trusted origins")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.show_run_settings = !view.show_run_settings;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("save-chain")
                            .outline()
                            .small()
                            .icon(svg_icon("icons/save.svg").xsmall())
                            .label("Save")
                            .when(dirty, |btn| {
                                btn.child(dot_indicator(crate::theme::brand_accent(
                                    cx.theme().is_dark(),
                                )))
                            })
                            .disabled(self.running)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.save(false, window, cx)),
                            ),
                    )
                    .when(self.saved, |el| {
                        el.child(
                            Button::new("delete-chain")
                                .ghost()
                                .small()
                                .disabled(self.running)
                                .label(if self.confirming_delete {
                                    "Confirm delete"
                                } else {
                                    "Delete"
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
                    })
                    .child(
                        Button::new("close-chain")
                            .ghost()
                            .small()
                            .label(if self.confirming_discard {
                                "Confirm discard"
                            } else {
                                "Close"
                            })
                            .disabled(self.running)
                            .on_click(cx.listener(|view, _, _, cx| {
                                if view.has_changes(cx) && !view.confirming_discard {
                                    view.confirming_discard = true;
                                    cx.notify();
                                } else {
                                    let _ = view.workspace.update(cx, |workspace, cx| {
                                        workspace.close_chain_editor(cx)
                                    });
                                }
                            })),
                    )
                    .child(div().w(px(1.)).h(px(18.)).bg(border))
                    .child(if self.running {
                        Button::new("cancel-chain")
                            .outline()
                            .small()
                            .icon(svg_icon("icons/square.svg").xsmall())
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
                            }))
                    } else {
                        Button::new("run-chain")
                            .primary()
                            .small()
                            .icon(svg_icon("icons/play.svg").xsmall())
                            .label("Run chain")
                            .on_click(cx.listener(|view, _, _, cx| view.run(cx)))
                    }),
            )
            .when(self.show_run_settings, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .pt_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Trusted origins (optional):"),
                        )
                        .child(div().w(px(360.)).child(Input::new(&self.trusted).small()))
                        .when(self.saved, |el| {
                            el.child(div().flex_1()).child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("ID: {}", self.chain_id)),
                            )
                        }),
                )
            })
            .children(self.error.as_ref().map(|error| {
                div()
                    .px_2p5()
                    .py_1()
                    .rounded(px(6.))
                    .bg(cx.theme().danger.opacity(0.12))
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(error.clone())
            }));

        div()
            .id("chain-editor")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(controls)
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("chain-editor-split")
                        .child(
                            resizable_panel()
                                .size_range(px(220.)..px(1400.))
                                .child(self.canvas.clone()),
                        )
                        .child(
                            resizable_panel()
                                .size(px(240.))
                                .size_range(px(160.)..px(480.))
                                .child(self.render_inspector(cx)),
                        ),
                ),
            )
    }
}

pub(super) fn chain_picker(view: &WorkspaceView, cx: &Context<WorkspaceView>) -> impl IntoElement {
    let choices: Vec<_> = view
        .current()
        .chains
        .iter()
        .map(|chain| (chain.id.clone(), chain.name.clone()))
        .collect();
    let workspace = cx.weak_entity();
    Button::new("chain-picker")
        .small()
        .outline()
        .label(format!("Chains ({})", choices.len()))
        .dropdown_menu(move |mut menu, _, _| {
            let workspace_new = workspace.clone();
            menu = menu.item(
                PopupMenuItem::new("New chain").on_click(move |_, window, cx| {
                    let _ = workspace_new
                        .update(cx, |view, cx| view.open_chain_editor(None, window, cx));
                }),
            );
            for (id, name) in &choices {
                let (id, workspace) = (id.clone(), workspace.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                    move |_, window, cx| {
                        let _ = workspace
                            .update(cx, |view, cx| view.open_chain_editor(Some(&id), window, cx));
                    },
                ));
            }
            menu
        })
}

pub(super) fn open(
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
    Some(cx.new(|cx| ChainSheet::new(workspace, project, chain, id.is_some(), window, cx)))
}

fn extract_origin(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let scheme_end = trimmed.find("://")?;
    let scheme = &trimmed[..scheme_end];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let rest = &trimmed[scheme_end + 3..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.is_empty() || host.contains("{{") {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

fn collect_requests(
    requests: &[fastman_core::ApiRequest],
    folders: &[Folder],
    path: &str,
    choices: &mut Vec<RequestChoice>,
) {
    choices.extend(requests.iter().map(|request| {
        let url_preview = if request.url.trim().is_empty() {
            "(no URL)".to_owned()
        } else {
            request.url.trim().to_owned()
        };
        RequestChoice {
            id: request.id.clone(),
            label: format!("{}  ·  {path}", request.name).into(),
            name: request.name.clone().into(),
            method: request.method.clone().into(),
            origin: extract_origin(&request.url),
            url_preview: url_preview.into(),
            variables: fastman_core::template::inspect_request(request, &BTreeMap::new())
                .missing_variables,
        }
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
    use super::{ResponseField, Rule, StepRequestKey, WorkspaceView};
    use fastman_core::{
        ApiRequest, AssertionRule, CaptureRule, CaptureSource, ChainInputTarget, ChainStep,
        ChainStepOutcome, ProjectStore, RequestChain,
    };
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
        chain.steps.push(ChainStep::new(request.id.clone()));
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
            let body = r#"{"auth":{"access/token":"masked-canary"}}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\nX-Token: masked-canary\r\n\r\n{body}", body.len()).unwrap();
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
                    view.open_chain_editor(Some(&chain.id), window, cx);
                    view.chain_editor.clone().unwrap()
                })
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            sheet.update(cx, |view, cx| {
                // Origin should already be auto-detected from the first request!
                assert_eq!(view.origin.read(cx).value().as_ref(), origin.as_str());
                view.steps[0].assertions.push(Rule::from_assertion(
                    AssertionRule::Status { expected: 200 },
                    window,
                    cx,
                ));
                view.steps[0].captures.push(Rule::from_capture(
                    CaptureRule {
                        name: "token".into(),
                        source: CaptureSource::Header {
                            name: "X-Token".into(),
                        },
                        required: true,
                        sensitive: true,
                    },
                    window,
                    cx,
                ));
                view.run_prefix(1, cx);
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
            assert!(view.render_result(0, cx).is_some());
        });
        cx.update_window(window, |_, window, cx| {
            sheet.update(cx, |view, cx| {
                let source = &view.steps[0].step;
                let key = StepRequestKey::new(&source.id, &source.request_id);
                let fields = &view.response_fields[&key];
                assert_eq!(fields, &["/auth/access~1token"]);
                assert_eq!(view.reports.len(), 1);
                let field = ResponseField {
                    step_id: source.id.clone(),
                    request_id: source.request_id.clone(),
                    pointer: fields[0].clone(),
                };
                view.select_step(Some(1), window, cx);
                view.link_field(1, &field, ChainInputTarget::BearerToken, window, cx);
                let saved = view.build(cx).unwrap();
                let link = &saved.steps[1].inputs[0];
                assert_eq!(link.target, ChainInputTarget::BearerToken);
                assert!(saved.steps[0].captures.iter().any(|rule| {
                    rule.name == link.capture_name
                        && rule.sensitive
                        && matches!(&rule.source, CaptureSource::LinkedJsonField { pointer } if pointer == &field.pointer)
                }));
            });
        })
        .unwrap();
        server.join().unwrap();
    }
}
