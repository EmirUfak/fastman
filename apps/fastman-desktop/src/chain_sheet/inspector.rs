use super::{
    ChainSheet, InputDraft, ResponseField, StepEdit, StepRequestKey,
    rules::{Kind, Rule},
    step_state, target_label,
};
use crate::theme::{count_pill, method_badge, status_badge, svg_icon};
use fastman_core::{AssertionRule, CaptureSource, ChainInput, ChainInputTarget, ChainStepOutcome};
use gpui_kit::base::Disableable as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::Input,
    menu::{DropdownMenu as _, PopupMenuItem},
    select::Select,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const TARGET_CHOICES: [(&str, ChainInputTarget); 4] = [
    (
        "Request variable",
        ChainInputTarget::Variable {
            name: String::new(),
        },
    ),
    ("Bearer token", ChainInputTarget::BearerToken),
    ("Basic username", ChainInputTarget::BasicUsername),
    ("Basic password", ChainInputTarget::BasicPassword),
];

#[derive(Clone)]
enum EarlierValueSource {
    ManualCapture {
        step_id: String,
        capture_name: String,
        label: String,
    },
    DiscoveredJson {
        field: ResponseField,
        label: String,
    },
}

fn wire_color(is_dark: bool) -> Hsla {
    if is_dark {
        rgb(0x49c7ef).into()
    } else {
        rgb(0x086ead).into()
    }
}

fn toggle_chip(
    id: &'static str,
    label: &'static str,
    active: bool,
    tone: Hsla,
    disabled: bool,
    tooltip: &'static str,
    cx: &App,
) -> Stateful<Div> {
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    div()
        .id(id)
        .h(px(24.))
        .px_2()
        .rounded(px(6.))
        .border_1()
        .border_color(if active {
            tone.opacity(0.45)
        } else {
            border.opacity(0.8)
        })
        .bg(if active {
            tone.opacity(0.14)
        } else {
            cx.theme().background
        })
        .flex()
        .items_center()
        .gap_1p5()
        .text_xs()
        .font_weight(if active {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .text_color(if active { tone } else { muted })
        .when(disabled, |el| el.opacity(0.5))
        .when(!disabled, |el| {
            el.cursor_pointer().hover(|s| {
                s.border_color(if active { tone } else { border })
                    .bg(if active {
                        tone.opacity(0.20)
                    } else {
                        cx.theme().secondary.opacity(0.6)
                    })
            })
        })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip).build(window, cx)
        })
        .when(active, |el| {
            el.child(div().w(px(5.)).h(px(5.)).rounded_full().bg(tone))
        })
        .child(label)
}

fn rule_kind_badge(tag: &'static str, cx: &App) -> Div {
    div()
        .w(px(48.))
        .h(px(20.))
        .flex_shrink_0()
        .rounded(px(4.))
        .bg(cx.theme().secondary)
        .flex()
        .items_center()
        .justify_center()
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(tag)
}

fn empty_hint(message: &str, cx: &App) -> Div {
    div()
        .px_3()
        .py_2p5()
        .rounded(px(6.))
        .border_1()
        .border_color(cx.theme().border.opacity(0.6))
        .bg(cx.theme().secondary.opacity(0.16))
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(message.to_owned())
}

