use crate::app::{MainState, Message, MessageEdit, Role};
use crate::session::message_ops::{self, MdPart};
use crate::ui::message_actions::{self, HoverAction};
use crate::ui::theme::tokens;
use crate::ui::{FADE_DURATION, with_alpha};
use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::cell::RefCell;

thread_local! {
    static MD_CACHE: RefCell<CommonMarkCache> = RefCell::new(CommonMarkCache::default());
}

pub enum ThreadAction {
    None,
    Send,
    Cancel,
    PersistChats,
    Copy(String),
    Regenerate,
    Delete { index: usize },
    StartEdit { index: usize },
    CommitEdit,
    CancelEdit,
    Export,
}

pub fn render(
    ui: &mut egui::Ui,
    state: &mut MainState,
    has_pending: bool,
    show_thinking: bool,
    want_focus: bool,
) -> ThreadAction {
    let mut action = ThreadAction::None;
    let captured = crate::ui::attachments::capture(ui);
    state.draft_images.extend(captured);
    crate::ui::attachments::draft_strip(ui, &mut state.draft_images, &mut state.lightbox);
    crate::ui::attachments::lightbox_window(ui.ctx(), &mut state.lightbox);

    if render_system_prompt(ui, state) {
        action = ThreadAction::PersistChats;
    }
    let palette = tokens(ui);
    if let Some(hint) = &state.retry_hint {
        ui.colored_label(palette.warning, hint);
    }
    if let Some(occ) = state.active_chat().and_then(|c| c.context_occupancy) {
        ui.label(
            egui::RichText::new(crate::session::context_window::occupancy_label(occ))
                .small()
                .color(palette.text_muted),
        );
    }
    ui.horizontal(|ui| {
        if ui
            .small_button("Export")
            .on_hover_text("Export as Markdown")
            .clicked()
        {
            action = ThreadAction::Export;
        }
    });

    let input_height = 72.0;
    let remaining = ui.available_rect_before_wrap();

    let messages_rect = egui::Rect::from_min_size(
        remaining.min,
        egui::vec2(
            remaining.width(),
            (remaining.height() - input_height).max(0.0),
        ),
    );
    let mut messages_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(messages_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let mut editing = state.editing_chat.take();
    if let Some(thread_action) = render_messages(
        &mut messages_ui,
        state.active_chat().map(|c| c.messages.as_slice()),
        show_thinking,
        has_pending,
        editing.as_mut(),
    ) {
        action = thread_action;
    }
    state.editing_chat = editing;

    let input_rect = egui::Rect::from_min_size(
        egui::pos2(remaining.min.x, remaining.max.y - input_height),
        egui::vec2(remaining.width(), input_height),
    );
    let mut input_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(input_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    input_ui.add_space(8.0);

    let send_enabled =
        !has_pending && (!state.input.trim().is_empty() || !state.draft_images.is_empty());
    let text_edit = egui::TextEdit::multiline(&mut state.input)
        .desired_rows(2)
        .desired_width(input_ui.available_width() - 90.0)
        .hint_text(if has_pending {
            "Waiting for response…"
        } else {
            "Type a message and press Enter to send"
        });

    let response = input_ui.add_enabled(!has_pending, text_edit);

    if !has_pending {
        let nobody_focused = input_ui.ctx().memory(|m| m.focused()).is_none();
        if want_focus || nobody_focused {
            response.request_focus();
        }
    }

    let enter_pressed = response.has_focus()
        && input_ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

    input_ui.add_space(4.0);
    if has_pending {
        if input_ui
            .add(egui::Button::new("■ Stop").min_size(egui::vec2(64.0, 32.0)))
            .clicked()
        {
            action = ThreadAction::Cancel;
        }
    } else {
        let send_clicked = input_ui
            .add_enabled(
                send_enabled,
                egui::Button::new("Send").min_size(egui::vec2(64.0, 32.0)),
            )
            .clicked();

        if (send_clicked || enter_pressed) && send_enabled {
            action = ThreadAction::Send;
        }
    }

    action
}

fn render_system_prompt(ui: &mut egui::Ui, state: &mut MainState) -> bool {
    let mut changed = false;
    let muted = tokens(ui).text_muted;
    egui::CollapsingHeader::new("System prompt")
        .id_salt("chat_system_prompt")
        .default_open(false)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(
                    "Sent with every request in this chat. Not part of the visible history.",
                )
                .small()
                .color(muted),
            );
            if let Some(chat) = state.active_chat_mut() {
                let mut text = chat.system.clone().unwrap_or_default();
                let response = ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("Optional instructions for the model…"),
                );
                if response.changed() {
                    chat.system = if text.trim().is_empty() {
                        None
                    } else {
                        Some(text)
                    };
                    changed = true;
                }
            } else {
                ui.label(
                    egui::RichText::new("Open a chat to set a system prompt.")
                        .italics()
                        .color(muted),
                );
            }
        });
    changed
}

