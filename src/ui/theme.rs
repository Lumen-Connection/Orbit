//! Orbit's semantic visual system.
//!
//! Renderers should ask this module for tokens and component frames instead of
//! inventing colors. That keeps the operational-console language consistent
//! across Chat, Coder, settings, onboarding, and approval surfaces.

use crate::storage::{MotionPreference, ThemePreference};
use eframe::egui;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Active,
    Success,
    Warning,
    Danger,
}

/// Contrast-checked semantic colors for the app's panels and status language.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: egui::Color32,
    pub surface: egui::Color32,
    pub surface_alt: egui::Color32,
    pub elevated: egui::Color32,
    pub bubble_user: egui::Color32,
    pub bubble_assistant: egui::Color32,
    pub text_primary: egui::Color32,
    pub text_on_user: egui::Color32,
    pub text_muted: egui::Color32,
    pub border: egui::Color32,
    pub border_strong: egui::Color32,
    pub divider: egui::Color32,
    pub danger: egui::Color32,
    pub warning: egui::Color32,
    pub success: egui::Color32,
    pub accent: egui::Color32,
    pub active: egui::Color32,
    pub diff_add_fg: egui::Color32,
    pub diff_add_bg: egui::Color32,
    pub diff_del_fg: egui::Color32,
    pub diff_del_bg: egui::Color32,
    pub diff_eq_fg: egui::Color32,
    pub diff_eq_bg: egui::Color32,
}

pub fn dark_palette() -> Palette {
    Palette {
        background: egui::Color32::from_rgb(13, 16, 19),
        surface: egui::Color32::from_rgb(20, 24, 28),
        surface_alt: egui::Color32::from_rgb(25, 30, 35),
        elevated: egui::Color32::from_rgb(31, 37, 43),
        bubble_user: egui::Color32::from_rgb(27, 75, 87),
        bubble_assistant: egui::Color32::from_rgb(24, 29, 34),
        text_primary: egui::Color32::from_rgb(235, 240, 242),
        text_on_user: egui::Color32::from_rgb(240, 253, 255),
        text_muted: egui::Color32::from_rgb(150, 163, 169),
        border: egui::Color32::from_rgb(57, 68, 75),
        border_strong: egui::Color32::from_rgb(91, 108, 114),
        divider: egui::Color32::from_rgb(42, 51, 57),
        danger: egui::Color32::from_rgb(244, 105, 91),
        warning: egui::Color32::from_rgb(235, 169, 75),
        success: egui::Color32::from_rgb(99, 206, 146),
        accent: egui::Color32::from_rgb(78, 202, 217),
        active: egui::Color32::from_rgb(129, 224, 236),
        diff_add_fg: egui::Color32::from_rgb(166, 237, 181),
        diff_add_bg: egui::Color32::from_rgb(22, 60, 43),
        diff_del_fg: egui::Color32::from_rgb(255, 177, 165),
        diff_del_bg: egui::Color32::from_rgb(70, 31, 34),
        diff_eq_fg: egui::Color32::from_rgb(177, 188, 191),
        diff_eq_bg: egui::Color32::from_rgb(24, 29, 34),
    }
}

pub fn light_palette() -> Palette {
    Palette {
        background: egui::Color32::from_rgb(232, 235, 233),
        surface: egui::Color32::from_rgb(247, 248, 246),
        surface_alt: egui::Color32::from_rgb(238, 242, 240),
        elevated: egui::Color32::from_rgb(255, 255, 253),
        bubble_user: egui::Color32::from_rgb(205, 236, 239),
        bubble_assistant: egui::Color32::from_rgb(247, 248, 246),
        text_primary: egui::Color32::from_rgb(29, 37, 39),
        text_on_user: egui::Color32::from_rgb(20, 45, 49),
        text_muted: egui::Color32::from_rgb(86, 104, 108),
        border: egui::Color32::from_rgb(168, 183, 184),
        border_strong: egui::Color32::from_rgb(105, 126, 129),
        divider: egui::Color32::from_rgb(201, 211, 210),
        danger: egui::Color32::from_rgb(184, 55, 47),
        warning: egui::Color32::from_rgb(161, 99, 17),
        success: egui::Color32::from_rgb(29, 126, 79),
        accent: egui::Color32::from_rgb(18, 129, 145),
        active: egui::Color32::from_rgb(16, 99, 114),
        diff_add_fg: egui::Color32::from_rgb(17, 105, 54),
        diff_add_bg: egui::Color32::from_rgb(218, 243, 224),
        diff_del_fg: egui::Color32::from_rgb(156, 38, 32),
        diff_del_bg: egui::Color32::from_rgb(255, 226, 222),
        diff_eq_fg: egui::Color32::from_rgb(74, 88, 91),
        diff_eq_bg: egui::Color32::from_rgb(247, 248, 246),
    }
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        dark_palette()
    } else {
        light_palette()
    }
}

