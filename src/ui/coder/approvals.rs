//! Inline approval cards. Not a modal.

use crate::security::ApprovalDecision;
use crate::session::ApprovalHandle;
use crate::ui::widgets;
use eframe::egui;

pub fn render(
    ui: &mut egui::Ui,
    handle: &ApprovalHandle,
    resolved: Option<ApprovalDecision>,
) -> Option<ApprovalDecision> {
    let mut decision = None;
    ui.add_space(8.0);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(format!("Approval · {}", handle.tool_name))
                    .strong()
                    .color(crate::ui::theme::tokens(ui).warning),
            );
            ui.label(egui::RichText::new(&handle.summary).italics());
            if let Some(patch) = &handle.patch {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(patch.relative_path.display().to_string())
                        .monospace()
                        .strong(),
                );
                widgets::diff::render(ui, &patch.original_content, &patch.proposed_content);
            }
            if let Some(command) = &handle.command {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(command.display()).monospace().strong());
            }
            ui.add_space(6.0);
            match resolved {
                None => {
                    let approve_label = if handle.command.is_some() {
                        "Allow"
                    } else {
                        "Apply"
                    };
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(approve_label).min_size(egui::vec2(80.0, 28.0)))
                            .clicked()
                        {
                            decision = Some(ApprovalDecision::Approved);
                        }
                        if ui
                            .add(egui::Button::new("Deny").min_size(egui::vec2(80.0, 28.0)))
                            .clicked()
                        {
                            decision = Some(ApprovalDecision::Denied);
                        }
                    });
                }
                Some(ApprovalDecision::Approved) => {
                    let msg = if handle.command.is_some() {
                        "Allowed."
                    } else {
                        "Applied."
                    };
                    ui.colored_label(crate::ui::theme::tokens(ui).success, msg);
                }
                Some(ApprovalDecision::Denied) => {
                    ui.colored_label(crate::ui::theme::tokens(ui).danger, "Denied.");
                }
            }
        });
    decision
}