impl ChainSheet {
    pub(super) fn render_inspector(&self, cx: &Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let panel = div()
            .id("chain-node-inspector")
            .size_full()
            .border_t_1()
            .border_color(border)
            .bg(cx.theme().background)
            .flex()
            .flex_col();

        let Some(index) = self.selected.filter(|&i| i < self.steps.len()) else {
            return panel
                .p_6()
                .justify_center()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("No step selected"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Select a step on the canvas to edit inputs and checks."),
                )
                .into_any_element();
        };

        let draft = &self.steps[index];
        let request = self
            .requests
            .iter()
            .find(|choice| choice.id == draft.step.request_id);
        let always_run = draft.step.always_run;
        let isolated = draft.step.isolate_cookies;
        let enabled = draft.step.enabled;
        let warning = cx.theme().warning;
        let wire = wire_color(cx.theme().is_dark());
        let step_count = self.steps.len();

        // Single-row step header aligned to outer px_4 rail
        let header = div()
            .h(px(44.))
            .flex_shrink_0()
            .px_4()
            .border_b_1()
            .border_color(border)
            .bg(cx.theme().secondary.opacity(0.22))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .h(px(20.))
                    .px_1p5()
                    .rounded(px(4.))
                    .bg(cx.theme().secondary)
                    .flex()
                    .items_center()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(format!("#{:02}", index + 1)),
            )
            .children(request.map(|r| method_badge(r.method.as_ref(), None, cx)))
            .child(
                div().w(px(288.)).min_w_0().child(
                    Select::new(&self.request)
                        .small()
                        .disabled(self.running)
                        .placeholder("Pick a saved request…"),
                ),
            )
            .child(
                Button::new("inspector-run-to")
                    .outline()
                    .small()
                    .icon(svg_icon("icons/play.svg").xsmall())
                    .label("Run to here")
                    .disabled(self.running || !enabled)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.run_prefix(index + 1, cx);
                    })),
            )
            .children(self.render_inline_result_badge(index, cx))
            .child(div().flex_1())
            .child(
                toggle_chip(
                    "toggle-always-run",
                    "Cleanup",
                    always_run,
                    warning,
                    self.running,
                    "Always run even if an earlier step fails (e.g. DELETE cleanup)",
                    cx,
                )
                .when(!self.running, |el| {
                    el.on_click(cx.listener(move |view, _, _, cx| {
                        if let Some(step) = view.steps.get_mut(index) {
                            step.step.always_run = !always_run;
                            view.clear_results(cx);
                        }
                    }))
                }),
            )
            .child(
                toggle_chip(
                    "isolate-node-cookies",
                    if isolated { "No cookies" } else { "Cookies" },
                    isolated,
                    wire,
                    self.running,
                    "Isolate this step from the shared session cookie jar",
                    cx,
                )
                .when(!self.running, |el| {
                    el.on_click(cx.listener(move |view, _, _, cx| {
                        if let Some(step) = view.steps.get_mut(index) {
                            step.step.isolate_cookies = !isolated;
                            view.clear_results(cx);
                        }
                    }))
                }),
            )
            .child(
                toggle_chip(
                    "toggle-chain-step",
                    if enabled { "Enabled" } else { "Disabled" },
                    !enabled,
                    muted,
                    self.running,
                    "Skip or include this step when running the chain",
                    cx,
                )
                .when(!self.running, |el| {
                    el.on_click(cx.listener(|view, _, window, cx| {
                        view.edit_step(StepEdit::Toggle, window, cx);
                    }))
                }),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .child(
                        Button::new("chain-step-up")
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/arrow-left.svg").xsmall())
                            .tooltip("Move step left")
                            .disabled(self.running || index == 0)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.edit_step(StepEdit::Move(-1), window, cx);
                            })),
                    )
                    .child(
                        Button::new("chain-step-down")
                            .ghost()
                            .xsmall()
                            .label("→")
                            .tooltip("Move step right")
                            .disabled(self.running || index + 1 >= step_count)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.edit_step(StepEdit::Move(1), window, cx);
                            })),
                    )
                    .child(
                        Button::new("duplicate-chain-step")
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/copy.svg").xsmall())
                            .tooltip("Duplicate step")
                            .disabled(
                                self.running
                                    || step_count >= fastman_core::chain_runner::MAX_CHAIN_STEPS,
                            )
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.edit_step(StepEdit::Duplicate, window, cx);
                            })),
                    )
                    .child(
                        Button::new("remove-chain-step")
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/trash.svg").xsmall())
                            .tooltip("Remove step")
                            .disabled(self.running)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.edit_step(StepEdit::Remove, window, cx);
                            })),
                    ),
            );

        // 2 balanced 50/50 columns (Inputs + Outputs on left, Checks on right)
        let columns = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .id("inspector-col-send")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_4()
                    .py_3()
                    .border_r_1()
                    .border_color(border)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(self.render_send_with_section(index, cx))
                    .child(
                        div()
                            .pt_2p5()
                            .border_t_1()
                            .border_color(border.opacity(0.6))
                            .child(self.render_outputs_column(index, cx)),
                    ),
            )
            .child(
                div()
                    .id("inspector-col-expect")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_4()
                    .py_3()
                    .child(self.render_expect_section(index, cx)),
            );

        panel
            .child(header)
            .children(self.render_failure_banner(index, cx))
            .child(columns)
            .into_any_element()
    }

    fn collect_earlier_sources(&self, index: usize, cx: &App) -> Vec<EarlierValueSource> {
        let mut out = Vec::new();
        for (source_idx, draft) in self.steps[..index]
            .iter()
            .enumerate()
            .filter(|(_, s)| s.step.enabled)
        {
            let step_name = self
                .requests
                .iter()
                .find(|c| c.id == draft.step.request_id)
                .map(|c| c.name.as_ref())
                .unwrap_or("Step");

            for rule in &draft.captures {
                let name = rule.output_name(cx);
                if !name.is_empty() {
                    out.push(EarlierValueSource::ManualCapture {
                        step_id: draft.step.id.clone(),
                        capture_name: name.clone(),
                        label: format!("#{:02} {step_name}  ·  {name}", source_idx + 1),
                    });
                }
            }

            let key = StepRequestKey::new(&draft.step.id, &draft.step.request_id);
            if let Some(fields) = self.response_fields.get(&key) {
                for pointer in fields.iter().take(20) {
                    let ptr_lbl = if pointer.is_empty() {
                        "(JSON root)"
                    } else {
                        pointer.as_str()
                    };
                    out.push(EarlierValueSource::DiscoveredJson {
                        field: ResponseField {
                            step_id: draft.step.id.clone(),
                            request_id: draft.step.request_id.clone(),
                            pointer: pointer.clone(),
                        },
                        label: format!("#{:02} {step_name}  ·  {ptr_lbl}", source_idx + 1),
                    });
                }
            }
        }
        out
    }

    fn column_header(
        &self,
        title: &'static str,
        count: usize,
        action: Option<AnyElement>,
        cx: &App,
    ) -> Div {
        div()
            .h(px(26.))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(title),
                    )
                    .when(count > 0, |el| el.child(count_pill(count, cx))),
            )
            .children(action)
    }

    fn render_send_with_section(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        let wire_accent = wire_color(cx.theme().is_dark());

        let draft = &self.steps[index];
        let earlier = self.collect_earlier_sources(index, cx);

        let wired_vars: std::collections::BTreeSet<String> = draft
            .inputs
            .iter()
            .filter_map(|i| match &i.binding.target {
                ChainInputTarget::Variable { .. } => {
                    let v = i.variable.read(cx).value().trim().to_owned();
                    (!v.is_empty()).then_some(v)
                }
                _ => None,
            })
            .collect();
        let missing_vars: Vec<String> = self
            .requests
            .iter()
            .find(|c| c.id == draft.step.request_id)
            .map(|c| {
                c.variables
                    .iter()
                    .filter(|v| !wired_vars.contains(*v))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let header_action = if index > 0 && !earlier.is_empty() {
            let weak = cx.weak_entity();
            let default_var = missing_vars.first().cloned();
            let sources = earlier.clone();
            Some(
                Button::new("use-earlier-value")
                    .outline()
                    .xsmall()
                    .icon(svg_icon("icons/plus.svg").xsmall())
                    .label("Pick value")
                    .disabled(self.running || !draft.step.enabled)
                    .dropdown_menu(move |mut menu, _, _| {
                        for src in &sources {
                            let (src, weak, default_var) =
                                (src.clone(), weak.clone(), default_var.clone());
                            let label = match &src {
                                EarlierValueSource::ManualCapture { label, .. }
                                | EarlierValueSource::DiscoveredJson { label, .. } => label.clone(),
                            };
                            menu = menu.item(PopupMenuItem::new(label).on_click(
                                move |_, window, cx| {
                                    let _ = weak.update(cx, |view, cx| {
                                        let target = if let Some(var) = &default_var {
                                            ChainInputTarget::Variable { name: var.clone() }
                                        } else {
                                            match &src {
                                                EarlierValueSource::ManualCapture {
                                                    capture_name,
                                                    ..
                                                } => ChainInputTarget::Variable {
                                                    name: capture_name.clone(),
                                                },
                                                EarlierValueSource::DiscoveredJson {
                                                    field,
                                                    ..
                                                } => {
                                                    if field.pointer.contains("token") {
                                                        ChainInputTarget::BearerToken
                                                    } else {
                                                        let guessed = field
                                                            .pointer
                                                            .rsplit('/')
                                                            .next()
                                                            .filter(|s| !s.is_empty())
                                                            .unwrap_or("value")
                                                            .to_owned();
                                                        ChainInputTarget::Variable { name: guessed }
                                                    }
                                                }
                                            }
                                        };
                                        apply_earlier_source(view, index, &src, target, window, cx);
                                    });
                                },
                            ));
                        }
                        menu
                    })
                    .into_any_element(),
            )
        } else {
            None
        };

        let mut section = div().flex().flex_col().gap_2().child(self.column_header(
            "Inputs",
            draft.inputs.len(),
            header_action,
            cx,
        ));

        for (v_idx, var_name) in missing_vars.iter().enumerate() {
            let sources = earlier.clone();
            let weak = cx.weak_entity();
            let var_for_menu = var_name.clone();
            section = section.child(
                div()
                    .px_2p5()
                    .py_1p5()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(warning.opacity(0.4))
                    .bg(warning.opacity(0.08))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .min_w_0()
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(warning)
                                    .child(format!("{{{{{var_name}}}}}")),
                            )
                            .child(div().text_xs().text_color(muted).child("unwired")),
                    )
                    .when(!sources.is_empty(), |el| {
                        el.child(
                            Button::new(("wire-missing-var", v_idx))
                                .outline()
                                .xsmall()
                                .label("Bind")
                                .disabled(self.running)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for src in &sources {
                                        let (src, var_name, weak) =
                                            (src.clone(), var_for_menu.clone(), weak.clone());
                                        let label = match &src {
                                            EarlierValueSource::ManualCapture { label, .. }
                                            | EarlierValueSource::DiscoveredJson {
                                                label, ..
                                            } => label.clone(),
                                        };
                                        menu = menu.item(PopupMenuItem::new(label).on_click(
                                            move |_, window, cx| {
                                                let _ = weak.update(cx, |view, cx| {
                                                    apply_earlier_source(
                                                        view,
                                                        index,
                                                        &src,
                                                        ChainInputTarget::Variable {
                                                            name: var_name.clone(),
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            },
                                        ));
                                    }
                                    menu
                                }),
                        )
                    }),
            );
        }

        for (row, input_draft) in draft.inputs.iter().enumerate() {
            let source_pos = self
                .steps
                .iter()
                .position(|s| s.step.id == input_draft.binding.source_step_id);
            let field_label = source_pos
                .and_then(|pos| {
                    self.steps[pos]
                        .step
                        .captures
                        .iter()
                        .find(|rule| rule.name == input_draft.binding.capture_name)
                })
                .and_then(|rule| match &rule.source {
                    CaptureSource::LinkedJsonField { pointer } => Some(if pointer.is_empty() {
                        "(JSON root)"
                    } else {
                        pointer
                    }),
                    _ => None,
                })
                .unwrap_or(&input_draft.binding.capture_name);
            let source_tag =
                source_pos.map_or_else(|| "Missing".to_owned(), |pos| format!("#{:02}", pos + 1));
            let weak = cx.weak_entity();
            let target = &input_draft.binding.target;

            section = section.child(
                div()
                    .px_2p5()
                    .py_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(border.opacity(0.85))
                    .bg(cx.theme().secondary.opacity(0.2))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .bg(wire_accent.opacity(0.15))
                                            .text_color(wire_accent)
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(10.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(source_tag),
                                    )
                                    .child(
                                        div()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate()
                                            .child(field_label.to_owned()),
                                    ),
                            )
                            .child(
                                Button::new(("unlink-output", row))
                                    .xsmall()
                                    .ghost()
                                    .icon(svg_icon("icons/trash.svg").xsmall())
                                    .tooltip("Remove input")
                                    .disabled(self.running)
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        if let Some(step) = view.steps.get_mut(index)
                                            && row < step.inputs.len()
                                        {
                                            step.inputs.remove(row);
                                            view.clear_results(cx);
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().text_xs().text_color(muted).child("→"))
                            .child(
                                Button::new(("binding-target", row))
                                    .xsmall()
                                    .outline()
                                    .label(target_label(target))
                                    .disabled(self.running)
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for (label, target) in &TARGET_CHOICES {
                                            let (weak, target) = (weak.clone(), target.clone());
                                            menu = menu.item(PopupMenuItem::new(*label).on_click(
                                                move |_, _, cx| {
                                                    let _ = weak.update(cx, |view, cx| {
                                                        if !view.running
                                                            && let Some(input) = view
                                                                .steps
                                                                .get_mut(index)
                                                                .and_then(|s| s.inputs.get_mut(row))
                                                        {
                                                            input.binding.target = target.clone();
                                                            view.clear_results(cx);
                                                        }
                                                    });
                                                },
                                            ));
                                        }
                                        menu
                                    }),
                            )
                            .when(matches!(target, ChainInputTarget::Variable { .. }), |el| {
                                el.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Input::new(&input_draft.variable).xsmall()),
                                )
                            }),
                    ),
            );
        }

        if draft.inputs.is_empty() && missing_vars.is_empty() {
            if index == 0 {
                section = section.child(empty_hint(
                    "First step — reads from project and environment variables.",
                    cx,
                ));
            } else if earlier.is_empty() {
                section = section.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(border.opacity(0.6))
                        .bg(cx.theme().secondary.opacity(0.16))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Run earlier steps once to pick response JSON fields."),
                        )
                        .child(
                            Button::new("run-earlier-for-fields")
                                .outline()
                                .xsmall()
                                .icon(svg_icon("icons/play.svg").xsmall())
                                .label(format!("Run 1–{index}"))
                                .disabled(self.running)
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.run_prefix(index, cx);
                                })),
                        ),
                );
            } else {
                section = section.child(empty_hint(
                    "No values wired yet — click + Pick value to reuse data from earlier steps.",
                    cx,
                ));
            }
        }

        section.into_any_element()
    }

    fn render_expect_section(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let draft = &self.steps[index];

        let quick_actions = div()
            .flex()
            .items_center()
            .gap_1()
            .children([200u16, 201, 204, 404].map(|code| {
                Button::new(("quick-status", usize::from(code)))
                    .ghost()
                    .xsmall()
                    .label(code.to_string())
                    .tooltip(format!("Expect HTTP {code}"))
                    .disabled(self.running)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if let Some(step) = view.steps.get_mut(index) {
                            step.assertions.push(Rule::from_assertion(
                                AssertionRule::Status { expected: code },
                                window,
                                cx,
                            ));
                            view.clear_results(cx);
                        }
                    }))
            }))
            .child(div().w(px(1.)).h(px(12.)).mx_0p5().bg(border))
            .child(
                Button::new("quick-json-check")
                    .outline()
                    .xsmall()
                    .label("+ JSON")
                    .disabled(self.running)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if let Some(rules) = view.rules_mut(index, false) {
                            rules.push(Rule::empty(Kind::Json, window, cx));
                            view.clear_results(cx);
                        }
                    })),
            )
            .child(
                Button::new("quick-header-check")
                    .outline()
                    .xsmall()
                    .label("+ Header")
                    .disabled(self.running)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if let Some(rules) = view.rules_mut(index, false) {
                            rules.push(Rule::empty(Kind::Header, window, cx));
                            view.clear_results(cx);
                        }
                    })),
            )
            .child(
                Button::new("quick-speed-check")
                    .outline()
                    .xsmall()
                    .label("+ Speed")
                    .disabled(self.running)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if let Some(rules) = view.rules_mut(index, false) {
                            rules.push(Rule::empty(Kind::Latency, window, cx));
                            view.clear_results(cx);
                        }
                    })),
            )
            .into_any_element();

        let mut section = div().flex().flex_col().gap_2().child(self.column_header(
            "Checks",
            draft.assertions.len(),
            Some(quick_actions),
            cx,
        ));

        if draft.assertions.is_empty() {
            section = section.child(empty_hint("No checks yet — any HTTP response passes.", cx));
        }

        // Single-row inline assertions
        for (row, form) in draft.assertions.iter().enumerate() {
            section = section.child(
                div()
                    .px_2()
                    .py_1p5()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(border.opacity(0.85))
                    .bg(cx.theme().secondary.opacity(0.2))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(rule_kind_badge(form.kind.short_tag(), cx))
                    .child(form.render_assertion_fields(cx))
                    .child(
                        Button::new(("check", row))
                            .xsmall()
                            .ghost()
                            .icon(svg_icon("icons/trash.svg").xsmall())
                            .tooltip("Remove check")
                            .disabled(self.running)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if let Some(rules) = view.rules_mut(index, false)
                                    && row < rules.len()
                                {
                                    rules.remove(row);
                                    view.clear_results(cx);
                                }
                            })),
                    ),
            );
        }

        section.into_any_element()
    }

    fn render_outputs_column(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let success = cx.theme().success;
        let draft = &self.steps[index];
        let total_outputs = draft.step.captures.len() + draft.captures.len();
        let weak = cx.weak_entity();

        let add_output_btn = Button::new("add-output")
            .xsmall()
            .outline()
            .icon(svg_icon("icons/plus.svg").xsmall())
            .label("Custom")
            .disabled(self.running)
            .dropdown_menu(move |mut menu, _, _| {
                for &kind in &Kind::OUTPUTS {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new(kind.label()).on_click(
                        move |_, window, cx| {
                            let _ = weak.update(cx, |view, cx| {
                                if !view.running
                                    && let Some(rules) = view.rules_mut(index, true)
                                {
                                    rules.push(Rule::empty(kind, window, cx));
                                    view.clear_results(cx);
                                }
                            });
                        },
                    ));
                }
                menu
            })
            .into_any_element();

        let mut section = div().flex().flex_col().gap_2().child(self.column_header(
            "Outputs",
            total_outputs,
            Some(add_output_btn),
            cx,
        ));

        if !draft.step.captures.is_empty() {
            section = section.child(div().flex().flex_wrap().gap_1p5().children(
                draft.step.captures.iter().map(|rule| {
                    let ptr = match &rule.source {
                        CaptureSource::LinkedJsonField { pointer } if !pointer.is_empty() => {
                            pointer.as_str()
                        }
                        CaptureSource::LinkedJsonField { .. } => "(JSON root)",
                        _ => rule.name.as_str(),
                    };
                    div()
                        .px_2()
                        .py(px(2.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(success.opacity(0.3))
                        .bg(success.opacity(0.12))
                        .text_color(success)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(11.))
                        .child(format!("→ {ptr}"))
                }),
            ));
        }

        for (row, form) in draft.captures.iter().enumerate() {
            section = section.child(
                div()
                    .px_2()
                    .py_1p5()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(border.opacity(0.85))
                    .bg(cx.theme().secondary.opacity(0.2))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .child(rule_kind_badge(form.kind.short_tag(), cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Button::new(("output-required", row))
                                            .xsmall()
                                            .ghost()
                                            .label(if form.required { "Req" } else { "Opt" })
                                            .tooltip("Toggle whether missing value fails this step")
                                            .disabled(self.running)
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                if let Some(form) = view
                                                    .steps
                                                    .get_mut(index)
                                                    .and_then(|s| s.captures.get_mut(row))
                                                {
                                                    form.required = !form.required;
                                                    view.clear_results(cx);
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new(("output-sensitive", row))
                                            .xsmall()
                                            .ghost()
                                            .label(if form.sensitive { "Mask" } else { "Plain" })
                                            .tooltip("Mask captured value in logs and reports")
                                            .disabled(self.running)
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                if let Some(form) = view
                                                    .steps
                                                    .get_mut(index)
                                                    .and_then(|s| s.captures.get_mut(row))
                                                {
                                                    form.sensitive = !form.sensitive;
                                                    view.clear_results(cx);
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new(("output", row))
                                            .xsmall()
                                            .ghost()
                                            .icon(svg_icon("icons/trash.svg").xsmall())
                                            .tooltip("Remove output")
                                            .disabled(self.running)
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                if let Some(rules) = view.rules_mut(index, true)
                                                    && row < rules.len()
                                                {
                                                    rules.remove(row);
                                                    view.clear_results(cx);
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .child(form.render_capture_fields(cx)),
            );
        }

        if total_outputs == 0 {
            section = section.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("JSON fields link automatically when picked in later steps."),
            );
        }

        section.into_any_element()
    }

    fn render_inline_result_badge(&self, index: usize, cx: &App) -> Option<AnyElement> {
        let report = self.reports.get(index)?;
        let tone = match report.outcome {
            ChainStepOutcome::Passed { .. } => cx.theme().success,
            ChainStepOutcome::RequestFailed | ChainStepOutcome::CheckFailed(_) => cx.theme().danger,
            _ => cx.theme().muted_foreground,
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tone)
                        .child(step_state(report)),
                )
                .children(
                    report
                        .response
                        .as_ref()
                        .map(|resp| status_badge(resp.status, "", cx)),
                )
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} ms", report.elapsed.as_millis())),
                )
                .into_any_element(),
        )
    }

    fn render_failure_banner(&self, index: usize, cx: &App) -> Option<AnyElement> {
        let report = self.reports.get(index)?;
        let msg = if let ChainStepOutcome::CheckFailed(error) = &report.outcome {
            error.to_string()
        } else if report.failure == Some(fastman_core::ChainFailure::Capture) {
            "An earlier value needed by this step wasn't available.".to_owned()
        } else {
            let failure = report.failure?;
            format!("Failure: {failure:?}")
        };
        Some(
            div()
                .px_4()
                .py_1p5()
                .border_b_1()
                .border_color(cx.theme().danger.opacity(0.3))
                .bg(cx.theme().danger.opacity(0.08))
                .text_xs()
                .text_color(cx.theme().danger)
                .child(msg)
                .into_any_element(),
        )
    }

    #[cfg(test)]
    pub(super) fn render_result(&self, index: usize, cx: &App) -> Option<AnyElement> {
        self.render_inline_result_badge(index, cx)
    }
}

fn apply_earlier_source(
    view: &mut ChainSheet,
    index: usize,
    source: &EarlierValueSource,
    target: ChainInputTarget,
    window: &mut Window,
    cx: &mut Context<ChainSheet>,
) {
    match source {
        EarlierValueSource::ManualCapture {
            step_id,
            capture_name,
            ..
        } => {
            if !view.running
                && let Some(step) = view.steps.get_mut(index)
            {
                let binding = ChainInput {
                    source_step_id: step_id.clone(),
                    capture_name: capture_name.clone(),
                    target,
                };
                step.inputs.push(InputDraft::new(binding, window, cx));
                view.clear_results(cx);
            }
        }
        EarlierValueSource::DiscoveredJson { field, .. } => {
            view.link_field(index, field, target, window, cx);
        }
    }
}