pub fn tokens(ui: &egui::Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

pub fn tone_color(palette: Palette, tone: Tone) -> egui::Color32 {
    match tone {
        Tone::Neutral => palette.text_muted,
        Tone::Accent => palette.accent,
        Tone::Active => palette.active,
        Tone::Success => palette.success,
        Tone::Warning => palette.warning,
        Tone::Danger => palette.danger,
    }
}

/// A deliberately square, thinly framed panel used throughout Orbit.
pub fn panel(ui: &egui::Ui) -> egui::Frame {
    let palette = tokens(ui);
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(2))
        .inner_margin(egui::Margin::same(10))
}

pub fn panel_toned(ui: &egui::Ui, tone: Tone) -> egui::Frame {
    let palette = tokens(ui);
    egui::Frame::new()
        .fill(if tone == Tone::Neutral {
            palette.surface
        } else {
            palette.elevated
        })
        .stroke(egui::Stroke::new(1.0, tone_color(palette, tone)))
        .corner_radius(egui::CornerRadius::same(2))
        .inner_margin(egui::Margin::same(10))
}

pub fn action_button(
    ui: &egui::Ui,
    label: impl Into<egui::WidgetText>,
    tone: Tone,
) -> egui::Button<'static> {
    let palette = tokens(ui);
    let color = tone_color(palette, tone);
    egui::Button::new(label)
        .fill(match tone {
            Tone::Accent | Tone::Active => with_alpha(color, 42),
            Tone::Success => with_alpha(color, 34),
            Tone::Warning => with_alpha(color, 30),
            Tone::Danger => with_alpha(color, 36),
            Tone::Neutral => palette.surface_alt,
        })
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(egui::CornerRadius::same(2))
}

pub fn status_text(ui: &egui::Ui, label: impl Into<String>, tone: Tone) -> egui::RichText {
    egui::RichText::new(label.into())
        .small()
        .strong()
        .monospace()
        .color(tone_color(tokens(ui), tone))
}

pub fn section_header(ui: &mut egui::Ui, label: &str) {
    let palette = tokens(ui);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label.to_uppercase())
                .small()
                .strong()
                .monospace()
                .color(palette.text_muted),
        );
        ui.add_space(6.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().line_segment(
            [rect.left_center(), rect.right_center()],
            egui::Stroke::new(1.0, palette.divider),
        );
    });
}

pub fn paint_grid(ui: &egui::Ui) {
    let palette = tokens(ui);
    let rect = ui.max_rect();
    let stroke = egui::Stroke::new(1.0, with_alpha(palette.border, 22));
    let step = 28.0;
    let mut x = rect.left() + step;
    while x < rect.right() {
        ui.painter().line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            stroke,
        );
        x += step;
    }
    let mut y = rect.top() + step;
    while y < rect.bottom() {
        ui.painter().line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            stroke,
        );
        y += step;
    }
}

pub fn motion_progress(time: f64, motion: MotionPreference, period: f64) -> f32 {
    if motion == MotionPreference::Reduced || period <= 0.0 {
        return 1.0;
    }
    (time.rem_euclid(period) / period) as f32
}

pub fn request_motion_repaint(ui: &egui::Ui, motion: MotionPreference) {
    if motion == MotionPreference::Full {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(90));
    }
}

/// Install the two regular IBM Plex faces while preserving EGUI's emoji and
/// fallback fonts. Called once during native startup, not once per frame.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "OrbitSans".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/orbit-sans.ttf"
        ))),
    );
    fonts.font_data.insert(
        "OrbitMono".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/orbit-mono.ttf"
        ))),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "OrbitSans".into());
    }
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        family.insert(0, "OrbitMono".into());
    }
    ctx.set_fonts(fonts);
}