fn render_messages(
    ui: &mut egui::Ui,
    messages: Option<&[Message]>,
    show_thinking: bool,
    has_pending: bool,
    mut editing: Option<&mut MessageEdit>,
) -> Option<ThreadAction> {
    let mut action = None;
    let muted = tokens(ui).text_muted;
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.add_space(8.0);

            let empty = match messages {
                None => true,
                Some(m) => m.is_empty(),
            };

            if empty && !show_thinking {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Start a conversation below.").color(muted));
                });
                return;
            }

            if let Some(msgs) = messages {
                let last = msgs.len().saturating_sub(1);
                for (idx, msg) in msgs.iter().enumerate() {
                    let editing_this = editing.as_ref().is_some_and(|e| e.index == idx);
                    let is_last_assistant =
                        matches!(msg.role, Role::Assistant) && idx == last && !has_pending;
                    let can_edit = matches!(msg.role, Role::User) && !has_pending;
                    if editing_this {
                        if let Some(edit) = editing.as_mut()
                            && let Some(edit_action) = render_edit(ui, &mut edit.draft)
                        {
                            action = Some(edit_action);
                        }
                    } else if let Some(msg_action) =
                        render_message(ui, msg, idx, is_last_assistant, can_edit, !has_pending)
                    {
                        action = Some(msg_action);
                    }
                    ui.add_space(8.0);
                }
            }

            if show_thinking {
                render_thinking(ui);
                ui.add_space(8.0);
            }
        });
    action
}

fn render_edit(ui: &mut egui::Ui, draft: &mut String) -> Option<ThreadAction> {
    let mut action = None;
    ui.add(
        egui::TextEdit::multiline(draft)
            .desired_rows(3)
            .desired_width(ui.available_width() * 0.75),
    );
    ui.horizontal(|ui| {
        if ui.button("Resend").clicked() {
            action = Some(ThreadAction::CommitEdit);
        }
        if ui.button("Cancel").clicked() {
            action = Some(ThreadAction::CancelEdit);
        }
    });
    action
}

fn render_thinking(ui: &mut egui::Ui) {
    let phase = (ui.input(|i| i.time) * 4.0) as usize % 4;
    let dots = ".".repeat(phase);
    let text = format!("Thinking{}", dots);
    let palette = tokens(ui);

    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        let max_width = ui.available_width() * 0.75;
        egui::Frame::group(ui.style())
            .fill(palette.bubble_assistant)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_max_width(max_width);
                ui.label(
                    egui::RichText::new(text)
                        .italics()
                        .color(palette.text_muted),
                );
            });
    });

    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(120));
}

