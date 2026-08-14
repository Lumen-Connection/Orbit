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
        .frame(crate::ui::theme::panel(ui).inner_margin(egui::Margin::same(8)))
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
        .frame(crate::ui::theme::panel(ui).inner_margin(egui::Margin::same(8)))
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
        .frame(crate::ui::theme::panel(ui).inner_margin(egui::Margin::same(8)))
        .show(ui, |ui| {
            context_panel::render(app, ui);
        });

    egui::CentralPanel::default()
        .frame(crate::ui::theme::panel(ui).inner_margin(egui::Margin::same(10)))
        .show(ui, |ui| {
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
        egui::ScrollArea::vertical()
            .id_salt("coder_project_intake")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(36.0);
                    ui.label(
                        egui::RichText::new("CODER // PROJECT INTAKE")
                            .size(24.0)
                            .strong()
                            .monospace()
                            .color(crate::ui::theme::tokens(ui).text_primary),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "Select a local workspace to begin an agent operation.",
                        )
                        .color(crate::ui::theme::tokens(ui).text_muted),
                    );
                    ui.add_space(22.0);
                });

                let available_width = ui.available_width();
                let content_width = (available_width - 32.0).clamp(320.0, 680.0);
                let side_space = ((available_width - content_width) / 2.0).max(0.0);
                ui.horizontal(|ui| {
                    ui.add_space(side_space);
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let Screen::Main(state) = &mut app.screen else {
                                return;
                            };
                            let inner_width = (content_width - 32.0).max(260.0);
                            crate::ui::theme::panel_toned(ui, crate::ui::theme::Tone::Accent)
                                .inner_margin(egui::Margin::same(16))
                                .show(ui, |ui| {
                                    ui.set_width(inner_width);
                                    crate::ui::theme::section_header(ui, "PROJECT FOLDER");
                                    ui.label(
                                        egui::RichText::new(
                                            "Enter a path directly or choose a folder from disk.",
                                        )
                                        .small()
                                        .color(crate::ui::theme::tokens(ui).text_muted),
                                    );
                                    ui.add_space(8.0);
                                    ui.add_sized(
                                        egui::vec2(ui.available_width(), 34.0),
                                        egui::TextEdit::singleline(
                                            &mut state.coder.path_input,
                                        )
                                        .hint_text(r"C:\dev\my-project"),
                                    );
                                    ui.add_space(10.0);
                                    ui.horizontal(|ui| {
                                        let open_button = crate::ui::theme::action_button(
                                            ui,
                                            "OPEN PATH",
                                            crate::ui::theme::Tone::Accent,
                                        )
                                        .min_size(egui::vec2(120.0, 32.0));
                                        if ui.add(open_button).clicked() {
                                            open_typed = true;
                                        }
                                        let browse_button = crate::ui::theme::action_button(
                                            ui,
                                            "BROWSE FOLDERS…",
                                            crate::ui::theme::Tone::Neutral,
                                        )
                                        .min_size(egui::vec2(140.0, 32.0));
                                        if ui.add(browse_button).clicked() {
                                            browse = true;
                                        }
                                    });
                                    if let Some(status) = &state.coder.status {
                                        ui.add_space(8.0);
                                        crate::ui::theme::panel_toned(
                                            ui,
                                            crate::ui::theme::Tone::Danger,
                                        )
                                        .inner_margin(egui::Margin::same(8))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(status)
                                                    .small()
                                                    .color(
                                                        crate::ui::theme::tokens(ui).danger,
                                                    ),
                                            );
                                        });
                                    }
                                });

                            ui.add_space(12.0);
                            crate::ui::theme::panel(ui)
                                .inner_margin(egui::Margin::same(16))
                                .show(ui, |ui| {
                                    ui.set_width(inner_width);
                                    crate::ui::theme::section_header(ui, "RECENT PROJECTS");
                                    ui.label(
                                        egui::RichText::new(
                                            "Resume a workspace with its sessions and pending reviews.",
                                        )
                                        .small()
                                        .color(crate::ui::theme::tokens(ui).text_muted),
                                    );
                                    ui.add_space(8.0);
                                    if state.coder.projects.is_empty() {
                                        ui.label(
                                            egui::RichText::new("No recent projects yet.")
                                                .italics()
                                                .color(
                                                    crate::ui::theme::tokens(ui).text_muted,
                                                ),
                                        );
                                    }
                                    for project in &state.coder.projects {
                                        let missing = matches!(
                                            project.availability,
                                            crate::workspace::registry::ProjectAvailability::Unavailable
                                        );
                                        let row_width = ui.available_width();
                                        crate::ui::theme::panel_toned(
                                            ui,
                                            if missing {
                                                crate::ui::theme::Tone::Warning
                                            } else {
                                                crate::ui::theme::Tone::Neutral
                                            },
                                        )
                                        .inner_margin(egui::Margin::same(10))
                                        .show(ui, |ui| {
                                            ui.set_width((row_width - 20.0).max(220.0));
                                            ui.label(
                                                egui::RichText::new(&project.name)
                                                    .strong()
                                                    .color(
                                                        crate::ui::theme::tokens(ui).text_primary,
                                                    ),
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(
                                                        project.path.display().to_string(),
                                                    )
                                                    .small()
                                                    .monospace()
                                                    .color(
                                                        crate::ui::theme::tokens(ui).text_muted,
                                                    ),
                                                )
                                                .wrap(),
                                            );
                                            ui.add_space(4.0);
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} SESSIONS  //  {} PENDING PATCHES",
                                                    project.session_count,
                                                    project.pending_patches
                                                ))
                                                .small()
                                                .monospace()
                                                .color(if missing {
                                                    crate::ui::theme::tokens(ui).warning
                                                } else {
                                                    crate::ui::theme::tokens(ui).text_muted
                                                }),
                                            );
                                            ui.add_space(6.0);
                                            ui.horizontal(|ui| {
                                                if missing {
                                                    let locate_button =
                                                        crate::ui::theme::action_button(
                                                            ui,
                                                            "LOCATE…",
                                                            crate::ui::theme::Tone::Warning,
                                                        )
                                                        .small();
                                                    if ui.add(locate_button).clicked() {
                                                        locate = Some(project.id.clone());
                                                    }
                                                } else {
                                                    let open_button =
                                                        crate::ui::theme::action_button(
                                                            ui,
                                                            "OPEN PROJECT",
                                                            crate::ui::theme::Tone::Accent,
                                                        )
                                                        .small();
                                                    if ui.add(open_button).clicked() {
                                                        open_recent =
                                                            Some(project.path.clone());
                                                    }
                                                }
                                                let remove_button =
                                                    crate::ui::theme::action_button(
                                                        ui,
                                                        "REMOVE",
                                                        crate::ui::theme::Tone::Danger,
                                                    )
                                                    .small();
                                                if ui
                                                    .add(remove_button)
                                                    .on_hover_text("Remove from history")
                                                    .clicked()
                                                {
                                                    forget = Some(project.id.clone());
                                                }
                                            });
                                        });
                                        ui.add_space(8.0);
                                    }
                                });
                        },
                    );
                });
                ui.add_space(36.0);
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
            ui.label(
                egui::RichText::new("TERMINAL // LIVE OUTPUT")
                    .small()
                    .strong()
                    .monospace()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            if let Some(cmd) = &state.coder.terminal.command {
                ui.label(egui::RichText::new(cmd).monospace().small());
            }
            if state.coder.terminal.running {
                if state.settings.motion == crate::storage::MotionPreference::Full {
                    ui.spinner();
                }
                if let Some(started) = state.coder.terminal.started_at {
                    ui.label(format!("{:.1}s", started.elapsed().as_secs_f32()));
                }
                let cancel_button =
                    crate::ui::theme::action_button(ui, "CANCEL", crate::ui::theme::Tone::Danger)
                        .small();
                if ui.add(cancel_button).clicked() {
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
    ui.label(
        egui::RichText::new("PENDING DIFFS // REVIEW QUEUE")
            .strong()
            .monospace()
            .color(crate::ui::theme::tokens(ui).text_primary),
    );
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
        if matches!(patch.status, crate::workspace::PatchStatus::Pending) {
            let apply_button =
                crate::ui::theme::action_button(ui, "APPLY PATCH", crate::ui::theme::Tone::Success);
            if ui.add(apply_button).clicked() {
                apply_at = Some(idx);
            }
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
        .exact_size(48.0)
        .frame(crate::ui::theme::panel(ui).inner_margin(egui::Margin::symmetric(12, 8)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let Screen::Main(state) = &app.screen else {
                    return;
                };
                let project = state
                    .coder
                    .project
                    .as_ref()
                    .map(|p| p.name.as_str())
                    .unwrap_or("NO PROJECT");
                ui.label(
                    egui::RichText::new("ORBIT")
                        .strong()
                        .size(17.0)
                        .monospace()
                        .color(crate::ui::theme::tokens(ui).text_primary),
                );
                ui.label(
                    egui::RichText::new("// OPERATIONS")
                        .small()
                        .monospace()
                        .color(crate::ui::theme::tokens(ui).accent),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(project.to_uppercase())
                        .small()
                        .monospace()
                        .color(crate::ui::theme::tokens(ui).text_muted),
                );
                ui.add_space(12.0);
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
                    let credential_tone = match state.credential.state {
                        crate::app::CredentialState::Present => crate::ui::theme::Tone::Success,
                        crate::app::CredentialState::Rejected => crate::ui::theme::Tone::Warning,
                        crate::app::CredentialState::Missing => crate::ui::theme::Tone::Danger,
                    };
                    ui.label(crate::ui::theme::status_text(
                        ui,
                        match state.credential.state {
                            crate::app::CredentialState::Present => "LINKED",
                            crate::app::CredentialState::Rejected => "KEY REJECTED",
                            crate::app::CredentialState::Missing => "KEY REQUIRED",
                        },
                        credential_tone,
                    ));
                    let header_action_size = egui::vec2(78.0, 28.0);
                    if ui
                        .add_sized(
                            header_action_size,
                            crate::ui::theme::action_button(
                                ui,
                                egui::RichText::new("SETTINGS").monospace().size(12.0),
                                crate::ui::theme::Tone::Neutral,
                            ),
                        )
                        .clicked()
                    {
                        open_settings = true;
                    }
                    if state.mode == AppMode::Coder
                        && ui
                            .add_sized(
                                header_action_size,
                                crate::ui::theme::action_button(
                                    ui,
                                    egui::RichText::new("USAGE").monospace().size(12.0),
                                    crate::ui::theme::Tone::Accent,
                                ),
                            )
                            .clicked()
                    {
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
