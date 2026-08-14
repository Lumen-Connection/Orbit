mod approvals;
mod context_panel;
mod explorer;
mod pipeline;
mod run_configs;
mod run_panel;
mod sessions;
mod viewer;

use crate::app::{App, Screen};
use crate::coder::AppMode;
use crate::ui::widgets;
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    app.poll_scan();
    app.poll_viewer();
    app.poll_agent();
    app.poll_terminal();
    app.poll_restore();
    app.poll_usage_report();
    app.poll_pipeline();

    let has_project = matches!(
        &app.screen,
        Screen::Main(state) if state.coder.project.is_some()
    );

    if !has_project {
        render_welcome(app, ui);
        return;
    }

    egui::Panel::left("coder_explorer")
        .resizable(true)
        .default_size(280.0)
        .size_range(200.0..=480.0)
        .show(ui, |ui| {
            let avail = ui.available_height();
            ui.allocate_ui(egui::vec2(ui.available_width(), avail * 0.5), |ui| {
                explorer::render(app, ui);
            });
            ui.separator();
            viewer::render(app, ui);
        });

    egui::Panel::bottom("coder_terminal")
        .resizable(true)
        .default_size(200.0)
        .size_range(80.0..=480.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("coder_bottom_stack")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    run_panel::render(app, ui);
                    ui.separator();
                    render_terminal(app, ui);
                });
        });

    egui::Panel::right("coder_context")
        .resizable(true)
        .default_size(240.0)
        .size_range(180.0..=400.0)
        .show(ui, |ui| {
            context_panel::render(app, ui);
        });

    egui::CentralPanel::default().show(ui, |ui| {
        sessions::render(app, ui);
        render_pending_patches(app, ui);
    });

    render_usage_window(app, ui);
    render_switch_dialog(app, ui);
}

fn render_switch_dialog(app: &mut App, ui: &mut egui::Ui) {
    let Screen::Main(state) = &app.screen else {
        return;
    };
    let Some(prompt) = state.coder.switch_prompt.clone() else {
        return;
    };
    let mut confirm = false;
    let mut back = false;
    let mut open = true;
    egui::Window::new("Switch project?")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(420.0)
        .show(ui.ctx(), |ui| {
            ui.label("This project still has work in progress:");
            ui.add_space(6.0);
            ui.label(egui::RichText::new(prompt.work.summary_line()).strong());
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(
                    "Cancel all and switch will stop generations, deny pending approvals, and unload this project.",
                )
                .small()
                .color(crate::ui::theme::tokens(ui).text_muted),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel all and switch").clicked() {
                    confirm = true;
                }
                if ui.button("Back").clicked() {
                    back = true;
                }
            });
        });
    if confirm {
        app.confirm_switch();
    } else if back || !open {
        app.cancel_switch();
    }
}

fn render_welcome(app: &mut App, ui: &mut egui::Ui) {
    let needs_load = matches!(
        &app.screen,
        Screen::Main(state) if !state.coder.projects_loaded
    );
    if needs_load {
        app.refresh_project_registry();
    }

    let mut open_typed = false;
    let mut open_recent: Option<std::path::PathBuf> = None;
    let mut browse = false;
    let mut forget: Option<String> = None;
    let mut locate: Option<String> = None;

    egui::CentralPanel::default().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.heading("Coder Mode");
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Open a local project to browse files and review diffs.")
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            ui.add_space(24.0);
        });

        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 480.0).max(0.0) / 2.0);
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.set_width(480.0);
                    let Screen::Main(state) = &mut app.screen else {
                        return;
                    };
                    ui.label("Project folder");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.coder.path_input)
                            .desired_width(f32::INFINITY)
                            .hint_text(r"C:\dev\my-project"),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open folder").clicked() {
                            open_typed = true;
                        }
                        if ui.button("Browse…").clicked() {
                            browse = true;
                        }
                    });
                    if let Some(status) = &state.coder.status {
                        ui.add_space(8.0);
                        ui.colored_label(crate::ui::theme::tokens(ui).danger, status);
                    }
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("Recent projects")
                            .small()
                            .strong()
                            .color(crate::ui::theme::tokens(ui).text_muted),
                    );
                    ui.separator();
                    if state.coder.projects.is_empty() {
                        ui.label(
                            egui::RichText::new("No recent projects yet.")
                                .italics()
                                .color(crate::ui::theme::tokens(ui).text_muted),
                        );
                    }
                    for project in &state.coder.projects {
                        let missing = matches!(
                            project.availability,
                            crate::workspace::registry::ProjectAvailability::Unavailable
                        );
                        ui.horizontal(|ui| {
                            let label = format!(
                                "{}  —  {}  ·  {} sessions",
                                project.name,
                                project.path.display(),
                                project.session_count
                            );
                            if missing {
                                ui.add_enabled(
                                    false,
                                    egui::Label::new(egui::RichText::new(label).weak()),
                                )
                                .on_disabled_hover_text(
                                    "Folder is missing. Use Locate… to re-bind.",
                                );
                                if ui.small_button("Locate…").clicked() {
                                    locate = Some(project.id.clone());
                                }
                            } else if ui.link(label).clicked() {
                                open_recent = Some(project.path.clone());
                            }
                            if ui
                                .small_button("✕")
                                .on_hover_text("Remove from history")
                                .clicked()
                            {
                                forget = Some(project.id.clone());
                            }
                        });
                    }
                });
        });
    });

    if browse {
        app.browse_for_project();
        app.set_mode(AppMode::Coder);
    }
    if open_typed && let Screen::Main(state) = &app.screen {
        let typed = state.coder.path_input.trim().to_string();
        if !typed.is_empty() {
            app.open_project_path(std::path::PathBuf::from(typed));
        }
    }
    if let Some(path) = open_recent {
        app.open_project_path(path);
    }
    if let Some(id) = forget {
        app.forget_project(&id);
    }
    if let Some(id) = locate
        && let Some(path) = rfd::FileDialog::new().pick_folder()
    {
        app.rebind_project(&id, path);
    }
}

