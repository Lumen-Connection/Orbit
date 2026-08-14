//! Right-hand Project Context panel.

use crate::app::{App, Screen};
use crate::context::TaskStatus;
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    app.refresh_context_if_stale();

    let mut open_folder = false;
    {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("PROJECT CONTEXT // HANDOFF LOG")
                    .small()
                    .strong()
                    .monospace()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            let open_button =
                crate::ui::theme::action_button(ui, "OPEN .ORBIT", crate::ui::theme::Tone::Accent)
                    .small();
            if ui.add(open_button).clicked() {
                open_folder = true;
            }
        });
        ui.add_space(6.0);

        let Some(store) = state.coder.store.clone() else {
            ui.label(
                egui::RichText::new("Open a project to create .orbit/.")
                    .italics()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            return;
        };
        let Ok(store) = store.lock() else {
            ui.colored_label(crate::ui::theme::tokens(ui).danger, "context lock busy");
            return;
        };

        if !store.warnings.is_empty() {
            crate::ui::theme::panel_toned(ui, crate::ui::theme::Tone::Warning).show(ui, |ui| {
                ui.colored_label(
                    crate::ui::theme::tokens(ui).warning,
                    store.warnings.join(" · "),
                );
            });
            ui.add_space(6.0);
        }

        let open_tasks = store
            .tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Done)
            .count();

        egui::CollapsingHeader::new(format!("Decisions ({})", store.decisions.len()))
            .id_salt("ctx_decisions")
            .default_open(state.coder.expand_decisions)
            .show(ui, |ui| {
                if store.decisions.is_empty() {
                    ui.label(muted(ui, "None yet."));
                }
                for decision in store.decisions.iter().rev().take(20) {
                    ui.label(
                        egui::RichText::new(format!("• {}", decision.decision))
                            .small()
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!("{} · {}", decision.model, decision.session))
                            .small()
                            .color(crate::ui::theme::tokens(ui).text_muted),
                    );
                }
            });

        egui::CollapsingHeader::new(format!("Tasks ({open_tasks} open)"))
            .id_salt("ctx_tasks")
            .default_open(state.coder.expand_tasks)
            .show(ui, |ui| {
                if store.tasks.is_empty() {
                    ui.label(muted(ui, "None yet."));
                }
                for task in &store.tasks {
                    ui.label(
                        egui::RichText::new(format!(
                            "[{}] {} — {}",
                            task.status.as_str(),
                            task.id,
                            task.description
                        ))
                        .small(),
                    );
                }
            });

        egui::CollapsingHeader::new(format!("Findings ({})", store.findings.len()))
            .id_salt("ctx_findings")
            .default_open(state.coder.expand_findings)
            .show(ui, |ui| {
                if store.findings.is_empty() {
                    ui.label(muted(ui, "None yet."));
                }
                for finding in store.findings.iter().rev().take(20) {
                    ui.label(
                        egui::RichText::new(format!(
                            "[{}] {}",
                            finding.severity, finding.description
                        ))
                        .small(),
                    );
                }
            });

        ui.add_space(10.0);
        let project_name = state
            .coder
            .project
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let session_id = state.coder.sessions.active().map(|s| s.id.clone());
        let mut prompt = match &session_id {
            Some(id) => {
                crate::session::agent_loop::compose_coder_system(Some(&store), id, &project_name)
            }
            None => crate::session::agent_loop::CODER_SYSTEM_PROMPT.to_string(),
        };
        egui::CollapsingHeader::new("System prompt")
            .id_salt("ctx_system_prompt")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Built by Orbit (agent instructions + Project Context). Not editable.",
                    )
                    .small()
                    .italics()
                    .color(crate::ui::theme::tokens(ui).text_muted),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut prompt)
                                .desired_width(f32::INFINITY)
                                .desired_rows(10)
                                .font(egui::TextStyle::Monospace)
                                .interactive(false),
                        );
                    });
            });

        ui.add_space(10.0);
        crate::ui::theme::section_header(ui, "FILES CHANGED BY SESSION");
        ui.add_space(4.0);
        let mut any = false;
        for rec in &store.sessions {
            for touch in &rec.touched {
                any = true;
                ui.label(
                    egui::RichText::new(format!("• {}  ({})", touch.path, rec.label))
                        .small()
                        .monospace(),
                );
            }
        }
        if !any {
            ui.label(muted(ui, "No recorded file changes yet."));
        }
    }

    if open_folder {
        app.open_orbit_folder();
    }
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    super::run_configs::render(app, ui);
}

fn muted(ui: &egui::Ui, text: &str) -> egui::RichText {
    let _ = ui;
    egui::RichText::new(text)
        .italics()
        .color(crate::ui::theme::tokens(ui).text_muted)
}
