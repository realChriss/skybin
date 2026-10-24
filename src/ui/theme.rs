//! Dark theme tweaks on top of egui's defaults.

use super::ACCENT;
use eframe::egui::{self, Color32, Theme, vec2};

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(Theme::Dark);
    ctx.style_mut_of(Theme::Dark, |s| {
        use egui::{FontId, TextStyle};
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Heading, FontId::proportional(18.0));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(11.0));
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 4.0);
        s.interaction.selectable_labels = false;

        let v = &mut s.visuals;
        v.panel_fill = Color32::from_rgb(24, 25, 28);
        v.window_fill = Color32::from_rgb(30, 31, 35);
        v.extreme_bg_color = Color32::from_rgb(17, 18, 20);
        v.faint_bg_color = Color32::from_rgb(29, 30, 34);
        v.selection.bg_fill = ACCENT;
        v.widgets.noninteractive.fg_stroke.color = Color32::from_gray(210);
        v.weak_text_color = Some(Color32::from_gray(155));
        v.window_corner_radius = 8.into();
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = 5.into();
        }
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(40, 42, 47);
    });
}
