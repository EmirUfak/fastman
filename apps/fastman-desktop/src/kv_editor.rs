use crate::theme::svg_icon;
use fastman_core::KeyValue;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeMap;

/// A single editable key-value row backed by retained GPUI input states.
struct KvRow {
    enabled: bool,
    key: Entity<InputState>,
    value: Entity<InputState>,
}

impl KvRow {
    fn is_blank(&self, cx: &App) -> bool {
        self.key.read(cx).value().is_empty() && self.value.read(cx).value().is_empty()
    }
}

/// Editable checkable key-value table with a trailing blank entry row.
pub struct KvTable {
    rows: Vec<KvRow>,
    key_placeholder: &'static str,
    value_placeholder: &'static str,
    show_enabled: bool,
    subscriptions: Vec<Subscription>,
}

impl KvTable {
    pub fn new(
        key_placeholder: &'static str,
        value_placeholder: &'static str,
        show_enabled: bool,
    ) -> Self {
        Self {
            rows: Vec::new(),
            key_placeholder,
            value_placeholder,
            show_enabled,
            subscriptions: Vec::new(),
        }
    }

    fn make_row<V: 'static>(
        &self,
        enabled: bool,
        key_val: &str,
        value_val: &str,
        window: &mut Window,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) -> (KvRow, [Subscription; 2]) {
        let key_ph = self.key_placeholder;
        let val_ph = self.value_placeholder;
        let k_init = key_val.to_owned();
        let v_init = value_val.to_owned();
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(key_ph)
                .default_value(k_init)
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(val_ph)
                .default_value(v_init)
        });
        let cb1 = on_change.clone();
        let sub_k = cx.subscribe_in(&key, window, move |view, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                cb1(view, window, cx);
            }
        });
        let sub_v = cx.subscribe_in(&value, window, move |view, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                on_change(view, window, cx);
            }
        });
        (
            KvRow {
                enabled,
                key,
                value,
            },
            [sub_k, sub_v],
        )
    }

    pub fn load<V: 'static>(
        &mut self,
        pairs: &[KeyValue],
        window: &mut Window,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) {
        self.rows.clear();
        self.subscriptions.clear();
        for pair in pairs {
            let (row, subs) = self.make_row(
                pair.enabled,
                &pair.key,
                &pair.value,
                window,
                cx,
                on_change.clone(),
            );
            self.rows.push(row);
            self.subscriptions.extend(subs);
        }
        self.add_row(window, cx, on_change);
    }

    pub fn load_map<V: 'static>(
        &mut self,
        map: &BTreeMap<String, String>,
        window: &mut Window,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) {
        let pairs: Vec<KeyValue> = map.iter().map(|(k, v)| KeyValue::new(k, v)).collect();
        self.load(&pairs, window, cx, on_change);
    }

    pub fn add_row<V: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) {
        let (blank, subs) = self.make_row(true, "", "", window, cx, on_change);
        self.rows.push(blank);
        self.subscriptions.extend(subs);
    }

    pub fn append_pair<V: 'static>(
        &mut self,
        key: &str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) {
        let insert_idx = self.rows.len().saturating_sub(1);
        let trailing_blank = self.rows.last().is_some_and(|row| row.is_blank(cx));
        let (row, subs) = self.make_row(true, key, value, window, cx, on_change);
        self.subscriptions.extend(subs);
        if trailing_blank {
            self.rows.insert(insert_idx, row);
        } else {
            self.rows.push(row);
        }
    }

    pub fn ensure_trailing_blank<V: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) {
        if !self.rows.last().is_some_and(|row| row.is_blank(cx)) {
            self.add_row(window, cx, on_change);
        }
    }

    pub fn remove_row<V: 'static>(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) {
        if idx < self.rows.len() {
            self.rows.remove(idx);
        }
        if self.rows.is_empty() {
            self.add_row(window, cx, on_change);
        }
    }

    pub fn toggle_row(&mut self, idx: usize) {
        if let Some(row) = self.rows.get_mut(idx) {
            row.enabled = !row.enabled;
        }
    }

    pub fn to_key_values(&self, cx: &App) -> Vec<KeyValue> {
        self.rows
            .iter()
            .filter_map(|row| {
                let key = row.key.read(cx).value().to_string();
                let value = row.value.read(cx).value().to_string();
                if key.trim().is_empty() && value.trim().is_empty() {
                    None
                } else {
                    Some(KeyValue {
                        enabled: row.enabled,
                        key: key.trim().to_owned(),
                        value,
                    })
                }
            })
            .collect()
    }

    pub fn to_map(&self, cx: &App) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for row in &self.rows {
            if !row.enabled {
                continue;
            }
            let key = row.key.read(cx).value().trim().to_owned();
            if key.is_empty() {
                continue;
            }
            let value = row.value.read(cx).value().to_string();
            out.insert(key, value);
        }
        out
    }

    pub fn active_count(&self, cx: &App) -> usize {
        self.rows
            .iter()
            .filter(|row| row.enabled && !row.key.read(cx).value().trim().is_empty())
            .count()
    }
}

