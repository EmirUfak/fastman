use super::{ChainSheet, StepEdit, step_state, target_short_label};
use crate::theme::{method_badge, svg_icon};
use fastman_core::ChainStepOutcome;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;

const NODE_WIDTH: f32 = 276.0;
const NODE_HEIGHT: f32 = 148.0;
const NODE_GAP_X: f32 = 84.0;
const WORLD_PAD_X: f32 = 96.0;
const WORLD_PAD_TOP: f32 = 88.0;
const MIN_ZOOM: f32 = 0.55;
const MAX_ZOOM: f32 = 1.40;

#[derive(Clone)]
struct SequentialEdge {
    from_x: f32,
    from_y: f32,
    to_x: f32,
    color: Hsla,
    active: bool,
}

#[derive(Clone)]
struct DataLinkWire {
    from_x: f32,
    from_y: f32,
    to_x: f32,
    to_y: f32,
    top_y: f32,
    color: Hsla,
}

pub(super) struct ChainCanvas {
    sheet: WeakEntity<ChainSheet>,
    scroll_handle: ScrollHandle,
    zoom: f32,
    dragging: Option<(Point<Pixels>, Point<Pixels>)>,
    _sheet_observation: Subscription,
}

impl ChainCanvas {
    pub(super) fn new(sheet: Entity<ChainSheet>, cx: &mut Context<Self>) -> Self {
        let observation = cx.observe(&sheet, |_, _, cx| cx.notify());
        Self {
            sheet: sheet.downgrade(),
            scroll_handle: ScrollHandle::new(),
            zoom: 1.0,
            dragging: None,
            _sheet_observation: observation,
        }
    }

    fn set_zoom(&mut self, next_zoom: f32) {
        let clamped = next_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (clamped - self.zoom).abs() < 0.001 {
            return;
        }
        let ratio = clamped / self.zoom;
        let current = self.scroll_handle.offset();
        self.zoom = clamped;
        self.scroll_handle
            .set_offset(point(current.x * ratio, px(0.)));
    }

    fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.scroll_handle.set_offset(point(px(0.), px(0.)));
    }
}

impl Render for ChainCanvas {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(sheet_entity) = self.sheet.upgrade() else {
            return div().flex_1().into_any_element();
        };
        let sheet = sheet_entity.read(cx);

