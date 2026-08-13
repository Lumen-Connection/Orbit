//! Persistent banner when the stored API key is rejected at runtime.

use crate::app::{App, CredentialState, KeyTestStatus, Screen};
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    let rejected = matches!(
        &app.screen,
        Screen::Main(state) if state.credential.state == CredentialState::Rejected
    );
    if !rejected {
        return;
    }

    let mut retry = false;
    egui::Panel::top("credential_banner")
        .exact_size(52.0)
        .show(ui, |ui| {
            let Screen::Main(state) = &mut app.screen else {
                return;
            };
            ui.horizontal_centered(|ui| {
                ui.colored_label(
                    crate::ui::theme::tokens(ui).warning,
                    "⚠ API key rejected. Conversations are intact.",
                );
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut state.banner_key_input)
                        .password(!state.banner_show_key)
                        .hint_text("sk-or-v1-…")
                        .desired_width(220.0),
                );
                ui.checkbox(&mut state.banner_show_key, "Show");
                let testing = matches!(state.settings_ui.test_status, KeyTestStatus::Testing);
                if ui
                    .add_enabled(
                        !testing,
                        egui::Button::new(if testing { "Trying…" } else { "Try again" }),
                    )
                    .clicked()
                {
                    retry = true;
                }
            });
        });
    if retry {
        app.start_banner_retry();
    }
}
