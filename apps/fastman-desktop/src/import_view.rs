use crate::workspace::WorkspaceView;
use anyhow::Result;
use fastman_core::{
    ApiRequest, Auth, curl, import,
    security::{
        SanitizeSummary, SensitiveFieldFinding, collect_sensitive_request_findings,
        sanitize_request_credentials, summarize_sensitive_findings,
    },
};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    alert::Alert,
    button::ButtonVariant,
    dialog::DialogButtonProps,
    input::{Textarea, TextareaState},
    notification::Notification,
    tab::{Tab, TabBar},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;

enum PendingSensitiveImport {
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

pub(super) struct CurlImportConfirmView {
    pub(super) report: curl::CurlParseReport,
    pub(super) replace_summary: Option<String>,
    pub(super) findings_count: usize,
    findings_summary: String,
    choice: usize,
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

struct ImportDialogView {
    mode: usize,
    input: Entity<TextareaState>,
    error: Option<String>,
    pending_sensitive: Option<PendingSensitiveImport>,
    sensitive_choice: usize,
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

impl WorkspaceView {
    pub(super) fn parse_url_bar_curl(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.url.read(cx).value().to_string();
        let saved = self.current_request().cloned();
        let prior_url = saved.as_ref().map(|r| r.url.clone()).unwrap_or_default();
        let non_url_dirty = saved.as_ref().map_or_else(
            || {
                self.method != "GET"
                    || !self.params_table.to_key_values(cx).is_empty()
                    || !self.headers_table.to_key_values(cx).is_empty()
                    || !self.body_kind.is_none()
                    || !self.auth_kind.is_none()
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
        if !self.body_kind.is_none() {
            items.push("body");
        }
        if !self.auth_kind.is_none() {
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

    pub(super) fn apply_confirmed_curl_view(
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
        let notice = Self::format_curl_notice(
            "Parsed cURL into request",
            &req,
            summary,
            &unsupported,
            true,
        );
        self.apply_curl_request_to_editor(req, Some(notice), window, cx);
    }

    fn format_curl_notice(
        prefix: &str,
        req: &ApiRequest,
        summary: Option<SanitizeSummary>,
        unsupported: &[String],
        use_url: bool,
    ) -> String {
        let target = if use_url { &req.url } else { &req.name };
        let mut msg = match summary {
            Some(s) if use_url => format!(
                "{prefix} (moved {} to session Auth, stripped {})",
                s.moved_to_session_auth, s.stripped_fields
            ),
            Some(s) => format!(
                "{prefix}: {} {target} (moved {} to session Auth, stripped {})",
                req.method, s.moved_to_session_auth, s.stripped_fields
            ),
            None if !matches!(req.auth, Auth::None) => {
                format!("{prefix}: {} {target} (Auth kept session-only)", req.method)
            }
            None => format!("{prefix}: {} {target}", req.method),
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

    pub(super) fn try_apply_curl_from_url_bar(
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
                        view.set_status(
                            "Copied full cURL (with credentials) to clipboard",
                            false,
                            cx,
                        );
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

    fn finalize_collection_import(
        &mut self,
        report: import::ImportReport,
        sanitized_summary: Option<SanitizeSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let count = report.requests_imported;
        let warnings = report.warnings.len();
        let project_name = report.project.name.clone();
        self.store.save(&report.project)?;
        self.push_and_select_project(report.project, window, cx);
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

    fn finalize_curl_import(
        &mut self,
        req: ApiRequest,
        summary: Option<SanitizeSummary>,
        unsupported: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let note = Self::format_curl_notice("Imported cURL", &req, summary, unsupported, false);
        self.insert_and_open_request(req, window, cx);
        self.set_status(note.clone(), false, cx);
        window.push_notification(Notification::success(note), cx);
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
                                    if let Err(err) = view
                                        .finalize_collection_import(report, sanitized, window, cx)
                                    {
                                        saved_ok = false;
                                        modal_ok.update(cx, |m, cx| {
                                            m.error =
                                                Some(format!("Failed to save project: {err:#}"));
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
                                let unsupported = report.unsupported_flags;
                                let _ = weak_ok.update(cx, |view, cx| {
                                    view.finalize_curl_import(
                                        req,
                                        summary,
                                        &unsupported,
                                        window,
                                        cx,
                                    );
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
                                            m.error =
                                                Some(format!("Failed to save project: {err:#}"));
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
                                let _ = weak_ok.update(cx, |view, cx| {
                                    view.finalize_curl_import(req, None, &[], window, cx);
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
}