fn render_message(
    ui: &mut egui::Ui,
    msg: &Message,
    index: usize,
    can_regenerate: bool,
    can_edit: bool,
    enabled: bool,
) -> Option<ThreadAction> {
    let is_user = matches!(msg.role, Role::User);
    let palette = tokens(ui);

    let alpha = if is_user {
        255u8
    } else if let Some(appeared_at) = msg.appeared_at {
        let t = appeared_at.elapsed().as_secs_f32() / FADE_DURATION.as_secs_f32();
        (t.clamp(0.0, 1.0) * 255.0) as u8
    } else {
        255u8
    };

    let bubble_color = if is_user {
        palette.bubble_user
    } else {
        palette.bubble_assistant
    };
    let bubble_color = with_alpha(bubble_color, alpha);
    let text_color = with_alpha(
        if is_user {
            palette.text_on_user
        } else {
            palette.text_primary
        },
        alpha,
    );

    let layout = if is_user {
        egui::Layout::right_to_left(egui::Align::Min)
    } else {
        egui::Layout::left_to_right(egui::Align::Min)
    };

    let mut action = None;
    ui.with_layout(layout, |ui| {
        let max_width = ui.available_width() * 0.75;
        let inner = egui::Frame::group(ui.style())
            .fill(bubble_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_max_width(max_width);

                if is_user {
                    if !msg.content.is_empty() {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&msg.content).color(text_color))
                                .wrap(),
                        );
                    }
                    for image in &msg.images {
                        ui.label(
                            egui::RichText::new(format!(
                                "🖼 image {}×{}",
                                image.width, image.height
                            ))
                            .small()
                            .color(text_color),
                        );
                    }
                } else if msg.content.is_empty() && !msg.interrupted {
                    ui.label(
                        egui::RichText::new("Thinking…")
                            .italics()
                            .color(with_alpha(palette.text_muted, alpha)),
                    );
                } else {
                    render_markdown_with_copy(ui, &msg.content, text_color, &mut action);
                    if msg.interrupted {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Interrupted")
                                .italics()
                                .small()
                                .color(with_alpha(palette.text_muted, alpha)),
                        );
                    }
                }
            });

        if let Some(hover) = message_actions::hover_bar(
            ui,
            inner.response.hovered(),
            can_regenerate,
            can_edit,
            enabled,
        ) {
            action = Some(match hover {
                HoverAction::Copy => ThreadAction::Copy(msg.content.clone()),
                HoverAction::Regenerate => ThreadAction::Regenerate,
                HoverAction::Edit => ThreadAction::StartEdit { index },
                HoverAction::Delete => ThreadAction::Delete { index },
            });
        }
    });
    action
}

fn render_markdown_with_copy(
    ui: &mut egui::Ui,
    content: &str,
    text_color: egui::Color32,
    action: &mut Option<ThreadAction>,
) {
    let parts = message_ops::split_fenced_code(content);
    if parts.is_empty() || (parts.len() == 1 && matches!(parts[0], MdPart::Text(_))) {
        let mut visuals = ui.visuals().clone();
        visuals.override_text_color = Some(text_color);
        let prev = std::mem::replace(ui.visuals_mut(), visuals);
        MD_CACHE.with(|cache| {
            CommonMarkViewer::new().show(ui, &mut cache.borrow_mut(), content);
        });
        *ui.visuals_mut() = prev;
        return;
    }
    for part in parts {
        match part {
            MdPart::Text(text) if !text.trim().is_empty() => {
                let mut visuals = ui.visuals().clone();
                visuals.override_text_color = Some(text_color);
                let prev = std::mem::replace(ui.visuals_mut(), visuals);
                MD_CACHE.with(|cache| {
                    CommonMarkViewer::new().show(ui, &mut cache.borrow_mut(), text);
                });
                *ui.visuals_mut() = prev;
            }
            MdPart::Text(_) => {}
            MdPart::Code { lang, body } => {
                let palette = tokens(ui);
                egui::Frame::group(ui.style())
                    .fill(palette.surface)
                    .stroke(egui::Stroke::new(1.0, palette.border))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if !lang.is_empty() {
                                ui.label(
                                    egui::RichText::new(lang).small().color(palette.text_muted),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if message_actions::copy_code_button(ui, body) {
                                        *action = Some(ThreadAction::Copy(body.to_string()));
                                    }
                                },
                            );
                        });
                        ui.label(
                            egui::RichText::new(body)
                                .monospace()
                                .color(palette.text_primary),
                        );
                    });
            }
        }
    }
}
