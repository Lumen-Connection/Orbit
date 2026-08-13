mod sidebar;
mod thread;

use crate::app::{App, DEFAULT_MODEL, MODEL_GROUPS, Role, Screen, SettingsTab};
use crate::secure_store::SecureStore;
use crate::storage;
use crate::ui::FADE_DURATION;
use eframe::egui;
use std::time::Duration;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.poll_pending();

    let (
        temporary_mode,
        has_pending,
        active_model,
        has_fading_message,
        confirm_eject,
        active_chat_id,
        pending_chat_id,
    ) = {
        let Screen::Main(state) = &app.screen else {
            return;
        };
        let active_model = state
            .active_chat()
            .map(|c| c.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let has_fading = state
            .active_chat()
            .map(|c| {
                c.messages.iter().any(|m| {
                    matches!(m.role, Role::Assistant)
                        && m.appeared_at
                            .is_some_and(|appeared_at| appeared_at.elapsed() < FADE_DURATION)
                })
            })
            .unwrap_or(false);
        (
            state.temporary_mode,
            state.pending.is_some(),
            active_model,
            has_fading,
            state.confirm_eject,
            state.active_chat().map(|c| c.id),
            state.pending.as_ref().map(|p| p.chat_id),
        )
    };

    // Streaming inserts an empty assistant bubble immediately.
    let show_thinking = false;
    let _ = (pending_chat_id, active_chat_id);

    // === TOP BAR ===
    let mut new_model_choice: Option<String> = None;
    let mut new_temp_mode: Option<bool> = None;

    egui::Panel::top("top_bar").exact_size(44.0).show(ui, |ui| {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        ui.horizontal_centered(|ui| {
            let active_label = state.catalog.display_name(&active_model);

            egui::ComboBox::from_id_salt("model_picker")
                .selected_text(active_label)
                .width(280.0)
                .show_ui(ui, |ui| {
                    ui.set_min_width(360.0);
                    ui.label(
                        egui::RichText::new("HIGHLIGHTS")
                            .small()
                            .strong()
                            .color(crate::ui::theme::tokens(ui).text_muted),
                    );
                    ui.separator();
                    for group in MODEL_GROUPS {
                        ui.label(
                            egui::RichText::new(group.provider.to_uppercase())
                                .small()
                                .strong()
                                .color(crate::ui::theme::tokens(ui).text_muted),
                        );
                        for entry in group.models {
                            if model_row(
                                ui,
                                entry.id,
                                entry.name,
                                Some(entry.descriptor),
                                active_model == entry.id,
                            ) {
                                new_model_choice = Some(entry.id.to_string());
                            }
                        }
                    }

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("ALL MODELS")
                            .small()
                            .strong()
                            .color(crate::ui::theme::tokens(ui).text_muted),
                    );
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut state.model_search)
                            .hint_text("Search models…")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                    let matches = state.catalog.search_all(&state.model_search);
                    let shown = matches.into_iter().take(80);
                    for model in shown {
                        if model_row(
                            ui,
                            &model.id,
                            &model.name,
                            model.descriptor.as_deref(),
                            active_model == model.id,
                        ) {
                            new_model_choice = Some(model.id.clone());
                        }
                    }
                });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut temp = temporary_mode;
                if ui.toggle_value(&mut temp, "🕓 Temporary chat").changed() {
                    new_temp_mode = Some(temp);
                }
            });
        });
    });

    if let Some(model) = new_model_choice
        && let Screen::Main(state) = &mut app.screen
        && let Some(chat) = state.active_chat_mut()
    {
        chat.model = model;
    }
    if let Some(on) = new_temp_mode {
        app.set_temporary_mode(on);
    }

    // === SIDEBAR ===
    let sidebar = {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        let mut request = None;
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(220.0)
            .size_range(160.0..=360.0)
            .show(ui, |ui| {
                request = Some(sidebar::render(ui, state));
            });
        request.expect("sidebar always sets request")
    };

    if sidebar.new_chat {
        app.new_chat();
    }
    if let Some(id) = sidebar.select {
        app.select_chat(id);
    }
    if let Some(id) = sidebar.delete {
        app.delete_chat(id);
    }
    if let Some(id) = sidebar.start_rename
        && let Screen::Main(state) = &mut app.screen
        && let Some(chat) = state.chats.iter().find(|c| c.id == id)
    {
        state.renaming_chat = Some((id, chat.title.clone()));
    }
    if let Some((id, title)) = sidebar.rename {
        app.rename_chat(id, title);
    }
    if let Some(id) = sidebar.toggle_pin {
        app.toggle_pin_chat(id);
    }
    if let Some(id) = sidebar.export
        && let Screen::Main(state) = &app.screen
        && let Some(chat) = state.chats.iter().find(|c| c.id == id)
    {
        let md = crate::session::export::chat_to_markdown(chat);
        let name = format!("{}.md", sanitize_filename(&chat.title));
        app.save_markdown(&name, &md);
    }
    if sidebar.settings {
        app.open_settings(SettingsTab::Credentials);
    }
    if sidebar.eject
        && let Screen::Main(state) = &mut app.screen
    {
        state.confirm_eject = true;
    }

    // === MAIN CHAT ===
    let want_focus = if let Screen::Main(state) = &mut app.screen {
        let f = state.focus_input_next_frame;
        state.focus_input_next_frame = false;
        f
    } else {
        false
    };

    let mut action = thread::ThreadAction::None;
    egui::CentralPanel::default().show(ui, |ui| {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        action = thread::render(ui, state, has_pending, show_thinking, want_focus);
    });

    match action {
        thread::ThreadAction::Send => {
            app.send_message();
            if let Screen::Main(state) = &mut app.screen {
                state.focus_input_next_frame = true;
            }
        }
        thread::ThreadAction::Cancel => app.cancel_pending(),
        thread::ThreadAction::PersistChats => {
            if let Screen::Main(state) = &app.screen
                && !state.temporary_mode
            {
                let _ = storage::save_chats(&state.chats);
            }
        }
        thread::ThreadAction::Copy(text) => {
            ctx.copy_text(text);
        }
        thread::ThreadAction::Regenerate => app.regenerate_chat(),
        thread::ThreadAction::Delete { index } => {
            let count = match &app.screen {
                Screen::Main(state) => state
                    .active_chat()
                    .and_then(|c| crate::session::message_ops::chat_pair_range(&c.messages, index))
                    .map(|r| r.end - r.start)
                    .unwrap_or(1),
                _ => 1,
            };
            if crate::session::message_ops::needs_confirm(count) {
                if let Screen::Main(state) = &mut app.screen {
                    state.pending_confirm =
                        Some(crate::app::PendingConfirm::DeleteChat { index, count });
                }
            } else {
                app.delete_chat_pair(index);
            }
        }
        thread::ThreadAction::StartEdit { index } => {
            if let Screen::Main(state) = &mut app.screen
                && let Some(chat) = state.active_chat()
                && let Some(msg) = chat.messages.get(index)
            {
                state.editing_chat = Some(crate::app::MessageEdit {
                    index,
                    draft: msg.content.clone(),
                });
            }
        }
        thread::ThreadAction::CommitEdit => {
            if let Screen::Main(state) = &app.screen
                && let Some(edit) = state.editing_chat.clone()
            {
                let discarded = state
                    .active_chat()
                    .map(|c| c.messages.len().saturating_sub(edit.index + 1))
                    .unwrap_or(0);
                if crate::session::message_ops::needs_confirm(discarded) {
                    if let Screen::Main(state) = &mut app.screen {
                        state.pending_confirm = Some(crate::app::PendingConfirm::EditResendChat {
                            index: edit.index,
                            text: edit.draft,
                            count: discarded,
                        });
                    }
                } else {
                    app.edit_resend_chat(edit.index, edit.draft);
                }
            }
        }
        thread::ThreadAction::CancelEdit => {
            if let Screen::Main(state) = &mut app.screen {
                state.editing_chat = None;
            }
        }
        thread::ThreadAction::Export => {
            if let Some(md) = app.export_active_chat() {
                let name = match &app.screen {
                    Screen::Main(state) => state
                        .active_chat()
                        .map(|c| format!("{}.md", sanitize_filename(&c.title)))
                        .unwrap_or_else(|| "chat.md".into()),
                    _ => "chat.md".into(),
                };
                app.save_markdown(&name, &md);
            }
        }
        thread::ThreadAction::None => {}
    }

    if let Screen::Main(state) = &app.screen
        && let Some(confirm) = state.pending_confirm.clone()
    {
        match confirm {
            crate::app::PendingConfirm::DeleteChat { index, count } => {
                match crate::ui::message_actions::confirm_discard(
                    &ctx,
                    "Delete messages?",
                    &format!("{count} messages will be removed from this chat."),
                    "Delete",
                ) {
                    Some(true) => app.delete_chat_pair(index),
                    Some(false) => {
                        if let Screen::Main(state) = &mut app.screen {
                            state.pending_confirm = None;
                        }
                    }
                    None => {}
                }
            }
            crate::app::PendingConfirm::EditResendChat { index, text, count } => {
                match crate::ui::message_actions::confirm_discard(
                    &ctx,
                    "Resend and discard later messages?",
                    &format!("{count} later messages will be discarded."),
                    "Resend",
                ) {
                    Some(true) => app.edit_resend_chat(index, text),
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

    if confirm_eject {
        let mut open = true;
        let mut do_eject = false;
        let mut cancel = false;

        egui::Window::new("Sign out?")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(360.0)
            .show(&ctx, |ui| {
                ui.add_space(4.0);
                ui.label(format!(
                    "This will remove the cached API key from {} and return you to the key entry screen.",
                    SecureStore::display_name()
                ));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Your saved chats will not be deleted.")
                        .small()
                        .color(crate::ui::theme::tokens(ui).text_muted),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new("Sign out").min_size(egui::vec2(100.0, 28.0)))
                        .clicked()
                    {
                        do_eject = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 28.0)))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });
        if do_eject {
            app.eject_key();
        } else if (cancel || !open)
            && let Screen::Main(state) = &mut app.screen
        {
            state.confirm_eject = false;
        }
    }

    if has_pending || has_fading_message {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "chat".into()
    } else {
        trimmed.chars().take(40).collect()
    }
}

fn model_row(
    ui: &mut egui::Ui,
    id: &str,
    name: &str,
    descriptor: Option<&str>,
    selected: bool,
) -> bool {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        name,
        0.0,
        egui::TextFormat {
            color: ui.visuals().text_color(),
            ..Default::default()
        },
    );
    if let Some(descriptor) = descriptor {
        job.append(
            &format!("   {descriptor}"),
            0.0,
            egui::TextFormat {
                color: crate::ui::theme::tokens(ui).text_muted,
                italics: true,
                ..Default::default()
            },
        );
    } else {
        job.append(
            &format!("   {id}"),
            0.0,
            egui::TextFormat {
                color: crate::ui::theme::tokens(ui).text_muted,
                italics: true,
                ..Default::default()
            },
        );
    }
    ui.selectable_label(selected, job).clicked()
}
