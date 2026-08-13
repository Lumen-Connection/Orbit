//! Saved and suggested run configs.

use crate::app::{App, Screen};
use crate::workspace::run_config::{RunConfig, RunKind};
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut run = None;
    let mut adopt = None;
    let mut edit = None;
    let mut add = false;

    {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        ui.heading("Run");
        ui.add_space(4.0);
        if state.coder.run_configs.is_empty() && state.coder.suggested_runs.is_empty() {
            ui.label(
                egui::RichText::new("No run configs detected.")
                    .italics()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
        }
        for config in &state.coder.run_configs {
            render_row(ui, config, false, &mut run, &mut edit);
        }
        if !state.coder.suggested_runs.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Suggested")
                    .small()
                    .strong()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            for config in &state.coder.suggested_runs {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&config.name).small());
                    ui.label(
                        egui::RichText::new(kind_label(config.kind))
                            .small()
                            .color(crate::ui::theme::tokens(ui).text_muted),
                    );
                    if ui.small_button("Save").clicked() {
                        adopt = Some(config.id.clone());
                    }
                    if ui.small_button("Run").clicked() {
                        run = Some(config.id.clone());
                    }
                });
            }
        }
        ui.add_space(6.0);
        if ui.small_button("+ Add run config").clicked() {
            add = true;
        }
    }

    if add && let Screen::Main(state) = &mut app.screen {
        state.coder.run_editor = Some(RunConfig::new(
            "new run",
            "cargo",
            vec!["test".into()],
            RunKind::OneShot,
        ));
    }
    if let Some(id) = adopt {
        app.adopt_suggested_run(&id);
    }
    if let Some(id) = edit
        && let Screen::Main(state) = &mut app.screen
        && let Some(config) = state.coder.run_configs.iter().find(|c| c.id == id).cloned()
    {
        state.coder.run_editor = Some(config);
    }
    if let Some(id) = run {
        app.request_run(&id);
    }

    render_editor(app, ui);
    render_approval(app, ui);
}

fn render_row(
    ui: &mut egui::Ui,
    config: &RunConfig,
    _suggested: bool,
    run: &mut Option<String>,
    edit: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        ui.label(&config.name);
        ui.label(
            egui::RichText::new(kind_label(config.kind))
                .small()
                .color(crate::ui::theme::tokens(ui).text_muted),
        );
        if ui.small_button("Run").clicked() {
            *run = Some(config.id.clone());
        }
        if ui.small_button("Edit").clicked() {
            *edit = Some(config.id.clone());
        }
    });
}

fn kind_label(kind: RunKind) -> &'static str {
    match kind {
        RunKind::OneShot => "one-shot",
        RunKind::LongRunning => "long-running",
    }
}

fn render_editor(app: &mut App, ui: &mut egui::Ui) {
    let Screen::Main(state) = &app.screen else {
        return;
    };
    if state.coder.run_editor.is_none() {
        return;
    }
    let mut save = false;
    let mut close = false;
    egui::Window::new("Run config")
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .show(ui.ctx(), |ui| {
            let Screen::Main(state) = &mut app.screen else {
                return;
            };
            let Some(config) = state.coder.run_editor.as_mut() else {
                return;
            };
            ui.label("Name");
            ui.text_edit_singleline(&mut config.name);
            ui.label("Program");
            ui.text_edit_singleline(&mut config.program);
            ui.label("Arguments (space-separated)");
            let mut args = config.args.join(" ");
            if ui.text_edit_singleline(&mut args).changed() {
                config.args = args.split_whitespace().map(ToString::to_string).collect();
            }
            ui.horizontal(|ui| {
                ui.label("Kind");
                ui.selectable_value(&mut config.kind, RunKind::OneShot, "One-shot");
                ui.selectable_value(&mut config.kind, RunKind::LongRunning, "Long-running");
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if save && let Screen::Main(state) = &mut app.screen {
        if let Some(config) = state.coder.run_editor.clone() {
            app.upsert_run_config(config);
        }
    } else if close && let Screen::Main(state) = &mut app.screen {
        state.coder.run_editor = None;
    }
}

fn render_approval(app: &mut App, ui: &mut egui::Ui) {
    let Screen::Main(state) = &app.screen else {
        return;
    };
    let Some(config) = state.coder.run_pending_approval.clone() else {
        return;
    };
    let mut approve = false;
    let mut deny = false;
    egui::Window::new("Approve run config?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(400.0)
        .show(ui.ctx(), |ui| {
            ui.label("This command has not been approved on this machine:");
            ui.add_space(6.0);
            ui.label(egui::RichText::new(config.display()).monospace().strong());
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Editing the program, args, or env later will ask again.")
                    .small()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Approve and run").clicked() {
                    approve = true;
                }
                if ui.button("Cancel").clicked() {
                    deny = true;
                }
            });
        });
    if approve {
        app.confirm_run_approval();
    } else if deny {
        app.decline_run_approval();
    }
}
