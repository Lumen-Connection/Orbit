//! Bottom run panel: one tab per process, virtualized output.

use crate::app::{App, Screen};
use crate::runner::{ProcessStatus, ProcessView};
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    app.poll_runner();

    let mut stop = None;
    let mut restart_id = None;
    let mut play = None;
    let mut clear = None;
    let mut select: Option<String> = None;
    let mut confirm_restart = false;
    let mut decline_restart = false;

    {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };

        if let Some(pending) = &state.coder.run_restart_prompt {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.label(format!(
                        "`{}` is already running. Restart it?",
                        pending.name
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Restart").clicked() {
                            confirm_restart = true;
                        }
                        if ui.button("Back").clicked() {
                            decline_restart = true;
                        }
                    });
                });
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            ui.heading("Run");
            ui.add_space(8.0);
            let runner = state.coder.runner.lock().ok();
            let mut ids: Vec<String> = runner
                .as_ref()
                .map(|r| r.processes.keys().cloned().collect())
                .unwrap_or_default();
            ids.sort();
            if ids.is_empty() {
                ui.label(
                    egui::RichText::new("No long-running process yet.")
                        .italics()
                        .color(crate::ui::theme::tokens(ui).text_muted),
                );
            }
            for id in &ids {
                let Some(proc) = runner.as_ref().and_then(|r| r.processes.get(id)) else {
                    continue;
                };
                let mark = match proc.status {
                    ProcessStatus::Running | ProcessStatus::Starting => " ●",
                    ProcessStatus::Stopping => " …",
                    _ => "",
                };
                if ui
                    .selectable_label(false, format!("{}{mark}", proc.name))
                    .clicked()
                {
                    select = Some(id.clone());
                }
            }
        });

        let selected = select.clone().or_else(|| {
            state
                .coder
                .runner
                .lock()
                .ok()
                .and_then(|r| r.processes.keys().next().cloned())
        });
        let Some(id) = selected else {
            render_configs(ui, state, &mut play);
            return;
        };
        let Some(proc) = state
            .coder
            .runner
            .lock()
            .ok()
            .and_then(|r| r.processes.get(&id).cloned())
        else {
            return;
        };
        render_process_header(ui, &proc, &mut stop, &mut restart_id, &mut clear);
        render_output(ui, &proc);
    }

    if confirm_restart {
        app.confirm_run_restart();
    }
    if decline_restart {
        app.decline_run_restart();
    }
    if let Some(id) = stop {
        app.stop_run(&id);
    }
    if let Some(id) = restart_id
        && let Screen::Main(state) = &app.screen
        && let Some(config) = state
            .coder
            .run_configs
            .iter()
            .chain(state.coder.suggested_runs.iter())
            .find(|c| c.id == id)
            .cloned()
    {
        app.restart_run(config);
    }
    if let Some(id) = play {
        app.request_run(&id);
    }
    if let Some(id) = clear
        && let Screen::Main(state) = &mut app.screen
        && let Ok(mut runner) = state.coder.runner.lock()
    {
        runner.clear_output(&id);
    }
}

fn render_configs(ui: &mut egui::Ui, state: &crate::app::MainState, play: &mut Option<String>) {
    ui.add_space(6.0);
    for config in state
        .coder
        .run_configs
        .iter()
        .chain(state.coder.suggested_runs.iter())
        .filter(|c| c.kind == crate::workspace::run_config::RunKind::LongRunning)
    {
        ui.horizontal(|ui| {
            ui.label(&config.name);
            if ui.small_button("▶").on_hover_text("Start").clicked() {
                *play = Some(config.id.clone());
            }
        });
    }
}

fn render_process_header(
    ui: &mut egui::Ui,
    proc: &ProcessView,
    stop: &mut Option<String>,
    restart: &mut Option<String>,
    clear: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        let status = match proc.status {
            ProcessStatus::Starting => "starting".into(),
            ProcessStatus::Running => format!(
                "running · pid {} · {}",
                proc.pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "—".into()),
                proc.duration_label()
            ),
            ProcessStatus::Stopping => "stopping".into(),
            ProcessStatus::Exited => format!("exit {}", proc.exit_code.unwrap_or(-1)),
            ProcessStatus::Failed => "failed".into(),
        };
        ui.label(egui::RichText::new(status).small().monospace());
        if matches!(
            proc.status,
            ProcessStatus::Running | ProcessStatus::Starting
        ) {
            if ui.small_button("■").on_hover_text("Stop").clicked() {
                *stop = Some(proc.config_id.clone());
            }
        } else if ui.small_button("▶").on_hover_text("Start").clicked() {
            // replay last config id
            *stop = None;
        }
        if ui.small_button("↻").on_hover_text("Restart").clicked() {
            *restart = Some(proc.config_id.clone());
        }
        if ui.small_button("Clear").clicked() {
            *clear = Some(proc.config_id.clone());
        }
    });
}

fn render_output(ui: &mut egui::Ui, proc: &ProcessView) {
    let row_h = 16.0;
    let n = proc.lines.len();
    egui::ScrollArea::vertical()
        .id_salt(format!("run_out_{}", proc.config_id))
        .auto_shrink([false; 2])
        .stick_to_bottom(proc.follow)
        .show_rows(ui, row_h, n, |ui, range| {
            for i in range {
                let Some(line) = proc.lines.get(i) else {
                    continue;
                };
                if line.spans.is_empty() {
                    ui.label(egui::RichText::new(&line.text).monospace().size(12.5));
                } else {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for span in &line.spans {
                            ui.label(
                                egui::RichText::new(&span.text)
                                    .monospace()
                                    .size(12.5)
                                    .color(crate::ui::theme::ansi_rgb(span.r, span.g, span.b)),
                            );
                        }
                    });
                }
            }
        });
}