        let is_dark = cx.theme().is_dark();
        let accent = crate::theme::brand_accent(is_dark);
        let wire_accent: Hsla = if is_dark {
            rgb(0x49c7ef).into()
        } else {
            rgb(0x086ead).into()
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let card_bg = cx.theme().background;
        let canvas_bg = cx.theme().sidebar;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let warning = cx.theme().warning;
        let info = cx.theme().info;

        let scale = self.zoom;
        let node_w = NODE_WIDTH * scale;
        let node_h = NODE_HEIGHT * scale;
        let gap_x = NODE_GAP_X * scale;
        let step_pitch = node_w + gap_x;
        let origin_x = WORLD_PAD_X * scale;
        let origin_y = WORLD_PAD_TOP * scale;
        let port_y = origin_y + node_h * 0.5;
        let port_d = (10.0 * scale).clamp(6.0, 12.0);
        let port_r = port_d * 0.5;
        let compact = scale < 0.75;

        let add_w = 196.0 * scale;
        let add_h = 88.0 * scale;
        let world_w =
            (origin_x + sheet.steps.len() as f32 * step_pitch + add_w + WORLD_PAD_X).max(1000.0);

        let step_index_by_id: HashMap<&str, usize> = sheet
            .steps
            .iter()
            .enumerate()
            .map(|(idx, draft)| (draft.step.id.as_str(), idx))
            .collect();
        let request_by_id: HashMap<&str, &super::RequestChoice> = sheet
            .requests
            .iter()
            .map(|choice| (choice.id.as_str(), choice))
            .collect();

        // Build sequential execution edges (straight horizontal GPU quads)
        let mut seq_edges = Vec::with_capacity(sheet.steps.len());
        for index in 0..sheet.steps.len() {
            let from_x = origin_x + index as f32 * step_pitch + node_w;
            let to_x = origin_x + (index + 1) as f32 * step_pitch;
            let report = sheet.reports.get(index);
            let is_last_to_add = index + 1 == sheet.steps.len();
            let (color, active) = if is_last_to_add {
                (border.opacity(0.75), false)
            } else {
                match report.map(|r| &r.outcome) {
                    Some(ChainStepOutcome::Passed { .. }) => (success, true),
                    Some(ChainStepOutcome::RequestFailed | ChainStepOutcome::CheckFailed(_)) => {
                        (danger, true)
                    }
                    _ if sheet.selected == Some(index) || sheet.selected == Some(index + 1) => {
                        (accent, true)
                    }
                    _ => (border, false),
                }
            };
            seq_edges.push(SequentialEdge {
                from_x,
                from_y: port_y,
                to_x,
                color,
                active,
            });
        }

        // Build orthogonal data-flow wires above nodes (3 GPU quads each)
        let mut data_wires = Vec::new();
        let mut lane = 0usize;
        for (target_idx, draft) in sheet.steps.iter().enumerate() {
            let mut seen_sources = std::collections::BTreeSet::new();
            for input in &draft.inputs {
                if let Some(&source_idx) =
                    step_index_by_id.get(input.binding.source_step_id.as_str())
                    && source_idx < target_idx
                    && seen_sources.insert(source_idx)
                {
                    let from_x = origin_x + source_idx as f32 * step_pitch + node_w * 0.78;
                    let to_x = origin_x + target_idx as f32 * step_pitch + node_w * 0.22;
                    let lift = (18.0 + (lane % 5) as f32 * 10.0) * scale;
                    lane += 1;
                    let highlighted =
                        sheet.selected == Some(source_idx) || sheet.selected == Some(target_idx);
                    data_wires.push(DataLinkWire {
                        from_x,
                        from_y: origin_y,
                        to_x,
                        to_y: origin_y,
                        top_y: origin_y - lift,
                        color: if highlighted {
                            wire_accent
                        } else {
                            wire_accent.opacity(0.45)
                        },
                    });
                }
            }
        }

        // Edges layer inside the horizontally scrolled world
        let edges_layer = canvas(
            move |_, _, _| {},
            move |bounds, (), window, _| {
                for edge in &seq_edges {
                    let len = (edge.to_x - edge.from_x).max(0.0);
                    if len <= 1.0 {
                        continue;
                    }
                    let thickness = if edge.active { 2.2 } else { 1.6 };
                    window.paint_quad(fill(
                        Bounds {
                            origin: point(
                                bounds.origin.x + px(edge.from_x),
                                bounds.origin.y + px(edge.from_y - thickness * 0.5),
                            ),
                            size: size(px(len), px(thickness)),
                        },
                        edge.color,
                    ));
                    window.paint_quad(fill(
                        Bounds {
                            origin: point(
                                bounds.origin.x + px(edge.to_x - 6.0),
                                bounds.origin.y + px(edge.from_y - 3.5),
                            ),
                            size: size(px(6.0), px(7.0)),
                        },
                        edge.color.opacity(0.35),
                    ));
                }
                for wire in &data_wires {
                    let t = 1.6;
                    let left_h = (wire.from_y - wire.top_y).max(0.0);
                    let right_h = (wire.to_y - wire.top_y).max(0.0);
                    let span_w = (wire.to_x - wire.from_x).max(0.0);
                    window.paint_quad(fill(
                        Bounds {
                            origin: point(
                                bounds.origin.x + px(wire.from_x - t * 0.5),
                                bounds.origin.y + px(wire.top_y),
                            ),
                            size: size(px(t), px(left_h)),
                        },
                        wire.color,
                    ));
                    window.paint_quad(fill(
                        Bounds {
                            origin: point(
                                bounds.origin.x + px(wire.from_x),
                                bounds.origin.y + px(wire.top_y),
                            ),
                            size: size(px(span_w), px(t)),
                        },
                        wire.color,
                    ));
                    window.paint_quad(fill(
                        Bounds {
                            origin: point(
                                bounds.origin.x + px(wire.to_x - t * 0.5),
                                bounds.origin.y + px(wire.top_y),
                            ),
                            size: size(px(t), px(right_h)),
                        },
                        wire.color,
                    ));
                }
            },
        )
        .size_full();

        // Build world container with nodes (fixed vertical height, horizontal-only scroll)
        let mut world = div()
            .relative()
            .w(px(world_w))
            .h_full()
            .flex_shrink_0()
            .child(div().absolute().inset_0().child(edges_layer));

        let weak_sheet = self.sheet.clone();
        let step_count = sheet.steps.len();
        let running = sheet.running;

        for (index, draft) in sheet.steps.iter().enumerate() {
            let x = origin_x + index as f32 * step_pitch;
            let y = origin_y;
            let request = request_by_id.get(draft.step.request_id.as_str()).copied();
            let report = sheet.reports.get(index);
            let state = report.map_or(
                if !draft.step.enabled {
                    "Disabled"
                } else if request.is_none() {
                    "Needs request"
                } else if running && index == sheet.reports.len() {
                    "Running"
                } else {
                    "Ready"
                },
                step_state,
            );
            let status_color = match report.map(|r| &r.outcome) {
                Some(ChainStepOutcome::Passed { .. }) => success,
                Some(ChainStepOutcome::RequestFailed | ChainStepOutcome::CheckFailed(_)) => danger,
                _ if running && index == sheet.reports.len() => info,
                _ if draft.step.always_run => warning,
                _ => muted,
            };
            let title = request.map_or(
                if draft.step.request_id.is_empty() {
                    "Choose a request"
                } else {
                    "Missing request"
                },
                |choice| choice.name.as_ref(),
            );
            let url_preview = request
                .map(|choice| choice.url_preview.as_ref())
                .unwrap_or("Select a saved request");
            let selected = sheet.selected == Some(index);

            let has_outgoing = !draft.captures.is_empty() || !draft.step.captures.is_empty();
            let has_incoming = !draft.inputs.is_empty();

            let mut chips: Vec<AnyElement> = Vec::new();
            if !compact {
                for input in draft.inputs.iter().take(2) {
                    let src_idx = step_index_by_id
                        .get(input.binding.source_step_id.as_str())
                        .map(|&i| format!("#{:02}", i + 1))
                        .unwrap_or_else(|| "#?".to_owned());
                    let target_lbl =
                        target_short_label(&input.binding.target, &input.variable.read(cx).value());
                    chips.push(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(wire_accent.opacity(0.14))
                            .text_color(wire_accent)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px((10.0 * scale).clamp(8.5, 11.5)))
                            .truncate()
                            .child(format!("← {src_idx} {target_lbl}"))
                            .into_any_element(),
                    );
                }
                let total_outputs = draft.captures.len() + draft.step.captures.len();
                if total_outputs > 0 {
                    chips.push(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(success.opacity(0.14))
                            .text_color(success)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px((10.0 * scale).clamp(8.5, 11.5)))
                            .child(format!("→ {total_outputs} saved"))
                            .into_any_element(),
                    );
                }
                if !draft.assertions.is_empty() {
                    chips.push(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(cx.theme().secondary)
                            .text_color(muted)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px((10.0 * scale).clamp(8.5, 11.5)))
                            .child(format!("✓ {}", draft.assertions.len()))
                            .into_any_element(),
                    );
                }
                if draft.step.always_run {
                    chips.push(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(warning.opacity(0.16))
                            .text_color(warning)
                            .text_size(px((10.0 * scale).clamp(8.5, 11.5)))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Cleanup")
                            .into_any_element(),
                    );
                }
                if draft.step.isolate_cookies {
                    chips.push(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(cx.theme().secondary)
                            .text_color(muted)
                            .text_size(px((10.0 * scale).clamp(8.5, 11.5)))
                            .child("No cookies")
                            .into_any_element(),
                    );
                }
            }

            let status_label = match report {
                Some(r) => match r.outcome {
                    ChainStepOutcome::Passed { status } => {
                        format!("{status} · {}ms", r.elapsed.as_millis())
                    }
                    _ => format!("{state} · {}ms", r.elapsed.as_millis()),
                },
                None => state.to_owned(),
            };

            let sheet_click = weak_sheet.clone();
            let step_enabled = draft.step.enabled;
            let card_radius = px((10.0 * scale).clamp(6.0, 12.0));

            let card_surface = div()
                .id(("chain-node", index))
                .size_full()
                .rounded(card_radius)
                .border_1()
                .border_color(if selected { accent } else { border })
                .bg(card_bg)
                .when(!step_enabled, |el| el.opacity(0.55))
                .cursor_pointer()
                .hover(|s| s.border_color(accent))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(move |_, window, cx| {
                    let _ = sheet_click.update(cx, |view, cx| {
                        view.select_step(Some(index), window, cx);
                    });
                })
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(
                    div()
                        .w_full()
                        .h(px((2.5 * scale).clamp(2.0, 3.5)))
                        .flex_shrink_0()
                        .bg(if selected { accent } else { status_color }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px(px((12.0 * scale).clamp(6.0, 16.0)))
                        .py(px((9.0 * scale).clamp(4.0, 12.0)))
                        .flex()
                        .flex_col()
                        .gap(px((5.0 * scale).clamp(2.0, 8.0)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_1p5()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(cx.theme().secondary)
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_size(px((10.0 * scale).clamp(8.0, 12.0)))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(muted)
                                                .child(format!("{:02}", index + 1)),
                                        )
                                        .children(request.map(|choice| {
                                            method_badge(choice.method.as_ref(), None, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(status_color.opacity(0.14))
                                        .text_color(status_color)
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px((10.0 * scale).clamp(8.0, 11.5)))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(status_label),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px((13.5 * scale).clamp(10.0, 16.0)))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .truncate()
                                .child(title.to_owned()),
                        )
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px((11.0 * scale).clamp(8.5, 13.0)))
                                .text_color(muted)
                                .truncate()
                                .child(url_preview.to_owned()),
                        )
                        .when(!chips.is_empty(), |el| {
                            el.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .pt_0p5()
                                    .overflow_hidden()
                                    .children(chips),
                            )
                        }),
                );

            // Unclipped outer wrapper keeps circular edge ports full on the card edge.
            let node_wrapper = div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(node_w))
                .h(px(node_h))
                .child(card_surface)
                .child(
                    div()
                        .absolute()
                        .left(px(-port_r))
                        .top(px(node_h * 0.5 - port_r))
                        .w(px(port_d))
                        .h(px(port_d))
                        .rounded_full()
                        .border_1()
                        .border_color(if has_incoming { wire_accent } else { border })
                        .bg(if has_incoming { wire_accent } else { card_bg }),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(-port_r))
                        .top(px(node_h * 0.5 - port_r))
                        .w(px(port_d))
                        .h(px(port_d))
                        .rounded_full()
                        .border_1()
                        .border_color(if has_outgoing { success } else { border })
                        .bg(if has_outgoing { success } else { card_bg }),
                );

            world = world.child(node_wrapper);
        }

        // "+ Add step" card
        let add_x = origin_x + step_count as f32 * step_pitch;
        let add_y = origin_y + (node_h - add_h) * 0.5;
        let weak = weak_sheet.clone();
        let can_add = !running && step_count < fastman_core::chain_runner::MAX_CHAIN_STEPS;
        world = world.child(
            div()
                .id("add-node-card")
                .absolute()
                .left(px(add_x))
                .top(px(add_y))
                .w(px(add_w))
                .h(px(add_h))
                .rounded(px((10.0 * scale).clamp(6.0, 12.0)))
                .border_1()
                .border_color(border.opacity(0.85))
                .bg(card_bg.opacity(0.75))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    Button::new("add-chain-step")
                        .small()
                        .outline()
                        .icon(svg_icon("icons/plus.svg").xsmall())
                        .label(if step_count == 0 {
                            "Add first step"
                        } else {
                            "Add step"
                        })
                        .disabled(!can_add)
                        .dropdown_menu(move |mut menu, _, cx| {
                            let weak_blank = weak.clone();
                            menu = menu.item(PopupMenuItem::new("Empty step").on_click(
                                move |_, window, cx| {
                                    let _ = weak_blank.update(cx, |view, cx| {
                                        view.edit_step(StepEdit::Add, window, cx);
                                    });
                                },
                            ));
                            let quick_requests: Vec<(String, SharedString)> = weak
                                .upgrade()
                                .map(|entity| {
                                    entity
                                        .read(cx)
                                        .requests
                                        .iter()
                                        .take(24)
                                        .map(|r| (r.id.clone(), r.label.clone()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            if !quick_requests.is_empty() {
                                menu = menu.separator();
                            }
                            for (req_id, label) in quick_requests {
                                let weak = weak.clone();
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, window, cx| {
                                        let _ = weak.update(cx, |view, cx| {
                                            view.add_step_for_request(req_id.clone(), window, cx);
                                        });
                                    },
                                ));
                            }
                            menu
                        }),
                ),
        );

        // Summary floating pill
        let passed = sheet
            .reports
            .iter()
            .filter(|r| matches!(r.outcome, ChainStepOutcome::Passed { .. }))
            .count();
        let failed = sheet
            .reports
            .iter()
            .filter(|r| {
                matches!(
                    r.outcome,
                    ChainStepOutcome::RequestFailed | ChainStepOutcome::CheckFailed(_)
                )
            })
            .count();
        let total_ms: u128 = sheet.reports.iter().map(|r| r.elapsed.as_millis()).sum();
        let has_reports = !sheet.reports.is_empty();

        let summary_pill = div()
            .absolute()
            .top_4()
            .left_4()
            .px_3()
            .py_1p5()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(card_bg.opacity(0.94))
            .flex()
            .items_center()
            .gap_2p5()
            .text_xs()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child(format!(
                        "{step_count} {}",
                        if step_count == 1 { "step" } else { "steps" }
                    )),
            )
            .when(has_reports, |el| {
                el.child(div().text_color(border).child("|"))
                    .child(div().text_color(success).child(format!("{passed} passed")))
                    .when(failed > 0, |el| {
                        el.child(div().text_color(danger).child(format!("{failed} failed")))
                    })
                    .child(div().text_color(muted).child(format!("{total_ms} ms")))
            })
            .when(running, |el| {
                el.child(div().text_color(info).child("Running…"))
            });

        // Bottom-left zoom controls pill
        let controls_pill = div()
            .absolute()
            .bottom_4()
            .left_4()
            .px_1p5()
            .py_1()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(card_bg.opacity(0.94))
            .flex()
            .items_center()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Button::new("canvas-zoom-out")
                    .ghost()
                    .xsmall()
                    .label("-")
                    .tooltip("Zoom out")
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.set_zoom(view.zoom - 0.15);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .px_1p5()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(format!("{}%", (self.zoom * 100.0).round() as i32)),
            )
            .child(
                Button::new("canvas-zoom-in")
                    .ghost()
                    .xsmall()
                    .label("+")
                    .tooltip("Zoom in")
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.set_zoom(view.zoom + 0.15);
                        cx.notify();
                    })),
            )
            .child(div().w(px(1.)).h(px(14.)).bg(border))
            .child(
                Button::new("canvas-reset-view")
                    .ghost()
                    .xsmall()
                    .label("Reset")
                    .tooltip("Reset pan and zoom")
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.reset_view();
                        cx.notify();
                    })),
            );

        // Native GPU-accelerated horizontal-only scroll container
        let scroll_viewport = div()
            .id("chain-canvas-scroll")
            .size_full()
            .overflow_x_scroll()
            .track_scroll(&self.scroll_handle)
            .cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, _, _| {
                    view.dragging = Some((event.position, view.scroll_handle.offset()));
                }),
            )
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                if let Some((start, origin)) = view.dragging {
                    if event.pressed_button == Some(MouseButton::Left) {
                        cx.stop_propagation();
                        view.scroll_handle
                            .set_offset(point(origin.x + (event.position.x - start.x), px(0.)));
                        window.refresh();
                    } else {
                        view.dragging = None;
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _: &MouseUpEvent, _, _| {
                    view.dragging = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _: &MouseUpEvent, _, _| {
                    view.dragging = None;
                }),
            )
            .on_scroll(cx.listener(|view, event: &ScrollEvent, window, cx| {
                let delta = event.delta.pixel_delta(px(40.));
                if event.modifiers.control || event.modifiers.platform {
                    cx.stop_propagation();
                    let step = if f32::from(delta.y) > 0.0 {
                        0.10
                    } else {
                        -0.10
                    };
                    view.set_zoom(view.zoom + step);
                    cx.notify();
                } else if f32::from(delta.x).abs() < 0.5 && f32::from(delta.y).abs() > 0.0 {
                    cx.stop_propagation();
                    let offset = view.scroll_handle.offset();
                    view.scroll_handle
                        .set_offset(point(offset.x + delta.y * 1.6, px(0.)));
                    window.refresh();
                }
            }))
            .child(world);

        div()
            .id("chain-flow-canvas")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(canvas_bg)
            .child(scroll_viewport)
            .child(summary_pill)
            .child(controls_pill)
            .into_any_element()
    }
}
