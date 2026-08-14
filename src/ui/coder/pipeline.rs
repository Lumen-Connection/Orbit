//! N3.1 — Pipeline configuration dialog.

use crate::app::{App, Screen};
use crate::pipeline::{Complexity, GitGateMode};
use crate::providers::catalog::MODEL_GROUPS;
use crate::ui::theme::tokens;
use eframe::egui;

pub fn render_dialog(app: &mut App, ui: &mut egui::Ui) {
    let open = matches!(
        &app.screen,
        Screen::Main(state) if state.coder.pipeline_dialog.is_some()
    );
    if !open {
        return;
    }
    let mut confirm = false;
    let mut cancel = false;
    let palette = tokens(ui);
    egui::Window::new("New pipeline")
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            let Screen::Main(state) = &mut app.screen else {
                return;
            };
            let Some(cfg) = state.coder.pipeline_dialog.as_mut() else {
                return;
            };
            ui.label("Feature");
            ui.add(
                egui::TextEdit::multiline(&mut cfg.feature)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("What should this pipeline do?"),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Complexity");
                egui::ComboBox::from_id_salt("pipeline_complexity")
                    .selected_text(cfg.complexity.label())
                    .show_ui(ui, |ui| {
                        for option in Complexity::all() {
                            ui.selectable_value(&mut cfg.complexity, option, option.label());
                        }
                    });
            });
            ui.add_space(8.0);
            stage_combo(ui, "Planner", &mut cfg.planner.auto, &mut cfg.planner.model);
            stage_combo(ui, "Coder", &mut cfg.coder.auto, &mut cfg.coder.model);
            stage_combo(
                ui,
                "Reviewer",
                &mut cfg.reviewer.auto,
                &mut cfg.reviewer.model,
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Git Gate");
                egui::ComboBox::from_id_salt("pipeline_git_gate")
                    .selected_text("Manual")
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut cfg.git_gate, GitGateMode::Manual, "Manual");
                    });
            });
            ui.label(
                egui::RichText::new("The Git Gate is not an LLM. Approval is always manual.")
                    .small()
                    .color(palette.text_muted),
            );
            ui.add_space(8.0);
            ui.checkbox(
                &mut cfg.auto_planner_to_coder,
                "Auto-advance Planner → Coder",
            );
            ui.checkbox(
                &mut cfg.auto_coder_to_reviewer,
                "Auto-advance Coder → Reviewer",
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Create sessions").clicked() && !cfg.feature.trim().is_empty() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if confirm {
        app.confirm_pipeline_dialog();
    } else if cancel && let Screen::Main(state) = &mut app.screen {
        state.coder.pipeline_dialog = None;
    }
}

fn stage_combo(ui: &mut egui::Ui, label: &str, auto: &mut bool, model: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.checkbox(auto, "Auto");
        ui.add_enabled_ui(!*auto, |ui| {
            let shown = if model.is_empty() {
                "Select model"
            } else {
                model.as_str()
            };
            egui::ComboBox::from_id_salt(format!("pipeline_model_{label}"))
                .selected_text(shown)
                .width(280.0)
                .show_ui(ui, |ui| {
                    for group in MODEL_GROUPS {
                        ui.label(
                            egui::RichText::new(group.provider.to_uppercase())
                                .small()
                                .strong(),
                        );
                        for entry in group.models {
                            if ui
                                .selectable_label(*model == entry.id, entry.name)
                                .clicked()
                            {
                                *model = entry.id.to_string();
                            }
                        }
                    }
                });
        });
    });
}

pub fn render_banner(app: &mut App, ui: &mut egui::Ui) {
    let mut start = false;
    let mut cancel = false;
    {
        let Screen::Main(state) = &app.screen else {
            return;
        };
        if state.coder.pipeline.is_none() {
            return;
        }
    }
    let palette = tokens(ui);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            let Screen::Main(state) = &app.screen else {
                return;
            };
            let Some(pipeline) = &state.coder.pipeline else {
                return;
            };
            ui.label(
                egui::RichText::new(format!("Pipeline · {}", pipeline.config.feature))
                    .strong()
                    .color(palette.accent),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} · current: {}",
                    pipeline.config.complexity.label(),
                    pipeline.current.label()
                ))
                .small()
                .color(palette.text_muted),
            );
            if pipeline.waiting_git_gate {
                ui.colored_label(palette.warning, "Git Gate waiting for manual approval.");
            }
            if let Some(reason) = &pipeline.stopped_reason {
                ui.colored_label(palette.warning, reason);
            }
            for block in &pipeline.transcript {
                ui.label(
                    egui::RichText::new(format!("{}: {}", block.stage.label(), block.text))
                        .small()
                        .color(palette.text_muted),
                );
            }
            ui.horizontal(|ui| {
                if pipeline.stopped_reason.is_none() && ui.small_button("Start").clicked() {
                    start = true;
                }
                if ui.small_button("Cancel pipeline").clicked() {
                    cancel = true;
                }
            });
        });
    if start {
        app.start_pipeline();
    }
    if cancel {
        app.cancel_pipeline();
    }
}
