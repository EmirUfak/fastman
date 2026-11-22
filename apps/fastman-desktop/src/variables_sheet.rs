use crate::theme::svg_icon;
use crate::workspace::WorkspaceView;
use fastman_core::model::{Environment, Project, SecretRef};
use fastman_core::secrets::{
    MAX_SECRET_VALUE_BYTES, SecretChangePlan, make_keychain_key, validate_env_var_name,
};
use fastman_core::security::is_sensitive_var_name;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VarKind {
    Plain,
    Secret,
    Env,
}

impl VarKind {
    fn label(self) -> &'static str {
        match self {
            Self::Plain => "Plain",
            Self::Secret => "Secret",
            Self::Env => "Env",
        }
    }

    fn value_placeholder(self, missing_keychain: bool) -> &'static str {
        match self {
            Self::Plain => "value",
            Self::Secret if missing_keychain => "Missing from keychain — enter value",
            Self::Secret => "secret value (OS keychain)",
            Self::Env => "ENV_VAR_NAME",
        }
    }
}

struct VarRowSpec<'a> {
    kind: VarKind,
    key_init: &'a str,
    val_init: &'a str,
    existing_keychain_key: Option<String>,
    value_edited: bool,
    is_missing_from_keychain: bool,
}

impl<'a> VarRowSpec<'a> {
    fn blank() -> Self {
        Self {
            kind: VarKind::Plain,
            key_init: "",
            val_init: "",
            existing_keychain_key: None,
            value_edited: true,
            is_missing_from_keychain: false,
        }
    }
}

struct VarRow {
    kind: VarKind,
    key: Entity<InputState>,
    value: Entity<InputState>,
    existing_keychain_key: Option<String>,
    value_edited: bool,
    is_missing_from_keychain: bool,
    _subs: [Subscription; 2],
}

impl VarRow {
    fn is_blank(&self, cx: &App) -> bool {
        self.key.read(cx).value().is_empty()
            && self.value.read(cx).value().is_empty()
            && self.existing_keychain_key.is_none()
    }
}

struct ScopeTabState {
    env_id: Option<String>,
    name: String,
    rows: Vec<VarRow>,
}

pub(super) struct VariablesSubmission {
    pub variables: BTreeMap<String, String>,
    pub secret_variables: BTreeMap<String, SecretRef>,
    pub environments: Vec<Environment>,
    pub active_environment: Option<String>,
    pub secret_plan: SecretChangePlan,
}

pub(super) struct VariablesSheetView {
    project_id: String,
    active_environment: Option<String>,
    scopes: Vec<ScopeTabState>,
    selected_scope: usize,
    new_env_name: Entity<InputState>,
    renaming_env: bool,
    rename_env_input: Entity<InputState>,
    removed_keychain_keys: Vec<String>,
    pub(super) error: Option<String>,
    pub(super) saving: bool,
}

impl VariablesSheetView {
    pub(super) fn new(
        project: &Project,
        unresolved_secrets: &BTreeMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let new_env_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("New environment (e.g. staging)"));
        let rename_env_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Environment name"));

        let mut view = Self {
            project_id: project.id.clone(),
            active_environment: project.active_environment.clone(),
            scopes: Vec::new(),
            selected_scope: 0,
            new_env_name,
            renaming_env: false,
            rename_env_input,
            removed_keychain_keys: Vec::new(),
            error: None,
            saving: false,
        };

        let base_rows = Self::load_scope_rows(
            &project.variables,
            &project.secret_variables,
            unresolved_secrets,
            window,
            cx,
        );
        view.scopes.push(ScopeTabState {
            env_id: None,
            name: "Base".into(),
            rows: base_rows,
        });

        for (idx, env) in project.environments.iter().enumerate() {
            if project.active_environment.as_deref() == Some(env.id.as_str()) {
                view.selected_scope = idx + 1;
            }
            let rows = Self::load_scope_rows(
                &env.variables,
                &env.secret_variables,
                unresolved_secrets,
                window,
                cx,
            );
            view.scopes.push(ScopeTabState {
                env_id: Some(env.id.clone()),
                name: env.name.clone(),
                rows,
            });
        }

