mod attachments;
mod chat;
pub(crate) mod coder;
pub(crate) mod message_actions;
mod onboarding;
mod settings;
pub(crate) mod shortcuts;
pub(crate) mod theme;
pub(crate) mod widgets;

use crate::app::{App, OnboardingStatus, Screen};
use crate::coder::AppMode;
use eframe::egui;
use std::time::Duration;

pub(crate) const FADE_DURATION: Duration = Duration::from_millis(300);

/// Returns `true` when the user asks to retry initialization.
pub fn render_init_error(ui: &mut egui::Ui, message: &str) -> bool {
    let mut retry = false;
    egui::CentralPanel::default()
        .frame(theme::panel(ui))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                ui.label(
                    egui::RichText::new("ORBIT // STARTUP FAULT")
                        .size(24.0)
                        .strong()
                        .monospace(),
                );
                ui.add_space(12.0);
                ui.label(egui::RichText::new(message).color(theme::tokens(ui).danger));
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "If this mentions {}, start or unlock it and try again.",
                        crate::secure_store::SecureStore::display_name()
                    ))
                    .color(crate::ui::theme::tokens(ui).text_muted),
                );
                ui.add_space(20.0);
                let retry_button = theme::action_button(ui, "TRY AGAIN", theme::Tone::Accent)
                    .min_size(egui::vec2(140.0, 32.0));
                if ui.add(retry_button).clicked() {
                    retry = true;
                }
            });
        });
    retry
}

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    apply_appearance(app, ui.ctx());
    theme::paint_grid(ui);
    dispatch_shortcuts(app, ui.ctx());
    app.poll_validation();
    app.poll_catalog();
    app.poll_key_test();
    if matches!(
        &app.screen,
        Screen::Onboarding(s) if matches!(s.status, OnboardingStatus::Validating)
    ) || matches!(
        &app.screen,
        Screen::Main(s) if matches!(s.settings_ui.test_status, crate::app::KeyTestStatus::Testing)
    ) {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    match &mut app.screen {
        Screen::Onboarding(_) => onboarding::render(app, ui),
        Screen::Main(_) => {
            let (mode, busy) = match &app.screen {
                Screen::Main(state) => (
                    state.mode,
                    state.coder.scanning
                        || state.coder.sessions.any_busy()
                        || state.coder.terminal.running
                        || matches!(state.coder.viewer.body, crate::coder::ViewerBody::Loading),
                ),
                _ => (AppMode::Chat, false),
            };
            if busy {
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
            coder::render_mode_bar(app, ui);
            widgets::credential_banner::render(app, ui);
            match mode {
                AppMode::Chat => chat::render(app, ui),
                AppMode::Coder => coder::render(app, ui),
            }
            settings::render(app, ui);
        }
    }
}

pub(crate) fn with_alpha(c: egui::Color32, a: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn apply_appearance(app: &App, ctx: &egui::Context) {
    if let Screen::Main(state) = &app.screen {
        theme::apply(
            ctx,
            state.settings.theme,
            state.settings.font_scale,
            state.settings.motion,
        );
    }
}

fn dispatch_shortcuts(app: &mut App, ctx: &egui::Context) {
    let Screen::Main(_) = &app.screen else {
        return;
    };
    let text_focused = ctx.memory(|m| m.focused().is_some());
    let Some(id) = shortcuts::consume(ctx, text_focused) else {
        return;
    };
    let (mode, settings_open, confirm_open, editing) = match &app.screen {
        Screen::Main(state) => (
            state.mode,
            state.settings_ui.open,
            state.pending_confirm.is_some(),
            state.editing_chat.is_some() || state.editing_coder.is_some(),
        ),
        _ => return,
    };
    match id {
        shortcuts::ShortcutId::NewSession => match mode {
            AppMode::Chat => app.new_chat(),
            AppMode::Coder => app.new_coder_session(),
        },
        shortcuts::ShortcutId::Search => {
            if mode != AppMode::Chat
                && let Screen::Main(state) = &mut app.screen
            {
                state.mode = AppMode::Chat;
            }
            if let Screen::Main(state) = &mut app.screen {
                state.focus_search_next_frame = true;
            }
        }
        shortcuts::ShortcutId::Cancel => {
            if confirm_open {
                if let Screen::Main(state) = &mut app.screen {
                    state.pending_confirm = None;
                }
            } else if settings_open {
                app.close_settings();
            } else if editing {
                if let Screen::Main(state) = &mut app.screen {
                    state.editing_chat = None;
                    state.editing_coder = None;
                }
            } else {
                match mode {
                    AppMode::Chat => app.cancel_pending(),
                    AppMode::Coder => app.cancel_coder_turn(),
                }
            }
        }
        shortcuts::ShortcutId::NextSession => match mode {
            AppMode::Chat => app.cycle_chat(1),
            AppMode::Coder => app.cycle_coder_session(1),
        },
        shortcuts::ShortcutId::PrevSession => match mode {
            AppMode::Chat => app.cycle_chat(-1),
            AppMode::Coder => app.cycle_coder_session(-1),
        },
        shortcuts::ShortcutId::Settings => app.open_settings(crate::app::SettingsTab::Credentials),
        shortcuts::ShortcutId::Send => match mode {
            AppMode::Chat => app.send_message(),
            AppMode::Coder => app.send_coder_prompt(),
        },
        shortcuts::ShortcutId::FontBigger => app.nudge_font_scale(0.1),
        shortcuts::ShortcutId::FontSmaller => app.nudge_font_scale(-0.1),
        shortcuts::ShortcutId::FontReset => app.reset_font_scale(),
        shortcuts::ShortcutId::ToggleMode => {
            if let Screen::Main(state) = &mut app.screen {
                state.mode = match state.mode {
                    AppMode::Chat => AppMode::Coder,
                    AppMode::Coder => AppMode::Chat,
                };
            }
        }
    }
}
