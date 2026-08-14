//! Coder session transcript: messages, tool rows, input.

use super::approvals;
use crate::app::{
    App, CredentialState, MODEL_GROUPS, Screen, SettingsTab, can_create_session, credential_state,
};
use crate::session::{SessionId, TranscriptItem};
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut send = false;
    let mut cancel = false;
    let mut toggle_tool = None;
    let mut resolve = None;
    let mut new_model = None;
    let mut select: Option<SessionId> = None;
    let mut close: Option<SessionId> = None;
    let mut create = false;
    let mut rename: Option<(SessionId, String)> = None;
    let mut start_rename: Option<SessionId> = None;
    let mut raise_budget: Option<f64> = None;
    let mut decline_budget = false;
    let mut open_settings = false;
    let mut copy_text: Option<String> = None;
    let mut regenerate = false;
    let mut delete_idx: Option<usize> = None;
    let mut start_edit: Option<usize> = None;
    let mut commit_edit = false;
    let mut cancel_edit = false;
    let mut export = false;

    {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        let cred = credential_state(&state.credential);
        let allowed = can_create_session(cred).is_ok();
        let palette = crate::ui::theme::tokens(ui);

        if state.coder.restore_rx.is_some() {
            ui.horizontal(|ui| {
                if state.settings.motion == crate::storage::MotionPreference::Full {
                    ui.spinner();
                }
                ui.label(
                    egui::RichText::new("Restoring sessions…")
                        .italics()
                        .color(palette.text_muted),
                );
            });
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("SESSIONS // AGENT CHANNELS")
                    .small()
                    .strong()
                    .monospace()
                    .color(palette.text_muted),
            );
            let export_button =
                crate::ui::theme::action_button(ui, "EXPORT", crate::ui::theme::Tone::Neutral)
                    .small();
            if ui
                .add(export_button)
                .on_hover_text("Export as Markdown")
                .clicked()
            {
                export = true;
            }
            let add_button =
                crate::ui::theme::action_button(ui, "+ NEW", crate::ui::theme::Tone::Accent)
                    .small();
            let add = ui
                .add_enabled(allowed, add_button)
                .on_hover_text("New session")
                .on_disabled_hover_text(
                    "Configure an API key in Settings before creating a session.",
                );
            if add.clicked() {
                create = true;
            }
            let configure_button = crate::ui::theme::action_button(
                ui,
                "CONFIGURE KEY",
                crate::ui::theme::Tone::Warning,
            )
            .small();
            if !allowed && ui.add(configure_button).clicked() {
                open_settings = true;
            }
        });

        if !allowed && state.coder.sessions.sessions.is_empty() {
            ui.add_space(28.0);
            ui.label(
                egui::RichText::new("An API key is required before any Coder session.")
                    .color(palette.text_muted),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(match cred {
                    CredentialState::Missing => {
                        "Open Settings to add your OpenRouter key. Saved history stays on disk."
                    }
                    CredentialState::Rejected => {
                        "The stored key was rejected. Replace it in Settings to continue."
                    }
                    CredentialState::Present => "",
                })
                .small()
                .color(palette.text_muted),
            );
            ui.add_space(10.0);
            let configure_button = crate::ui::theme::action_button(
                ui,
                "CONFIGURE KEY",
                crate::ui::theme::Tone::Warning,
            );
            if ui.add(configure_button).clicked() {
                open_settings = true;
            }
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let active_id = state.coder.sessions.active().map(|s| s.id.clone());
            let can_close = state.coder.sessions.sessions.len() > 1;
            for session in &mut state.coder.sessions.sessions {
                let selected = active_id.as_ref() == Some(&session.id);
                let mark = if session.busy { " ●" } else { "" };
                if session.editing_label && selected {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut session.label)
                            .desired_width(140.0)
                            .font(egui::TextStyle::Body),
                    );
                    if response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.pointer.any_click())
                    {
                        rename = Some((session.id.clone(), session.label.clone()));
                    }
                } else {
                    let tab = ui.selectable_label(selected, format!("{}{mark}", session.label));
                    if tab.clicked() {
                        select = Some(session.id.clone());
                    }
                    if tab.double_clicked() {
                        start_rename = Some(session.id.clone());
                    }
                }
                if can_close
                    && ui
                        .small_button("✕")
                        .on_hover_text("Close session")
                        .clicked()
                {
                    close = Some(session.id.clone());
                }
            }
        });
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if let Some(live) = state.coder.sessions.active() {
                if live.busy {
                    if state.settings.motion == crate::storage::MotionPreference::Full {
                        ui.spinner();
                    }
                    if let Some(hint) = &live.retry_hint {
                        ui.colored_label(palette.warning, hint);
                    } else {
                        ui.label(
                            egui::RichText::new("working…")
                                .italics()
                                .color(palette.text_muted),
                        );
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = state.catalog.display_name(&live.model);
                    egui::ComboBox::from_id_salt("coder_model")
                        .selected_text(label)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for group in MODEL_GROUPS {
                                ui.label(
                                    egui::RichText::new(group.provider.to_uppercase())
                                        .small()
                                        .strong()
                                        .color(palette.text_muted),
                                );
                                for entry in group.models {
                                    if ui
                                        .selectable_label(live.model == entry.id, entry.name)
                                        .clicked()
                                    {
                                        new_model = Some(entry.id.to_string());
                                    }
                                }
                            }
                            let current = live.model.clone();
                            if !MODEL_GROUPS
                                .iter()
                                .flat_map(|g| g.models)
                                .any(|m| m.id == current)
                            {
                                ui.separator();
                                let _ = ui.selectable_label(true, current);
                            }
                        });
                });
            }
        });
        ui.add_space(6.0);

        if let Some(live) = state.coder.sessions.active() {
            crate::ui::widgets::cost_meter::render(ui, live);
            ui.add_space(4.0);
        }

        if let Some(live) = state.coder.sessions.active()
            && let Some((spent, cap)) = live.budget_prompt
        {
            crate::ui::theme::panel_toned(ui, crate::ui::theme::Tone::Warning)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Budget reached: ${spent:.4} / ${cap:.2}. Raise the cap to continue."
                        ))
                        .color(palette.warning),
                    );
                    ui.horizontal(|ui| {
                        let raise_button = crate::ui::theme::action_button(
                            ui,
                            format!("RAISE TO ${:.2}", cap + 2.0),
                            crate::ui::theme::Tone::Warning,
                        );
                        if ui.add(raise_button).clicked() {
                            raise_budget = Some(cap + 2.0);
                        }
                        let stop_button = crate::ui::theme::action_button(
                            ui,
                            "STOP",
                            crate::ui::theme::Tone::Danger,
                        );
                        if ui.add(stop_button).clicked() {
                            decline_budget = true;
                        }
                    });
                });
            ui.add_space(6.0);
        }

        if let Some(live) = state.coder.sessions.active()
            && !live.handoff_dismissed
            && let Some(handoff) = &live.handoff
            && handoff.is_interesting()
        {
            crate::ui::theme::panel_toned(ui, crate::ui::theme::Tone::Active)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(&handoff.banner)
                            .strong()
                            .color(palette.warning),
                    );
                    ui.label(
                        egui::RichText::new(handoff.digest_section.trim())
                            .small()
                            .monospace()
                            .color(palette.text_muted),
                    );
                });
            ui.add_space(6.0);
        }

        // Reserve the full framed dispatch-console height. The previous 86 px
        // reservation let the panel extend into the Run // Process Bay at
        // compact window heights.
        let input_height = 112.0;
        let avail = ui.available_size();
        let thread_h = (avail.y - input_height).max(0.0);
        let mut edit_draft = state.editing_coder.clone();

        egui::ScrollArea::vertical()
            .id_salt("coder_transcript")
            .auto_shrink([false; 2])
            .stick_to_bottom(true)
            .max_height(thread_h)
            .show(ui, |ui| {
                let live = state.coder.sessions.active_mut();
                let empty = live
                    .as_ref()
                    .is_none_or(|s| s.transcript.is_empty() && !s.busy);
                if empty {
                    ui.add_space(24.0);
                    ui.label(
                        egui::RichText::new(
                            "Ask the agent to inspect or change this project. Writes wait for approval.",
                        )
                        .italics()
                        .color(palette.text_muted),
                    );
                }
                let Some(live) = live else {
                    return;
                };
                let last_user = crate::session::message_ops::last_user_transcript(&live.transcript);
                let busy = live.busy;
                for (idx, item) in live.transcript.iter().enumerate() {
                    match item {
                        TranscriptItem::User(text) => {
                            let editing_this = edit_draft.as_ref().is_some_and(|e| e.index == idx);
                            if editing_this {
                                if let Some(edit) = edit_draft.as_mut() {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut edit.draft)
                                            .desired_rows(3)
                                            .desired_width(ui.available_width()),
                                    );
                                    ui.horizontal(|ui| {
                                        if ui.button("Resend").clicked() {
                                            commit_edit = true;
                                        }
                                        if ui.button("Cancel").clicked() {
                                            cancel_edit = true;
                                        }
                                    });
                                }
                            } else if let Some(act) =
                                bubble(ui, "You", text, false, false, true, !busy)
                            {
                                apply_hover(
                                    act,
                                    text,
                                    idx,
                                    &mut copy_text,
                                    &mut regenerate,
                                    &mut delete_idx,
                                    &mut start_edit,
                                );
                            }
                        }
                        TranscriptItem::Assistant(text) => {
                            if !text.is_empty() {
                                let can_regen =
                                    last_user.is_some_and(|u| idx >= u) && !busy;
                                if let Some(act) = bubble(
                                    ui,
                                    "Agent",
                                    text,
                                    text.starts_with('⚠'),
                                    can_regen,
                                    false,
                                    !busy,
                                ) {
                                    apply_hover(
                                        act,
                                        text,
                                        idx,
                                        &mut copy_text,
                                        &mut regenerate,
                                        &mut delete_idx,
                                        &mut start_edit,
                                    );
                                }
                            }
                        }
                        TranscriptItem::Tool {
                            name,
                            summary,
                            output,
                            is_error,
                            running,
                            expanded,
                            ..
                        } => {
                            if render_tool_row(ui, name, summary, output, *is_error, *running, *expanded)
                            {
                                toggle_tool = Some(idx);
                            }
                        }
                        TranscriptItem::Approval { handle, resolved } => {
                            if let Some(decision) = approvals::render(ui, handle, *resolved) {
                                resolve = Some((handle.id, decision));
                            }
                        }
                    }
                }
            });
        if !commit_edit && !cancel_edit {
            state.editing_coder = edit_draft;
        }

        ui.add_space(6.0);
        let captured = crate::ui::attachments::capture(ui);
        state.draft_images.extend(captured);
        crate::ui::attachments::draft_strip(ui, &mut state.draft_images, &mut state.lightbox);
        crate::ui::attachments::lightbox_window(ui.ctx(), &mut state.lightbox);
        crate::ui::theme::panel(ui).show(ui, |ui| {
            ui.label(
                egui::RichText::new("DISPATCH CONSOLE")
                    .small()
                    .strong()
                    .monospace()
                    .color(palette.accent),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let Some(live) = state.coder.sessions.active_mut() else {
                    return;
                };
                let busy = live.busy;
                let send_width = 68.0;
                let text_width = (ui.available_width() - send_width - 12.0).max(40.0);
                let mut response = ui.add_enabled(
                    !busy && allowed,
                    egui::TextEdit::multiline(&mut live.input)
                        .desired_rows(2)
                        .desired_width(text_width)
                        .hint_text(if !allowed {
                            "Configure an API key to send…"
                        } else if busy {
                            "Agent is working…"
                        } else {
                            "Ask the agent… (Enter to send)"
                        }),
                );
                if !allowed {
                    response = response.on_disabled_hover_text(
                        "Sessions are read-only until an API key is configured.",
                    );
                }
                let enter = allowed
                    && response.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if busy {
                    let stop_button = crate::ui::theme::action_button(
                        ui,
                        "■ STOP",
                        crate::ui::theme::Tone::Danger,
                    )
                    .min_size(egui::vec2(74.0, 32.0));
                    if ui.add(stop_button).clicked() {
                        cancel = true;
                    }
                } else {
                    let can_send = allowed
                        && (!live.input.trim().is_empty() || !state.draft_images.is_empty());
                    let send_button = crate::ui::theme::action_button(
                        ui,
                        "SEND  ↑",
                        crate::ui::theme::Tone::Accent,
                    )
                    .min_size(egui::vec2(send_width, 30.0));
                    if (ui
                        .add_enabled(can_send, send_button)
                        .on_disabled_hover_text(if !allowed {
                            "Configure an API key in Settings to send."
                        } else {
                            "Type a message first."
                        })
                        .clicked()
                        || enter)
                        && can_send
                    {
                        send = true;
                    }
                }
            });
        });
    }

    if open_settings {
        app.open_settings(SettingsTab::Credentials);
    }
    if create {
        app.new_coder_session();
    }
    if let Some(id) = select {
        app.select_coder_session(id);
    }
    if let Some(id) = close {
        app.close_coder_session(id);
    }
    if let Some(id) = start_rename
        && let Screen::Main(state) = &mut app.screen
        && let Some(live) = state.coder.sessions.get_mut(&id)
    {
        live.editing_label = true;
    }
    if let Some((id, label)) = rename {
        app.rename_coder_session(id, label);
    }
    if let Some(cap) = raise_budget {
        app.raise_coder_budget(cap);
    }
    if decline_budget {
        app.decline_coder_budget();
    }
    if let Some(model) = new_model {
        app.set_coder_model(model);
    }
    if let Some(idx) = toggle_tool
        && let Screen::Main(state) = &mut app.screen
        && let Some(live) = state.coder.sessions.active_mut()
        && let Some(TranscriptItem::Tool { expanded, .. }) = live.transcript.get_mut(idx)
    {
        *expanded = !*expanded;
    }
    if let Some((id, decision)) = resolve {
        app.resolve_coder_approval(id, decision);
    }
    if send {
        app.send_coder_prompt();
    }
    if cancel {
        app.cancel_coder_turn();
    }
    if let Some(text) = copy_text {
        ui.ctx().copy_text(text);
    }
    if regenerate {
        app.regenerate_coder();
    }
    if let Some(index) = delete_idx {
        let count = match &app.screen {
            Screen::Main(state) => state
                .coder
                .sessions
                .active()
                .and_then(|s| crate::session::message_ops::coder_turn_range(&s.transcript, index))
                .map(|r| r.end - r.start)
                .unwrap_or(1),
            _ => 1,
        };
        if crate::session::message_ops::needs_confirm(count) {
            if let Screen::Main(state) = &mut app.screen {
                state.pending_confirm =
                    Some(crate::app::PendingConfirm::DeleteCoder { index, count });
            }
        } else {
            app.delete_coder_turn(index);
        }
    }
    if let Some(index) = start_edit
        && let Screen::Main(state) = &mut app.screen
        && let Some(live) = state.coder.sessions.active()
        && let Some(crate::session::TranscriptItem::User(text)) = live.transcript.get(index)
    {
        state.editing_coder = Some(crate::app::MessageEdit {
            index,
            draft: text.clone(),
        });
    }
    if commit_edit
        && let Screen::Main(state) = &app.screen
        && let Some(edit) = state.editing_coder.clone()
    {
        let discarded = state
            .coder
            .sessions
            .active()
            .map(|s| s.transcript.len().saturating_sub(edit.index + 1))
            .unwrap_or(0);
        if crate::session::message_ops::needs_confirm(discarded) {
            if let Screen::Main(state) = &mut app.screen {
                state.pending_confirm = Some(crate::app::PendingConfirm::EditResendCoder {
                    index: edit.index,
                    text: edit.draft,
                    count: discarded,
                });
            }
        } else {
            app.edit_resend_coder(edit.index, edit.draft);
        }
    }
    if cancel_edit && let Screen::Main(state) = &mut app.screen {
        state.editing_coder = None;
    }
    if export && let Some(md) = app.export_active_coder() {
        let name = match &app.screen {
            Screen::Main(state) => state
                .coder
                .sessions
                .active()
                .map(|s| format!("{}.md", s.label.replace(' ', "-")))
                .unwrap_or_else(|| "session.md".into()),
            _ => "session.md".into(),
        };
        app.save_markdown(&name, &md);
    }
    if let Screen::Main(state) = &app.screen
        && let Some(confirm) = state.pending_confirm.clone()
    {
        match confirm {
            crate::app::PendingConfirm::DeleteCoder { index, count } => {
                match crate::ui::message_actions::confirm_discard(
                    ui.ctx(),
                    "Delete this turn?",
                    &format!("{count} items will be removed from the session."),
                    "Delete",
                ) {
                    Some(true) => app.delete_coder_turn(index),
                    Some(false) => {
                        if let Screen::Main(state) = &mut app.screen {
                            state.pending_confirm = None;
                        }
                    }
                    None => {}
                }
            }
            crate::app::PendingConfirm::EditResendCoder { index, text, count } => {
                match crate::ui::message_actions::confirm_discard(
                    ui.ctx(),
                    "Resend and discard later turns?",
                    &format!("{count} later items will be discarded."),
                    "Resend",
                ) {
                    Some(true) => app.edit_resend_coder(index, text),
                    Some(false) => {
                        if let Screen::Main(state) = &mut app.screen {
                            state.pending_confirm = None;
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }
}

fn apply_hover(
    act: crate::ui::message_actions::HoverAction,
    text: &str,
    idx: usize,
    copy_text: &mut Option<String>,
    regenerate: &mut bool,
    delete_idx: &mut Option<usize>,
    start_edit: &mut Option<usize>,
) {
    match act {
        crate::ui::message_actions::HoverAction::Copy => *copy_text = Some(text.to_string()),
        crate::ui::message_actions::HoverAction::Regenerate => *regenerate = true,
        crate::ui::message_actions::HoverAction::Edit => *start_edit = Some(idx),
        crate::ui::message_actions::HoverAction::Delete => *delete_idx = Some(idx),
    }
}

fn bubble(
    ui: &mut egui::Ui,
    who: &str,
    text: &str,
    error: bool,
    can_regen: bool,
    can_edit: bool,
    enabled: bool,
) -> Option<crate::ui::message_actions::HoverAction> {
    let palette = crate::ui::theme::tokens(ui);
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(format!("{} // TRANSCRIPT", who.to_uppercase()))
            .small()
            .strong()
            .monospace()
            .color(if error {
                palette.danger
            } else {
                palette.text_muted
            }),
    );
    let color = if error {
        palette.danger
    } else {
        palette.text_primary
    };
    let inner = crate::ui::theme::panel_toned(
        ui,
        if error {
            crate::ui::theme::Tone::Danger
        } else if who == "You" {
            crate::ui::theme::Tone::Accent
        } else {
            crate::ui::theme::Tone::Neutral
        },
    )
    .show(ui, |ui| ui.label(egui::RichText::new(text).color(color)))
    .response;
    crate::ui::message_actions::hover_bar(ui, inner.hovered(), can_regen, can_edit, enabled)
}

fn render_tool_row(
    ui: &mut egui::Ui,
    name: &str,
    summary: &str,
    output: &str,
    is_error: bool,
    running: bool,
    expanded: bool,
) -> bool {
    ui.add_space(4.0);
    let hint = if running {
        "running…".to_string()
    } else if is_error {
        "error".into()
    } else {
        result_hint(name, output)
    };
    let icon = if is_error { "⚠" } else { "⚙" };
    let header = format!("{icon} {summary} · {hint}");
    let palette = crate::ui::theme::tokens(ui);
    let color = if is_error {
        palette.danger
    } else if running {
        palette.warning
    } else {
        palette.text_muted
    };
    let clicked = crate::ui::theme::panel_toned(
        ui,
        if is_error {
            crate::ui::theme::Tone::Danger
        } else if running {
            crate::ui::theme::Tone::Active
        } else {
            crate::ui::theme::Tone::Neutral
        },
    )
    .show(ui, |ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(header).color(color).monospace())
                .sense(egui::Sense::click()),
        )
        .clicked()
    })
    .inner;
    if running {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                egui::RichText::new("executing")
                    .small()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
        });
    }
    if expanded && !output.is_empty() {
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let palette = crate::ui::theme::tokens(ui);
                ui.label(egui::RichText::new(output).monospace().color(if is_error {
                    palette.danger
                } else {
                    palette.text_muted
                }));
            });
    }
    clicked
}

pub fn result_hint(name: &str, output: &str) -> String {
    if output == "No matches." {
        return "0 results".into();
    }
    let lines = output
        .lines()
        .filter(|line| !line.starts_with('…') && !line.starts_with('#'))
        .count();
    match name {
        "grep" => format!("{lines} results"),
        "glob" | "list_dir" => format!("{lines} paths"),
        "run_command" => "command".into(),
        "record_decision" | "add_finding" | "update_task" => "saved".into(),
        _ => "done".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::result_hint;

    #[test]
    fn grep_hint_counts_hits() {
        let out = "src/lib.rs:1:fn authenticate()\nsrc/a.rs:4:fn authenticate_user()";
        assert_eq!(result_hint("grep", out), "2 results");
        assert_eq!(result_hint("grep", "No matches."), "0 results");
    }
}
