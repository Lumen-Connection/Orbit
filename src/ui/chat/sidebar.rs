use crate::app::{Chat, MainState};
use crate::session::message_ops::{self, DateGroup};
use crate::ui::theme::tokens;
use crate::ui::truncate;
use chrono::Utc;
use eframe::egui;
use uuid::Uuid;

pub struct SidebarRequest {
    pub select: Option<Uuid>,
    pub delete: Option<Uuid>,
    pub new_chat: bool,
    pub settings: bool,
    pub eject: bool,
    pub rename: Option<(Uuid, String)>,
    pub start_rename: Option<Uuid>,
    pub toggle_pin: Option<Uuid>,
    pub export: Option<Uuid>,
}

pub fn render(ui: &mut egui::Ui, state: &mut MainState) -> SidebarRequest {
    let mut request = SidebarRequest {
        select: None,
        delete: None,
        new_chat: false,
        settings: false,
        eject: false,
        rename: None,
        start_rename: None,
        toggle_pin: None,
        export: None,
    };
    let palette = tokens(ui);

    let about_row_height = 32.0;
    let total_h = ui.available_height();
    let list_h = (total_h - about_row_height - 16.0).max(40.0);

    ui.allocate_ui(egui::vec2(ui.available_width(), list_h), |ui| {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("ARCHIVE // CHAT LOG")
                .small()
                .strong()
                .monospace()
                .color(palette.text_muted),
        );
        ui.add_space(6.0);
        let new_chat_button =
            crate::ui::theme::action_button(ui, "+  NEW DISPATCH", crate::ui::theme::Tone::Accent)
                .min_size(egui::vec2(ui.available_width(), 32.0));
        if ui.add(new_chat_button).clicked() {
            request.new_chat = true;
        }
        ui.add_space(6.0);
        let search = ui.add(
            egui::TextEdit::singleline(&mut state.chat_search)
                .hint_text("Search chats (Ctrl+K)")
                .desired_width(f32::INFINITY),
        );
        if state.focus_search_next_frame {
            search.request_focus();
            state.focus_search_next_frame = false;
        }
        ui.add_space(8.0);
        crate::ui::theme::section_header(ui, "CHAT LIST");

        if state.temporary_mode {
            ui.add_space(8.0);
            crate::ui::theme::panel_toned(ui, crate::ui::theme::Tone::Warning).show(ui, |ui| {
                ui.label(
                    egui::RichText::new("TEMPORARY MODE")
                        .small()
                        .strong()
                        .monospace()
                        .color(palette.warning),
                );
                ui.label(
                    egui::RichText::new("This dispatch will not be saved.")
                        .small()
                        .color(palette.text_muted),
                );
            });
            return;
        }

        let query = state.chat_search.clone();
        let now = Utc::now();
        let mut rows: Vec<ChatRow> = state
            .chats
            .iter()
            .filter(|chat| message_ops::chat_matches(&chat.title, &chat.messages, &query))
            .map(|chat| ChatRow::from_chat(chat, &query))
            .collect();
        rows.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let active_id = state.active_chat_id;
                let mut last_group: Option<DateGroup> = None;
                for row in rows {
                    let group = if row.pinned {
                        DateGroup::Pinned
                    } else {
                        message_ops::date_group(row.created_at, now)
                    };
                    if last_group != Some(group) {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(group.label().to_uppercase())
                                .small()
                                .strong()
                                .monospace()
                                .color(palette.text_muted),
                        );
                        last_group = Some(group);
                    }

                    let selected = active_id == Some(row.id);
                    let editing_this = state
                        .renaming_chat
                        .as_ref()
                        .is_some_and(|(id, _)| *id == row.id);
                    ui.vertical(|ui| {
                        if editing_this {
                            if let Some((_, draft)) = state.renaming_chat.as_mut() {
                                let resp = ui.add(
                                    egui::TextEdit::singleline(draft)
                                        .desired_width(ui.available_width() - 56.0),
                                );
                                if resp.lost_focus()
                                    && ui.input(|i| {
                                        i.key_pressed(egui::Key::Enter) || i.pointer.any_click()
                                    })
                                {
                                    request.rename = Some((row.id, draft.clone()));
                                }
                            }
                        } else {
                            let title = truncate(&row.title, 28);
                            let label = ui.selectable_label(selected, title);
                            if label.clicked() {
                                request.select = Some(row.id);
                            }
                            if label.double_clicked() {
                                request.start_rename = Some(row.id);
                            }
                            if !query.trim().is_empty()
                                && let Some(snippet) = &row.snippet
                            {
                                ui.label(
                                    egui::RichText::new(snippet).small().color(palette.accent),
                                );
                            }
                        }

                        // Keep row actions below the title so narrow archive panels never
                        // obscure the chat name. The selected label supplies the only active
                        // treatment; no extra marker glyph is needed.
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("↓")
                                .on_hover_text("Export Markdown")
                                .clicked()
                            {
                                request.export = Some(row.id);
                            }
                            let pin = if row.pinned { "★" } else { "☆" };
                            if ui.small_button(pin).on_hover_text("Pin to top").clicked() {
                                request.toggle_pin = Some(row.id);
                            }
                            if ui.small_button("🗑").on_hover_text("Delete chat").clicked() {
                                request.delete = Some(row.id);
                            }
                        });
                    });
                    ui.add_space(3.0);
                }
            });
    });

    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.add_space(8.0);
        let signout_button = crate::ui::theme::action_button(
            ui,
            "EJECT  /  CHANGE KEY",
            crate::ui::theme::Tone::Danger,
        )
        .min_size(egui::vec2(ui.available_width(), 28.0));
        if ui
            .add(signout_button)
            .on_hover_text("Remove the cached API key and return to onboarding")
            .clicked()
        {
            request.eject = true;
        }
        ui.add_space(4.0);
        let settings_button =
            crate::ui::theme::action_button(ui, "SETTINGS", crate::ui::theme::Tone::Neutral)
                .min_size(egui::vec2(ui.available_width(), 28.0));
        if ui.add(settings_button).clicked() {
            request.settings = true;
        }
    });

    request
}

struct ChatRow {
    id: Uuid,
    title: String,
    pinned: bool,
    created_at: chrono::DateTime<Utc>,
    snippet: Option<String>,
}

impl ChatRow {
    fn from_chat(chat: &Chat, query: &str) -> Self {
        let snippet = if query.trim().is_empty() {
            None
        } else {
            chat.messages
                .iter()
                .find_map(|m| message_ops::search_snippet(&m.content, query, 18))
        };
        Self {
            id: chat.id,
            title: chat.title.clone(),
            pinned: chat.pinned,
            created_at: chat.created_at,
            snippet,
        }
    }
}
