use crate::app::{App, OnboardingStatus, Screen};
use crate::secure_store::SecureStore;
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);

            ui.heading("Welcome to Orbit");
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Connect your OpenRouter API key to get started.")
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            ui.add_space(28.0);

            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(20))
                .show(ui, |ui| {
                    ui.set_max_width(440.0);

                    let Screen::Onboarding(state) = &mut app.screen else {
                        return;
                    };

                    ui.label("OpenRouter API key");
                    ui.add_space(4.0);

                    let response = ui.add(
                        egui::TextEdit::singleline(&mut state.key_input)
                            .password(!state.show_key)
                            .hint_text("sk-or-v1-…")
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut state.show_key, "Show key");
                        ui.add_space(8.0);
                        ui.hyperlink_to("Get a key", "https://openrouter.ai/keys");
                    });

                    ui.add_space(14.0);

                    let validating = matches!(state.status, OnboardingStatus::Validating);

                    let submit_clicked = ui
                        .add_enabled(
                            !validating,
                            egui::Button::new(if validating {
                                "Validating…"
                            } else {
                                "Continue"
                            })
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