/// Render a checkable key-value table inside an existing GPUI view context.
pub fn render_kv_table<V: 'static>(
    table_id: &'static str,
    table: &KvTable,
    cx: &Context<V>,
    on_toggle: impl Fn(&mut V, usize, &mut Context<V>) + Clone + 'static,
    on_remove: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + Clone + 'static,
    on_add: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    let border = cx.theme().border;
    let header_bg = cx.theme().table_head;
    let header_fg = cx.theme().table_head_foreground;
    let row_border = cx.theme().table_row_border;
    let table_bg = cx.theme().table;
    let mono = cx.theme().mono_font_family.clone();
    let row_count = table.rows.len();

    let mut rows_ui = Vec::with_capacity(row_count);
    for (idx, row) in table.rows.iter().enumerate() {
        let is_empty_trailing = idx + 1 == row_count && row.is_blank(cx);
        let view_toggle = cx.weak_entity();
        let on_toggle_cb = on_toggle.clone();
        let on_remove_cb = on_remove.clone();
        let dimmed = !row.enabled;
        let can_delete = !(row_count == 1 && is_empty_trailing);

        let mut line = div()
            .w_full()
            .min_h(px(32.))
            .flex()
            .items_center()
            .bg(table_bg)
            .border_t_1()
            .border_color(row_border);

        if table.show_enabled {
            line = line.child(
                div()
                    .w(px(34.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .when(!is_empty_trailing, |cell| {
                        cell.child(
                            Checkbox::new(SharedString::from(format!("{table_id}-chk-{idx}")))
                                .checked(row.enabled)
                                .on_change(move |_, _, cx| {
                                    let _ = view_toggle.update(cx, |view, cx| {
                                        on_toggle_cb(view, idx, cx);
                                    });
                                }),
                        )
                    })
                    .when(is_empty_trailing, |cell| {
                        cell.child(
                            div()
                                .w(px(14.))
                                .h(px(14.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(cx.theme().border.opacity(0.7)),
                        )
                    }),
            );
        }

        line = line
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .border_l_1()
                    .border_color(row_border)
                    .px_1()
                    .when(dimmed, |el| el.opacity(0.5))
                    .child(
                        Input::new(&row.key)
                            .small()
                            .appearance(false)
                            .font_family(mono.clone())
                            .text_size(px(12.)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .border_l_1()
                    .border_color(row_border)
                    .px_1()
                    .when(dimmed, |el| el.opacity(0.5))
                    .child(
                        Input::new(&row.value)
                            .small()
                            .appearance(false)
                            .font_family(mono.clone())
                            .text_size(px(12.)),
                    ),
            )
            .child(
                div()
                    .w(px(34.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(row_border)
                    .child(
                        Button::new(SharedString::from(format!("{table_id}-del-{idx}")))
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/trash.svg").xsmall())
                            .tooltip("Remove row")
                            .disabled(!can_delete)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                on_remove_cb(view, idx, window, cx);
                            })),
                    ),
            );

        rows_ui.push(line.into_any_element());
    }

    let mut header = div()
        .w_full()
        .h(px(28.))
        .flex()
        .items_center()
        .bg(header_bg)
        .text_color(header_fg)
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD);

    if table.show_enabled {
        header = header.child(div().w(px(34.)).flex_shrink_0());
    }

    header = header
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px_2p5()
                .border_l_1()
                .border_color(row_border)
                .child("KEY"),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px_2p5()
                .border_l_1()
                .border_color(row_border)
                .child("VALUE"),
        )
        .child(
            div()
                .w(px(34.))
                .flex_shrink_0()
                .border_l_1()
                .border_color(row_border),
        );

    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .child(header)
        .children(rows_ui)
        .child(
            div()
                .w_full()
                .h(px(30.))
                .flex()
                .items_center()
                .px_2()
                .bg(header_bg)
                .border_t_1()
                .border_color(row_border)
                .child(
                    Button::new(format!("{table_id}-add-row"))
                        .ghost()
                        .xsmall()
                        .icon(svg_icon("icons/plus.svg").xsmall())
                        .label("Add row")
                        .on_click(cx.listener(move |view, _, window, cx| {
                            on_add(view, window, cx);
                        })),
                ),
        )
        .into_any_element()
}
