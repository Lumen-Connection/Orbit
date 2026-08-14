//! Always-visible token, latency, and cost telemetry.

use crate::session::manager::LiveSession;
use eframe::egui;

pub fn render(ui: &mut egui::Ui, session: &LiveSession) {
    let ratio = if session.budget_usd > 0.0 {
        session.spent_usd / session.budget_usd
    } else {
        0.0
    };
    let palette = crate::ui::theme::tokens(ui);
    let color = if ratio >= 1.0 {
        palette.danger
    } else if ratio >= 0.8 {
        palette.warning
    } else {
        palette.accent
    };
    let latency = session
        .last_latency_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_else(|| "—".into());

    crate::ui::theme::panel(ui).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("SESSION TELEMETRY")
                    .small()
                    .strong()
                    .monospace()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
            if let Some(occ) = session.context_occupancy {
                ui.label(
                    egui::RichText::new(crate::session::context_window::occupancy_label(occ))
                        .small()
                        .monospace()
                        .color(crate::ui::theme::tokens(ui).text_muted),
                );
            }
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "IN {in_t}  OUT {out_t}  ${cost:.4} / ${cap:.2}  {latency}  ITER {iter}",
                in_t = session.prompt_tokens,
                out_t = session.completion_tokens,
                cost = session.spent_usd,
                cap = session.budget_usd,
                iter = session.iteration
            ))
            .small()
            .monospace()
            .color(color),
        );
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(1), palette.divider);
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                rect.min,
                egui::pos2(
                    rect.left() + rect.width() * (ratio as f32).min(1.0),
                    rect.bottom(),
                ),
            ),
            egui::CornerRadius::same(1),
            color,
        );
        if (0.8..1.0).contains(&ratio) {
            ui.label(
                egui::RichText::new("80% OF BUDGET")
                    .small()
                    .monospace()
                    .color(crate::ui::theme::tokens(ui).warning),
            );
        }
    });
}
