//! Unified-style diff: context, additions and removals.

use eframe::egui;
use similar::{ChangeTag, TextDiff};

pub fn render(ui: &mut egui::Ui, original: &str, proposed: &str) {
    let diff = TextDiff::from_lines(original, proposed);
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for change in diff.iter_all_changes() {
                let palette = crate::ui::theme::tokens(ui);
                let (prefix, color, fill) = match change.tag() {
                    ChangeTag::Delete => ('-', palette.diff_del_fg, palette.diff_del_bg),
                    ChangeTag::Insert => ('+', palette.diff_add_fg, palette.diff_add_bg),
                    ChangeTag::Equal => (' ', palette.diff_eq_fg, palette.diff_eq_bg),
                };
                let line = format!("{prefix}{}", change.value().trim_end_matches('\n'));
                egui::Frame::new()
                    .fill(fill)
                    .inner_margin(egui::Margin::symmetric(6, 1))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(egui::RichText::new(line).monospace().color(color));
                    });
            }
        });
}

#[cfg(test)]
mod tests {
    use similar::{ChangeTag, TextDiff};

    #[test]
    fn classifies_add_remove_and_equal() {
        let diff = TextDiff::from_lines("keep\ngone\n", "keep\nnew\n");
        let tags: Vec<ChangeTag> = diff.iter_all_changes().map(|c| c.tag()).collect();
        assert!(tags.contains(&ChangeTag::Equal));
        assert!(tags.contains(&ChangeTag::Delete));
        assert!(tags.contains(&ChangeTag::Insert));
    }
}
