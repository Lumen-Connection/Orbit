use crate::app::{App, OnboardingStatus, Screen};
use crate::secure_store::SecureStore;
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(64.0);

            ui.label(
                egui::RichText::new("ORBIT // INITIAL LINK")
                    .size(26.0)
                    .strong()
                    .monospace()
                    .color(crate::ui::theme::tokens(ui).text_primary),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Connect an OpenRouter key to open the operations console.")
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            ui.add_space(28.0);

            crate::ui::theme::panel_toned(ui, crate::ui::theme::Tone::Accent)
                .inner_margin(egui::Margin::same(20))
                .show(ui, |ui| {
                    ui.set_max_width(440.0);

                    let Screen::Onboarding(state) = &mut app.screen else {
                        return;
                    };

                    crate::ui::theme::section_header(ui, "CREDENTIAL LINK");
                    ui.add_space(4.0);

                    let response = ui.add(
                        egui::TextEdit::singleline(&mut state.key_input)
                            .password(!state.show_key)
                            .hint_text("sk-or-v1-…")
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut state.show_key, "SHOW KEY");
                        ui.add_space(8.0);
                        ui.hyperlink_to("Get a key", "https://openrouter.ai/keys");
                    });

                    ui.add_space(14.0);

                    let validating = matches!(state.status, OnboardingStatus::Validating);

                    let submit_clicked = ui
                        .add_enabled(
                            !validating,
                            crate::ui::theme::action_button(
                                ui,
                                if validating {
                                    "Validating…"
                                } else {
                                    "CONTINUE"
                                },
                                crate::ui::theme::Tone::Accent,
                            )
                            .min_size(egui::vec2(120.0, 32.0)),
                        )
                        .clicked();

                    let enter_pressed = response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !validating;

                    if submit_clicked || enter_pressed {
                        app.start_validation();
                        return;
                    }

                    ui.add_space(10.0);
                    match &state.status {
                        OnboardingStatus::Idle => {}
                        OnboardingStatus::Validating => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Checking your key with OpenRouter…");
                            });
                        }
                        OnboardingStatus::Error(msg) => {
                            ui.colored_label(crate::ui::theme::tokens(ui).danger, msg);
                        }
                    }
                });

            ui.add_space(20.0);
            ui.label(
                egui::RichText::new(format!(
                    "Your key is stored in {} and never written to disk in plain text.",
                    SecureStore::display_name()
                ))
                .small()
                .color(crate::ui::theme::tokens(ui).text_muted),
            );
        });
    });
}
