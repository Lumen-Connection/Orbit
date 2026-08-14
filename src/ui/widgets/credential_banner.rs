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
        .frame(crate::ui::theme::panel_toned(
            ui,
            crate::ui::theme::Tone::Warning,
        ))
        .show(ui, |ui| {
            let Screen::Main(state) = &mut app.screen else {
                return;
            };
            ui.horizontal_centered(|ui| {
                ui.colored_label(
                    crate::ui::theme::tokens(ui).warning,
                    "KEY REJECTED // CONVERSATIONS INTACT",
                );
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut state.banner_key_input)
                        .password(!state.banner_show_key)
                        .hint_text("sk-or-v1-…")
                        .desired_width(220.0),
                );
                ui.checkbox(&mut state.banner_show_key, "SHOW");
                let testing = matches!(state.settings_ui.test_status, KeyTestStatus::Testing);
                if ui
                    .add_enabled(
                        !testing,
                        crate::ui::theme::action_button(
                            ui,
                            if testing { "TRYING…" } else { "TRY AGAIN" },
                            crate::ui::theme::Tone::Warning,
                        ),
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
