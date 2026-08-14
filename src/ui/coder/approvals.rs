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
    crate::ui::theme::panel_toned(ui, crate::ui::theme::Tone::Warning)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(format!(
                    "REVIEW REQUIRED  //  {}",
                    handle.tool_name.to_uppercase()
                ))
                .strong()
                .monospace()
                .color(crate::ui::theme::tokens(ui).warning),
            );
            ui.label(
                egui::RichText::new("Agent operation is paused until you choose an outcome.")
                    .small()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&handle.summary).strong());
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
                        let approve_button = crate::ui::theme::action_button(
                            ui,
                            approve_label,
                            crate::ui::theme::Tone::Success,
                        )
                        .min_size(egui::vec2(96.0, 30.0));
                        if ui.add(approve_button).clicked() {
                            decision = Some(ApprovalDecision::Approved);
                        }
                        let deny_button = crate::ui::theme::action_button(
                            ui,
                            "DENY",
                            crate::ui::theme::Tone::Danger,
                        )
                        .min_size(egui::vec2(96.0, 30.0));
                        if ui.add(deny_button).clicked() {
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