/// Apply the stored preference, semantic visuals, typography scale, and
/// animation cadence. Both palettes are configured so switching themes does
/// not briefly expose EGUI's stock colors in popups.
pub fn apply(
    ctx: &egui::Context,
    preference: ThemePreference,
    font_scale: f32,
    motion: MotionPreference,
) {
    let dark = match preference {
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        ThemePreference::System => {
            ctx.options(|opt| opt.theme_preference == egui::ThemePreference::Dark)
        }
    };

    for (theme, is_dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        let colors = palette(is_dark);
        let mut visuals = if is_dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.dark_mode = is_dark;
        visuals.override_text_color = Some(colors.text_primary);
        visuals.weak_text_color = Some(colors.text_muted);
        visuals.hyperlink_color = colors.accent;
        visuals.faint_bg_color = colors.surface_alt;
        visuals.extreme_bg_color = colors.background;
        visuals.text_edit_bg_color = Some(colors.background);
        visuals.code_bg_color = colors.background;
        visuals.warn_fg_color = colors.warning;
        visuals.error_fg_color = colors.danger;
        visuals.window_corner_radius = egui::CornerRadius::same(2);
        visuals.menu_corner_radius = egui::CornerRadius::same(2);
        visuals.window_fill = colors.surface;
        visuals.window_stroke = egui::Stroke::new(1.0, colors.border_strong);
        visuals.panel_fill = colors.background;
        visuals.selection.bg_fill = with_alpha(colors.accent, 78);
        visuals.selection.stroke = egui::Stroke::new(1.0, colors.active);
        visuals.widgets.noninteractive.bg_fill = colors.surface;
        visuals.widgets.noninteractive.weak_bg_fill = colors.surface;
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, colors.border);
        visuals.widgets.noninteractive.fg_stroke.color = colors.text_primary;
        visuals.widgets.inactive.bg_fill = colors.surface_alt;
        visuals.widgets.inactive.weak_bg_fill = colors.surface_alt;
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, colors.border);
        visuals.widgets.inactive.fg_stroke.color = colors.text_primary;
        visuals.widgets.hovered.bg_fill = with_alpha(colors.accent, 42);
        visuals.widgets.hovered.weak_bg_fill = with_alpha(colors.accent, 24);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, colors.active);
        visuals.widgets.hovered.fg_stroke.color = colors.text_primary;
        visuals.widgets.active.bg_fill = with_alpha(colors.accent, 62);
        visuals.widgets.active.weak_bg_fill = with_alpha(colors.accent, 36);
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, colors.active);
        visuals.widgets.active.fg_stroke.color = colors.text_primary;
        visuals.widgets.open.bg_fill = with_alpha(colors.accent, 48);
        visuals.widgets.open.weak_bg_fill = with_alpha(colors.accent, 30);
        visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, colors.active);
        visuals.widgets.open.fg_stroke.color = colors.text_primary;
        ctx.set_visuals_of(theme, visuals);
        ctx.style_mut_of(theme, |style| {
            style.animation_time = if motion == MotionPreference::Reduced {
                0.0
            } else {
                0.18
            };
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 6.0);
            style.spacing.window_margin = egui::Margin::same(12);
            style.text_styles.insert(
                egui::TextStyle::Heading,
                egui::FontId::new(24.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Body,
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Small,
                egui::FontId::new(11.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Monospace,
                egui::FontId::new(12.0, egui::FontFamily::Monospace),
            );
        });
    }

    ctx.options_mut(|opt| {
        opt.theme_preference = match preference {
            ThemePreference::System => egui::ThemePreference::System,
            ThemePreference::Light => egui::ThemePreference::Light,
            ThemePreference::Dark => egui::ThemePreference::Dark,
        };
    });
    ctx.set_zoom_factor(font_scale.clamp(
        crate::storage::MIN_FONT_SCALE,
        crate::storage::MAX_FONT_SCALE,
    ));
    let _ = dark;
}

/// Map a parsed ANSI SGR triple onto a [`Color32`]. Terminal output is not a
/// design token; this keeps the conversion next to the palettes.
pub fn ansi_rgb(r: u8, g: u8, b: u8) -> egui::Color32 {
    egui::Color32::from_rgb(r, g, b)
}

pub(crate) fn with_alpha(c: egui::Color32, a: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

#[cfg(test)]
mod tests {
    use super::{Tone, dark_palette, light_palette, motion_progress};
    use crate::storage::MotionPreference;

    fn contrast_ratio(a: egui::Color32, b: egui::Color32) -> f32 {
        fn lin(c: u8) -> f32 {
            let s = c as f32 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        fn lum(c: egui::Color32) -> f32 {
            0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
        }
        let (l1, l2) = (lum(a), lum(b));
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn palettes_meet_aa_for_primary_text() {
        let dark = dark_palette();
        let light = light_palette();
        assert!(contrast_ratio(dark.text_primary, dark.background) >= 4.5);
        assert!(contrast_ratio(light.text_primary, light.background) >= 4.5);
        assert!(contrast_ratio(dark.text_on_user, dark.bubble_user) >= 4.5);
        assert!(contrast_ratio(light.text_on_user, light.bubble_user) >= 4.5);
    }

    #[test]
    fn semantic_tones_are_distinct() {
        let palette = dark_palette();
        assert_ne!(super::tone_color(palette, Tone::Accent), palette.warning);
        assert_ne!(super::tone_color(palette, Tone::Success), palette.danger);
    }

    #[test]
    fn reduced_motion_is_static() {
        assert_eq!(motion_progress(0.3, MotionPreference::Reduced, 1.0), 1.0);
        assert!(motion_progress(0.3, MotionPreference::Full, 1.0) < 1.0);
    }

    #[test]
    fn light_and_dark_are_distinct() {
        let dark = dark_palette();
        let light = light_palette();
        assert_ne!(dark.bubble_assistant, light.bubble_assistant);
        assert_ne!(dark.surface, light.surface);
        assert_ne!(dark.border, light.border);
    }
}