        view
    }

    fn make_row(spec: VarRowSpec<'_>, window: &mut Window, cx: &mut Context<Self>) -> VarRow {
        let VarRowSpec {
            kind,
            key_init,
            val_init,
            existing_keychain_key,
            value_edited,
            is_missing_from_keychain,
        } = spec;
        let placeholder = if existing_keychain_key.is_some() && !value_edited {
            if is_missing_from_keychain {
                "Missing from keychain — enter value"
            } else {
                "Stored in OS keychain (type to replace)"
            }
        } else {
            kind.value_placeholder(is_missing_from_keychain)
        };
        let key_str = key_init.to_owned();
        let val_str = val_init.to_owned();
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("variable_name")
                .default_value(key_str)
        });
        let value = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(val_str);
            if kind == VarKind::Secret {
                state = state.masked(true);
            }
            state
        });
        let sub_key = cx.subscribe_in(&key, window, |view, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                view.error = None;
                view.ensure_trailing_blank(view.selected_scope, window, cx);
                cx.notify();
            }
        });
        let sub_val = cx.subscribe_in(&value, window, |view, entity, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                view.error = None;
                let scope_idx = view.selected_scope;
                if let Some(scope) = view.scopes.get_mut(scope_idx)
                    && let Some(row) = scope.rows.iter_mut().find(|r| r.value == *entity)
                    && !entity.read(cx).value().is_empty()
                {
                    row.value_edited = true;
                    row.is_missing_from_keychain = false;
                }
                view.ensure_trailing_blank(scope_idx, window, cx);
                cx.notify();
            }
        });
        VarRow {
            kind,
            key,
            value,
            existing_keychain_key,
            value_edited,
            is_missing_from_keychain,
            _subs: [sub_key, sub_val],
        }
    }

    fn load_scope_rows(
        plain: &BTreeMap<String, String>,
        secrets: &BTreeMap<String, SecretRef>,
        unresolved_secrets: &BTreeMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<VarRow> {
        let mut rows = Vec::new();
        for (name, val) in plain {
            rows.push(Self::make_row(
                VarRowSpec {
                    kind: VarKind::Plain,
                    key_init: name,
                    val_init: val,
                    existing_keychain_key: None,
                    value_edited: true,
                    is_missing_from_keychain: false,
                },
                window,
                cx,
            ));
        }
        for (name, secret_ref) in secrets {
            match secret_ref {
                SecretRef::Keychain { key } => {
                    let missing = unresolved_secrets.contains_key(name);
                    rows.push(Self::make_row(
                        VarRowSpec {
                            kind: VarKind::Secret,
                            key_init: name,
                            val_init: "",
                            existing_keychain_key: Some(key.clone()),
                            value_edited: false,
                            is_missing_from_keychain: missing,
                        },
                        window,
                        cx,
                    ));
                }
                SecretRef::Env { var } => {
                    rows.push(Self::make_row(
                        VarRowSpec {
                            kind: VarKind::Env,
                            key_init: name,
                            val_init: var,
                            existing_keychain_key: None,
                            value_edited: true,
                            is_missing_from_keychain: false,
                        },
                        window,
                        cx,
                    ));
                }
            }
        }
        rows.push(Self::make_row(VarRowSpec::blank(), window, cx));
        rows
    }

    fn ensure_trailing_blank(
        &mut self,
        scope_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let needs_blank = self
            .scopes
            .get(scope_idx)
            .is_some_and(|scope| !scope.rows.last().is_some_and(|r| r.is_blank(cx)));
        if needs_blank {
            let blank = Self::make_row(VarRowSpec::blank(), window, cx);
            if let Some(scope) = self.scopes.get_mut(scope_idx) {
                scope.rows.push(blank);
            }
        }
    }

    fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let blank = Self::make_row(VarRowSpec::blank(), window, cx);
        if let Some(scope) = self.scopes.get_mut(self.selected_scope) {
            scope.rows.push(blank);
        }
        cx.notify();
    }

    fn remove_row(&mut self, row_idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let scope_idx = self.selected_scope;
        if let Some(scope) = self.scopes.get_mut(scope_idx)
            && row_idx < scope.rows.len()
        {
            let mut removed = scope.rows.remove(row_idx);
            if let Some(old_key) = removed.existing_keychain_key.take() {
                self.removed_keychain_keys.push(old_key);
            }
            if scope.rows.is_empty() {
                drop(removed);
                self.ensure_trailing_blank(scope_idx, window, cx);
            }
        }
        cx.notify();
    }

    fn set_row_kind(
        &mut self,
        row_idx: usize,
        next_kind: VarKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope_idx = self.selected_scope;
        let Some(scope) = self.scopes.get_mut(scope_idx) else {
            return;
        };
        let Some(existing) = scope.rows.get_mut(row_idx) else {
            return;
        };
        if existing.kind == next_kind {
            return;
        }
        let key_text = existing.key.read(cx).value().to_string();
        let old_val = existing.value.read(cx).value().to_string();
        let prev_kind = existing.kind;
        let prev_keychain = existing.existing_keychain_key.take();
        if let Some(old_key) = prev_keychain
            && next_kind != VarKind::Secret
        {
            self.removed_keychain_keys.push(old_key);
        }
        let (next_val, next_edited) = match (prev_kind, next_kind) {
            // Preserve typed text when upgrading Plain -> Secret; never copy secret back to Plain/Env.
            (VarKind::Plain, VarKind::Secret) => (old_val, true),
            (_, VarKind::Secret) => (String::new(), false),
            (VarKind::Plain, VarKind::Env) => (String::new(), true),
            (VarKind::Secret | VarKind::Env, VarKind::Plain | VarKind::Env) => {
                (String::new(), true)
            }
            (VarKind::Plain, VarKind::Plain) => (old_val, true),
        };
        let replacement = Self::make_row(
            VarRowSpec {
                kind: next_kind,
                key_init: &key_text,
                val_init: &next_val,
                existing_keychain_key: None,
                value_edited: next_edited,
                is_missing_from_keychain: false,
            },
            window,
            cx,
        );
        scope.rows[row_idx] = replacement;
        self.error = None;
        cx.notify();
    }

    fn add_environment(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            self.error = Some("Environment name cannot be empty.".into());
            cx.notify();
            return;
        }
        let env_id = Environment::new(trimmed).id;
        let rows = Self::load_scope_rows(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            window,
            cx,
        );
        self.scopes.push(ScopeTabState {
            env_id: Some(env_id),
            name: trimmed.to_owned(),
            rows,
        });
        self.selected_scope = self.scopes.len() - 1;
        self.renaming_env = false;
        self.error = None;
        self.new_env_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn add_standard_presets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scopes.len() > 1 {
            return;
        }
        for name in ["local", "staging", "prod"] {
            self.add_environment(name, window, cx);
        }
        if let Some(first_env) = self.scopes.get(1).and_then(|s| s.env_id.clone()) {
            self.active_environment = Some(first_env);
            self.selected_scope = 1;
        }
        cx.notify();
    }

    fn delete_selected_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_scope == 0 || self.selected_scope >= self.scopes.len() {
            return;
        }
        let mut removed = self.scopes.remove(self.selected_scope);
        for row in &mut removed.rows {
            if let Some(key) = row.existing_keychain_key.take() {
                self.removed_keychain_keys.push(key);
            }
        }
        if let Some(removed_id) = removed.env_id.as_deref()
            && self.active_environment.as_deref() == Some(removed_id)
        {
            self.active_environment = None;
        }
        self.selected_scope = self.selected_scope.saturating_sub(1);
        self.renaming_env = false;
        let _ = window;
        cx.notify();
    }

    fn commit_env_rename(&mut self, cx: &mut Context<Self>) {
        let next = self.rename_env_input.read(cx).value().trim().to_owned();
        if next.is_empty() {
            self.error = Some("Environment name cannot be empty.".into());
            cx.notify();
            return;
        }
        if let Some(scope) = self.scopes.get_mut(self.selected_scope) {
            scope.name = next;
        }
        self.renaming_env = false;
        self.error = None;
        cx.notify();
    }

    pub(super) fn build_submission(&self, cx: &App) -> Result<VariablesSubmission, String> {
        let mut plan = SecretChangePlan::default();
        let mut retained_keys = HashSet::new();

        type ScopeVars = (BTreeMap<String, String>, BTreeMap<String, SecretRef>);
        let extract_scope = |scope: &ScopeTabState,
                             scope_tag: &str,
                             plan: &mut SecretChangePlan,
                             retained: &mut HashSet<String>|
         -> Result<ScopeVars, String> {
            let mut plain = BTreeMap::new();
            let mut secrets = BTreeMap::new();
            let mut seen_keys = HashSet::new();

            for row in &scope.rows {
                let raw_key = row.key.read(cx).value().to_string();
                let key = raw_key.trim().to_owned();
                let value = row.value.read(cx).value().to_string();

                if key.is_empty() && value.is_empty() && row.existing_keychain_key.is_none() {
                    continue;
                }
                if key.is_empty() {
                    return Err(format!(
                        "A row in '{}' has a value without a variable name.",
                        scope.name
                    ));
                }
                if !seen_keys.insert(key.clone()) {
                    return Err(format!("Duplicate variable '{}' in '{}'.", key, scope.name));
                }

                match row.kind {
                    VarKind::Plain => {
                        if let Some(old_key) = &row.existing_keychain_key {
                            plan.deletes_after_commit.push(old_key.clone());
                        }
                        plain.insert(key, value);
                    }
                    VarKind::Env => {
                        if let Some(old_key) = &row.existing_keychain_key {
                            plan.deletes_after_commit.push(old_key.clone());
                        }
                        let var_name = value.trim().to_owned();
                        if !validate_env_var_name(&var_name) {
                            return Err(format!(
                                "Env reference for '{}' in '{}' must be a valid env var name (letters/digits/_ starting with letter or _).",
                                key, scope.name
                            ));
                        }
                        secrets.insert(key, SecretRef::Env { var: var_name });
                    }
                    VarKind::Secret => {
                        if !row.value_edited
                            && let Some(existing_key) = &row.existing_keychain_key
                        {
                            retained.insert(existing_key.clone());
                            secrets.insert(
                                key,
                                SecretRef::Keychain {
                                    key: existing_key.clone(),
                                },
                            );
                        } else {
                            if value.is_empty() {
                                return Err(format!(
                                    "Secret variable '{}' in '{}' requires a non-empty value.",
                                    key, scope.name
                                ));
                            }
                            if value.len() > MAX_SECRET_VALUE_BYTES {
                                return Err(format!(
                                    "Secret variable '{}' in '{}' exceeds {} bytes.",
                                    key, scope.name, MAX_SECRET_VALUE_BYTES
                                ));
                            }
                            let new_key = make_keychain_key(&self.project_id, scope_tag);
                            plan.writes.push((new_key.clone(), value));
                            if let Some(old_key) = &row.existing_keychain_key {
                                plan.deletes_after_commit.push(old_key.clone());
                            }
                            retained.insert(new_key.clone());
                            secrets.insert(key, SecretRef::Keychain { key: new_key });
                        }
                    }
                }
            }

            Ok((plain, secrets))
        };

        let base = self.scopes.first().ok_or("Missing base variable scope")?;
        let (variables, secret_variables) =
            extract_scope(base, "base", &mut plan, &mut retained_keys)?;

        let mut environments = Vec::with_capacity(self.scopes.len().saturating_sub(1));
        for scope in self.scopes.iter().skip(1) {
            let Some(env_id) = scope.env_id.clone() else {
                continue;
            };
            let trimmed_name = scope.name.trim();
            if trimmed_name.is_empty() {
                return Err("Environment name cannot be empty.".into());
            }
            let (env_vars, env_secrets) =
                extract_scope(scope, &env_id, &mut plan, &mut retained_keys)?;
            environments.push(Environment {
                id: env_id,
                name: trimmed_name.to_owned(),
                variables: env_vars,
                secret_variables: env_secrets,
            });
        }

        let active_environment = self
            .active_environment
            .as_ref()
            .filter(|id| environments.iter().any(|e| &e.id == *id))
            .cloned();

        let mut seen_deletes = HashSet::new();
        for key in &self.removed_keychain_keys {
            if !retained_keys.contains(key) && seen_deletes.insert(key.clone()) {
                plan.deletes_after_commit.push(key.clone());
            }
        }
        plan.deletes_after_commit
            .retain(|k| !retained_keys.contains(k));

        Ok(VariablesSubmission {
            variables,
            secret_variables,
            environments,
            active_environment,
            secret_plan: plan,
        })
    }

    fn render_scope_table(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(scope) = self.scopes.get(self.selected_scope) else {
            return div().into_any_element();
        };

        let border = cx.theme().border;
        let header_bg = cx.theme().table_head;
        let header_fg = cx.theme().table_head_foreground;
        let row_border = cx.theme().table_row_border;
        let table_bg = cx.theme().table;
        let warn_color = cx.theme().warning;
        let mono = cx.theme().mono_font_family.clone();
        let row_count = scope.rows.len();

        let mut rows_ui = Vec::with_capacity(row_count);
        for (idx, row) in scope.rows.iter().enumerate() {
            let is_empty_trailing = idx + 1 == row_count && row.is_blank(cx);
            let can_delete = !(row_count == 1 && is_empty_trailing);
            let key_text = row.key.read(cx).value().to_string();
            let val_text = row.value.read(cx).value().to_string();
            let plain_secret_warning = row.kind == VarKind::Plain
                && is_sensitive_var_name(&key_text)
                && !val_text.trim().is_empty();
            let row_kind = row.kind;
            let weak_kind = cx.weak_entity();

            let line = div()
                .w_full()
                .min_h(px(34.))
                .flex()
                .items_center()
                .bg(table_bg)
                .border_t_1()
                .border_color(row_border)
                .child(
                    div().flex_1().min_w_0().px_1().child(
                        Input::new(&row.key)
                            .small()
                            .appearance(false)
                            .font_family(mono.clone())
                            .text_size(px(12.)),
                    ),
                )
                .child(
                    div()
                        .w(px(92.))
                        .flex_shrink_0()
                        .border_l_1()
                        .border_color(row_border)
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Button::new(SharedString::from(format!(
                                "var-kind-{}-{idx}",
                                self.selected_scope
                            )))
                            .xsmall()
                            .ghost()
                            .dropdown_caret(true)
                            .icon(
                                svg_icon(match row_kind {
                                    VarKind::Plain => "icons/variable.svg",
                                    VarKind::Secret => "icons/lock.svg",
                                    VarKind::Env => "icons/terminal.svg",
                                })
                                .xsmall(),
                            )
                            .label(row_kind.label())
                            .dropdown_menu(move |mut menu, _, _| {
                                for candidate in [VarKind::Plain, VarKind::Secret, VarKind::Env] {
                                    let view = weak_kind.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(candidate.label())
                                            .checked(candidate == row_kind)
                                            .on_click(move |_, window, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.set_row_kind(idx, candidate, window, cx);
                                                });
                                            }),
                                    );
                                }
                                menu
                            }),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .border_l_1()
                        .border_color(row_border)
                        .px_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div().flex_1().min_w_0().child(match row_kind {
                                VarKind::Secret => Input::new(&row.value)
                                    .small()
                                    .appearance(false)
                                    .mask_toggle()
                                    .font_family(mono.clone())
                                    .text_size(px(12.)),
                                VarKind::Plain | VarKind::Env => Input::new(&row.value)
                                    .small()
                                    .appearance(false)
                                    .font_family(mono.clone())
                                    .text_size(px(12.)),
                            }),
                        )
                        .when(plain_secret_warning, |cell| {
                            cell.child(
                                div()
                                    .flex_shrink_0()
                                    .pr_1()
                                    .text_color(warn_color)
                                    .text_size(px(10.5))
                                    .flex()
                                    .items_center()
                                    .gap_0p5()
                                    .child(svg_icon("icons/triangle-alert.svg").xsmall())
                                    .child("Plain"),
                            )
                        }),
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
                            Button::new(SharedString::from(format!(
                                "var-del-{}-{idx}",
                                self.selected_scope
                            )))
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/trash.svg").xsmall())
                            .tooltip("Remove variable")
                            .disabled(!can_delete)
                            .on_click(cx.listener(
                                move |view, _, window, cx| {
                                    view.remove_row(idx, window, cx);
                                },
                            )),
                        ),
                );
            rows_ui.push(line.into_any_element());
        }

        let header = div()
            .w_full()
            .h(px(28.))
            .flex()
            .items_center()
            .bg(header_bg)
            .text_color(header_fg)
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(div().flex_1().min_w_0().px_2p5().child("VARIABLE"))
            .child(
                div()
                    .w(px(92.))
                    .flex_shrink_0()
                    .px_2()
                    .border_l_1()
                    .border_color(row_border)
                    .child("TYPE"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2p5()
                    .border_l_1()
                    .border_color(row_border)
                    .child("VALUE / ENV VAR"),
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
                        Button::new("vars-add-row")
                            .ghost()
                            .xsmall()
                            .icon(svg_icon("icons/plus.svg").xsmall())
                            .label("Add variable")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.add_row(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for VariablesSheetView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_fg = cx.theme().muted_foreground;
        let selected_env = self
            .scopes
            .get(self.selected_scope)
            .and_then(|s| s.env_id.clone());
        let is_selected_active = selected_env.is_some() && selected_env == self.active_environment;
        let selected_name = self
            .scopes
            .get(self.selected_scope)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Base".into());

        let scope_buttons: Vec<AnyElement> = self
            .scopes
            .iter()
            .enumerate()
            .map(|(idx, scope)| {
                let is_selected = idx == self.selected_scope;
                let is_active_env = scope.env_id.is_some()
                    && scope.env_id.as_deref() == self.active_environment.as_deref();
                let mut btn = Button::new(SharedString::from(format!("var-scope-{idx}")))
                    .xsmall()
                    .label(scope.name.clone())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.selected_scope = idx;
                        view.renaming_env = false;
                        view.error = None;
                        cx.notify();
                    }));
                if is_selected {
                    btn = btn.primary();
                } else {
                    btn = btn.outline();
                }
                if is_active_env {
                    btn = btn.icon(svg_icon("icons/check.svg").xsmall());
                }
                btn.into_any_element()
            })
            .collect();

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted_fg)
                    .child("Use {{var}} in URLs, params, headers, auth, or body. Active environment overrides Base. Secret variables use the OS credential store or process env and are never written in plaintext to project.json."),
            )
            .when_some(self.error.as_ref(), |el, err| {
                el.child(Alert::error("vars-sheet-error", err.clone()).title("Validation Error"))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .id("var-scopes-strip")
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap_1p5()
                            .children(scope_buttons),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.new_env_name).xsmall()))
                            .child(
                                Button::new("add-env-btn")
                                    .xsmall()
                                    .outline()
                                    .icon(svg_icon("icons/plus.svg").xsmall())
                                    .label("Add Env")
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        let name = view.new_env_name.read(cx).value().to_string();
                                        view.add_environment(&name, window, cx);
                                    })),
                            )
                            .when(self.scopes.len() == 1, |row| {
                                row.child(
                                    Button::new("add-preset-envs")
                                        .xsmall()
                                        .ghost()
                                        .label("+ local / staging / prod")
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.add_standard_presets(window, cx);
                                        })),
                                )
                            }),
                    )
                    .when_some(selected_env, |panel, env_id| {
                        panel.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .p_2()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().secondary.opacity(0.3))
                                .child(if self.renaming_env {
                                    div()
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .child(Input::new(&self.rename_env_input).xsmall()),
                                        )
                                        .child(
                                            Button::new("env-rename-confirm")
                                                .xsmall()
                                                .primary()
                                                .label("Apply")
                                                .on_click(cx.listener(|view, _, _, cx| {
                                                    view.commit_env_rename(cx);
                                                })),
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(format!("Environment: {selected_name}"))
                                        .into_any_element()
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            Button::new("env-toggle-active")
                                                .xsmall()
                                                .outline()
                                                .label(if is_selected_active {
                                                    "Active ✓ (Use Base Only)"
                                                } else {
                                                    "Set Active"
                                                })
                                                .on_click(cx.listener(move |view, _, _, cx| {
                                                    view.active_environment = if is_selected_active {
                                                        None
                                                    } else {
                                                        Some(env_id.clone())
                                                    };
                                                    cx.notify();
                                                })),
                                        )
                                        .when(!self.renaming_env, |row| {
                                            row.child(
                                                Button::new("env-rename")
                                                    .xsmall()
                                                    .ghost()
                                                    .icon(svg_icon("icons/pencil.svg").xsmall())
                                                    .tooltip("Rename environment")
                                                    .on_click(cx.listener(|view, _, window, cx| {
                                                        let current_name = view
                                                            .scopes
                                                            .get(view.selected_scope)
                                                            .map(|s| s.name.clone())
                                                            .unwrap_or_default();
                                                        view.rename_env_input.update(cx, |input, cx| {
                                                            input.set_value(current_name, window, cx);
                                                        });
                                                        view.renaming_env = true;
                                                        cx.notify();
                                                    })),
                                            )
                                        })
                                        .child(
                                            Button::new("env-delete")
                                                .xsmall()
                                                .ghost()
                                                .icon(svg_icon("icons/trash.svg").xsmall())
                                                .tooltip("Delete environment")
                                                .on_click(cx.listener(|view, _, window, cx| {
                                                    view.delete_selected_environment(window, cx);
                                                })),
                                        ),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("variables-sheet-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.render_scope_table(cx)),
            )
    }
}

