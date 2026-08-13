use crate::app::{App, Screen};
use crate::workspace::FileNode;
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut switch = false;
    let mut close = false;
    ui.horizontal(|ui| {
        ui.heading("Explorer");
        let Screen::Main(state) = &app.screen else {
            return;
        };
        if state.coder.scanning {
            ui.spinner();
        }
        if state.coder.restore_rx.is_some() {
            ui.spinner();
            ui.label(
                egui::RichText::new("Restoring…")
                    .small()
                    .italics()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Close")
                .on_hover_text("Close project")
                .clicked()
            {
                close = true;
            }
            if ui
                .small_button("Switch…")
                .on_hover_text("Open another project")
                .clicked()
            {
                switch = true;
            }
        });
    });
    if close {
        app.request_close_project();
        return;
    }
    if switch {
        app.browse_for_project();
        return;
    }
    ui.add_space(4.0);

    let mut open = None;
    {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };
        let selected = state.coder.selected.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                render_nodes(
                    ui,
                    &mut state.coder.tree.children,
                    selected.as_deref(),
                    &mut open,
                );
            });
    }
    if let Some(path) = open {
        app.select_file(path);
    }
}

fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &mut [FileNode],
    selected: Option<&std::path::Path>,
    open: &mut Option<std::path::PathBuf>,
) {
    for node in nodes {
        if node.is_dir {
            egui::CollapsingHeader::new(format!("📁 {}", node.name))
                .id_salt(&node.relative)
                .default_open(false)
                .show(ui, |ui| {
                    render_nodes(ui, &mut node.children, selected, open);
                });
        } else {
            let is_selected = selected.is_some_and(|p| p == node.relative.as_path());
            if ui
                .selectable_label(is_selected, format!("📄 {}", node.name))
                .clicked()
            {
                *open = Some(node.relative.clone());
            }
        }
    }
}
