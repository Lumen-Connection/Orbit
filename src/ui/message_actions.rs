//! Hover toolbar and discard-confirmation for Chat / Coder messages.

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverAction {
    Copy,
    Regenerate,
    Edit,
    Delete,
}

pub fn hover_bar(
    ui: &mut egui::Ui,
    hovered: bool,
    can_regenerate: bool,
    can_edit: bool,
    enabled: bool,
) -> Option<HoverAction> {
    if !hovered {
        return None;
    }
    let mut action = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let btn = |ui: &mut egui::Ui, label: &str, tip: &str, on: bool| {
            ui.add_enabled(enabled && on, egui::Button::new(label).small())
                .on_hover_text(tip)
                .clicked()
        };
        if btn(ui, "Copy", "Copy message text", true) {
            action = Some(HoverAction::Copy);
        }
        if can_regenerate && btn(ui, "Regenerate", "Discard this reply and ask again", true) {
            action = Some(HoverAction::Regenerate);
        }
        if can_edit && btn(ui, "Edit", "Edit and resend, dropping later messages", true) {
            action = Some(HoverAction::Edit);
        }
        if btn(ui, "Delete", "Remove this question and its reply", true) {
            action = Some(HoverAction::Delete);
        }
    });
    action
}

pub fn copy_code_button(ui: &mut egui::Ui, body: &str) -> bool {
    ui.add(egui::Button::new("Copy").small())
        .on_hover_text("Copy this code block")
        .clicked()
        && !body.is_empty()
}

/// `Some(true)` confirm, `Some(false)` dismiss, `None` still open.
pub fn confirm_discard(
    ctx: &egui::Context,
    title: &str,
    detail: &str,
    confirm_label: &str,
) -> Option<bool> {
    let mut open = true;
    let mut decision = None;
    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(detail);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(confirm_label).min_size(egui::vec2(100.0, 28.0)))
                    .clicked()
                {
                    decision = Some(true);
                }
                if ui
                    .add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 28.0)))
                    .clicked()
                {
                    decision = Some(false);
                }
            });
        });
    if !open {
        return Some(false);
    }
    decision
}