pub(super) fn open_variables_sheet(
    workspace: &mut WorkspaceView,
    window: &mut Window,
    cx: &mut Context<WorkspaceView>,
) {
    let sheet_view = cx.new(|cx| {
        VariablesSheetView::new(
            workspace.current(),
            &workspace.resolved_variables.unresolved_secrets,
            window,
            cx,
        )
    });
    let weak_workspace = cx.weak_entity();
    let project_name = workspace.current().name.clone();
    window.open_sheet(cx, move |sheet, _, cx| {
        let sheet_save = sheet_view.clone();
        let weak_save = weak_workspace.clone();
        let is_saving = sheet_view.read(cx).saving;
        sheet
            .size(px(640.))
            .title(format!("Variables & Environments · {project_name}"))
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
                            .disabled(is_saving)
                            .icon(svg_icon("icons/save.svg").small())
                            .label(if is_saving {
                                "Saving…"
                            } else {
                                "Save Variables"
                            })
                            .on_click(move |_, window, cx| {
                                let built = sheet_save.read(cx).build_submission(cx);
                                match built {
                                    Ok(submission) => {
                                        sheet_save.update(cx, |s, cx| {
                                            s.error = None;
                                            s.saving = true;
                                            cx.notify();
                                        });
                                        let _ = weak_save.update(cx, |view, cx| {
                                            view.save_variables_and_environments(
                                                submission,
                                                sheet_save.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                    Err(err_msg) => {
                                        sheet_save.update(cx, |s, cx| {
                                            s.error = Some(err_msg);
                                            cx.notify();
                                        });
                                    }
                                }
                            }),
                    ),
            )
    });
}
