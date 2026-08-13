//! Semantic color tokens. No `Color32::from_rgb` literals belong outside this file.

use crate::storage::ThemePreference;
use eframe::egui;

/// Contrast-checked palettes for Chat, Coder, diffs and status.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bubble_user: egui::Color32,
    pub bubble_assistant: egui::Color32,
    pub text_primary: egui::Color32,
    pub text_on_user: egui::Color32,
    pub text_muted: egui::Color32,
    pub surface: egui::Color32,
    pub border: egui::Color32,
    pub danger: egui::Color32,
    pub warning: egui::Color32,
    pub success: egui::Color32,
    pub accent: egui::Color32,
    pub diff_add_fg: egui::Color32,
    pub diff_add_bg: egui::Color32,
    pub diff_del_fg: egui::Color32,
    pub diff_del_bg: egui::Color32,
    pub diff_eq_fg: egui::Color32,
    pub diff_eq_bg: egui::Color32,
}

pub fn dark_palette() -> Palette {
    Palette {
        bubble_user: egui::Color32::from_rgb(50, 90, 160),
        bubble_assistant: egui::Color32::from_rgb(48, 48, 52),
        text_primary: egui::Color32::from_rgb(236, 236, 240),
        text_on_user: egui::Color32::from_rgb(255, 255, 255),
        text_muted: egui::Color32::from_rgb(158, 158, 166),
        surface: egui::Color32::from_rgb(36, 36, 40),
        border: egui::Color32::from_rgb(70, 70, 76),
        danger: egui::Color32::from_rgb(220, 80, 80),
        warning: egui::Color32::from_rgb(220, 160, 80),
        success: egui::Color32::from_rgb(80, 180, 110),
        accent: egui::Color32::from_rgb(80, 140, 220),
        diff_add_fg: egui::Color32::from_rgb(180, 255, 180),
        diff_add_bg: egui::Color32::from_rgb(28, 60, 32),
        diff_del_fg: egui::Color32::from_rgb(255, 180, 180),
        diff_del_bg: egui::Color32::from_rgb(70, 32, 32),
        diff_eq_fg: egui::Color32::from_rgb(180, 180, 180),
        diff_eq_bg: egui::Color32::from_rgb(36, 36, 40),
    }
}

pub fn light_palette() -> Palette {
    Palette {
        bubble_user: egui::Color32::from_rgb(36, 92, 176),
        bubble_assistant: egui::Color32::from_rgb(232, 234, 238),
        text_primary: egui::Color32::from_rgb(28, 28, 32),
        text_on_user: egui::Color32::from_rgb(255, 255, 255),
        text_muted: egui::Color32::from_rgb(90, 90, 98),
        surface: egui::Color32::from_rgb(246, 246, 248),
        border: egui::Color32::from_rgb(198, 200, 206),
        danger: egui::Color32::from_rgb(180, 40, 40),
        warning: egui::Color32::from_rgb(160, 100, 20),
        success: egui::Color32::from_rgb(24, 128, 64),
        accent: egui::Color32::from_rgb(36, 92, 176),
        diff_add_fg: egui::Color32::from_rgb(16, 96, 32),
        diff_add_bg: egui::Color32::from_rgb(220, 242, 220),
        diff_del_fg: egui::Color32::from_rgb(160, 24, 24),
        diff_del_bg: egui::Color32::from_rgb(255, 228, 228),
        diff_eq_fg: egui::Color32::from_rgb(70, 70, 76),
        diff_eq_bg: egui::Color32::from_rgb(242, 242, 244),
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

/// Apply the stored preference (including Follow system) and font zoom.
pub fn apply(ctx: &egui::Context, preference: ThemePreference, font_scale: f32) {
    let theme = match preference {
        ThemePreference::System => egui::ThemePreference::System,
        ThemePreference::Light => egui::ThemePreference::Light,
        ThemePreference::Dark => egui::ThemePreference::Dark,
    };
    ctx.options_mut(|opt| {
        opt.theme_preference = theme;
    });
    ctx.set_zoom_factor(font_scale.clamp(
        crate::storage::MIN_FONT_SCALE,
        crate::storage::MAX_FONT_SCALE,
    ));
}

/// Map a parsed ANSI SGR triple onto a [`Color32`]. Terminal output is not a
/// design token; this keeps the conversion next to the palettes.
pub fn ansi_rgb(r: u8, g: u8, b: u8) -> egui::Color32 {
    egui::Color32::from_rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::{dark_palette, light_palette};

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
        assert!(contrast_ratio(dark.text_primary, dark.surface) >= 4.5);
        assert!(contrast_ratio(light.text_primary, light.surface) >= 4.5);
        assert!(contrast_ratio(dark.text_on_user, dark.bubble_user) >= 4.5);
        assert!(contrast_ratio(light.text_on_user, light.bubble_user) >= 4.5);
        assert!(contrast_ratio(light.text_primary, light.bubble_assistant) >= 4.5);
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
