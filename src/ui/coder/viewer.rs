use crate::app::{App, Screen};
use crate::coder::ViewerBody;
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Viewer");
    ui.add_space(4.0);
    let Screen::Main(state) = &app.screen else {
        return;
    };
    let title = state
        .coder
        .viewer
        .relative
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "No file selected".into());
    ui.label(
        egui::RichText::new(title)
            .small()
            .color(crate::ui::theme::tokens(ui).text_muted),
    );
    ui.add_space(4.0);

    egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .show(ui, |ui| match &state.coder.viewer.body {
            ViewerBody::Empty => {
                ui.label(
                    egui::RichText::new("Select a file to read it.")
                        .italics()
                        .color(crate::ui::theme::tokens(ui).text_muted),
                );
            }
            ViewerBody::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading…");
                });
            }
            ViewerBody::Error(err) => {
                ui.colored_label(crate::ui::theme::tokens(ui).danger, err);
            }
            ViewerBody::Text { plain, highlighted } => {
                if let Some(job) = highlighted {
                    ui.label(job.clone());
                } else {
                    ui.label(egui::RichText::new(plain).monospace());
                    ui.label(
                        egui::RichText::new("Highlight disabled for files over 5,000 lines.")
                            .small()
                            .italics()
                            .color(crate::ui::theme::tokens(ui).text_muted),
                    );
                }
            }
        });
}
