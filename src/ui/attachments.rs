//! Paste / drop image attachments shared by Chat and Coder.

use crate::providers::ImageAttachment;
use eframe::egui;

pub fn capture(ui: &egui::Ui) -> Vec<ImageAttachment> {
    let mut out = Vec::new();
    let paste = ui.input(|i| {
        i.events.iter().any(|e| match e {
            egui::Event::Paste(_) => true,
            egui::Event::Key {
                key: egui::Key::V,
                pressed: true,
                modifiers,
                ..
            } => modifiers.command,
            _ => false,
        })
    });
    if paste && let Some(img) = crate::media::from_clipboard() {
        out.push(img);
    }
    ui.ctx().input(|i| {
        for file in &i.raw.dropped_files {
            if let Ok(bytes) = file.bytes() {
                if let Some(img) = crate::media::from_bytes(&bytes) {
                    out.push(img);
                }
            } else if let Some(img) = crate::media::from_path(file.path()) {
                out.push(img);
            }
        }
    });
    out
}

pub fn draft_strip(
    ui: &mut egui::Ui,
    images: &mut Vec<ImageAttachment>,
    lightbox: &mut Option<ImageAttachment>,
) {
    if images.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        let mut remove = None;
        for (idx, image) in images.iter().enumerate() {
            if ui
                .add(egui::Button::new(format!("🖼 {}×{}", image.width, image.height)).small())
                .clicked()
            {
                *lightbox = Some(image.clone());
            }
            if ui.small_button("✕").clicked() {
                remove = Some(idx);
            }
        }
        if let Some(idx) = remove {
            images.remove(idx);
        }
    });
}

pub fn lightbox_window(ctx: &egui::Context, image: &mut Option<ImageAttachment>) {
    let Some(current) = image.clone() else {
        return;
    };
    let mut open = true;
    egui::Window::new("Image")
        .open(&mut open)
        .default_width(480.0)
        .show(ctx, |ui| {
            ui.label(format!(
                "{}  {}×{}",
                current.mime, current.width, current.height
            ));
            ui.label(
                egui::RichText::new("(embedded PNG, sent as a data URI)")
                    .small()
                    .color(crate::ui::theme::tokens(ui).text_muted),
            );
        });
    if !open {
        *image = None;
    }
}
