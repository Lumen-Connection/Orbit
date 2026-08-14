//! Always-visible token and cost meter.

use crate::session::manager::LiveSession;
use eframe::egui;

pub fn render(ui: &mut egui::Ui, session: &LiveSession) {
    if let Some(occ) = session.context_occupancy {
        let palette = crate::ui::theme::tokens(ui);
        // Heighten visibility: turn the occupancy color when we approach the
        // context limit (>= 80%) so the user sees the warning at a glance.
        let occ_color = if occ >= 0.8 {
            palette.warning
        } else {
            palette.text_muted
        };
        ui.label(
            egui::RichText::new(crate::session::context_window::occupancy_label(occ))
                .small()
                .color(occ_color),
        );
        ui.add_space(6.0);
    }
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
        palette.text_muted
    };
    let latency = session
        .last_latency_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_else(|| "—".into());
    ui.horizontal(|ui| {
        let cached = if session.cached_tokens > 0 {
            format!(" · cached {c}", c = session.cached_tokens)
        } else {
            String::new()
        };
        ui.label(
            egui::RichText::new(format!(
                "in {in_t} · out {out_t}{cached} · ${cost:.4} / ${cap:.2} · {latency} · iter {iter}",
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
        if (0.8..1.0).contains(&ratio) {
            ui.label(
                egui::RichText::new("80% of budget")
                    .small()
                    .color(crate::ui::theme::tokens(ui).warning),
            );
        }
    });
}