fn render_terminal(app: &mut App, ui: &mut egui::Ui) {
    let mut cancel = false;
    {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        ui.horizontal(|ui| {
            ui.strong("Terminal");
            if let Some(cmd) = &state.coder.terminal.command {
                ui.label(egui::RichText::new(cmd).monospace().small());
            }
            if state.coder.terminal.running {
                ui.spinner();
                if let Some(started) = state.coder.terminal.started_at {
                    ui.label(format!("{:.1}s", started.elapsed().as_secs_f32()));
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            } else if let Some(code) = state.coder.terminal.exit_code {
                ui.label(format!("exit {code}"));
                if let Some(ms) = state.coder.terminal.duration_ms {
                    ui.label(format!("{:.2}s", ms as f64 / 1000.0));
                }
                if state.coder.terminal.timed_out {
                    ui.colored_label(crate::ui::theme::tokens(ui).warning, "timed out");
                }
            }
        });
        ui.add_space(4.0);
        let view = state.coder.terminal.view().to_string();
        egui::ScrollArea::vertical()
            .id_salt("coder_terminal_scroll")
            .stick_to_bottom(true)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.label(egui::RichText::new(view).monospace().size(12.5));
            });
    }
    if cancel {
        app.cancel_coder_command();
    }
}

fn render_pending_patches(app: &mut App, ui: &mut egui::Ui) {
    let Screen::Main(state) = &app.screen else {
        return;
    };
    if state.coder.pending_patches.is_empty() {
        return;
    }
    ui.separator();
    ui.heading("Pending diffs");
    let mut apply_at = None;
    for (idx, patch) in state.coder.pending_patches.iter().enumerate() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "{} · {:?}",
                patch.relative_path.display(),
                patch.status
            ))
            .strong(),
        );
        widgets::diff::render(ui, &patch.original_content, &patch.proposed_content);
        if matches!(patch.status, crate::workspace::PatchStatus::Pending)
            && ui.button("Apply").clicked()
        {
            apply_at = Some(idx);
        }
    }
    if let Some(idx) = apply_at {
        app.apply_selected_patch(idx);
    }
}

pub fn render_mode_bar(app: &mut App, ui: &mut egui::Ui) {
    let mut next_mode = None;
    let mut open_usage = false;
    let mut open_settings = false;
    egui::Panel::top("mode_bar")
        .exact_size(36.0)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let Screen::Main(state) = &app.screen else {
                    return;
                };
                let title = state
                    .coder
                    .project
                    .as_ref()
                    .map(|p| format!("Orbit — {}", p.name))
                    .unwrap_or_else(|| "Orbit".into());
                ui.strong(title);
                ui.add_space(16.0);
                if ui
                    .selectable_label(state.mode == AppMode::Chat, "Chat Mode")
                    .clicked()
                {
                    next_mode = Some(AppMode::Chat);
                }
                if ui
                    .selectable_label(state.mode == AppMode::Coder, "Coder Mode")
                    .clicked()
                {
                    next_mode = Some(AppMode::Coder);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Settings").clicked() {
                        open_settings = true;
                    }
                    if ui.small_button("Usage").clicked() {
                        open_usage = true;
                    }
                });
            });
        });
    if let Some(mode) = next_mode {
        app.set_mode(mode);
    }
    if open_usage {
        app.request_usage_report();
    }
    if open_settings {
        app.open_settings(crate::app::SettingsTab::Credentials);
    }
}

fn render_usage_window(app: &mut App, ui: &mut egui::Ui) {
    let Screen::Main(state) = &app.screen else {
        return;
    };
    if !state.coder.show_usage {
        return;
    }
    let mut open = true;
    egui::Window::new("Usage")
        .open(&mut open)
        .resizable(true)
        .default_width(420.0)
        .show(ui.ctx(), |ui| {
            let Screen::Main(state) = &app.screen else {
                return;
            };
            let Some(report) = &state.coder.usage_report else {
                ui.label("Loading usage…");
                return;
            };
            ui.label(format!(
                "Total  ${:.4}   in {} · out {}",
                report.total_cost, report.total_input, report.total_output
            ));
            ui.separator();
            ui.strong("By project");
            for row in &report.by_project {
                ui.label(format!(
                    "{}  ${:.4}  ({} / {})",
                    row.key, row.cost_usd, row.input_tokens, row.output_tokens
                ));
            }
            ui.add_space(8.0);
            ui.strong("By model");
            for row in &report.by_model {
                ui.label(format!(
                    "{}  ${:.4}  ({} / {})",
                    row.key, row.cost_usd, row.input_tokens, row.output_tokens
                ));
            }
            ui.add_space(8.0);
            ui.strong("By day");
            for row in &report.by_day {
                ui.label(format!(
                    "{}  ${:.4}  ({} / {})",
                    row.key, row.cost_usd, row.input_tokens, row.output_tokens
                ));
            }
        });
    if !open && let Screen::Main(state) = &mut app.screen {
        state.coder.show_usage = false;
    }
}
