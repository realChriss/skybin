use super::ACCENT;
use eframe::egui::{self, Color32, FontData, FontDefinitions, Theme, vec2};
use std::sync::Arc;

/// egui only bundles Latin, Greek, Cyrillic and common emoji, but keys can be in any script.
// ponytail: fixed paths per OS; ask fontconfig if a Linux distro keeps its fonts elsewhere
const SYSTEM_FONTS: &[&str] = if cfg!(windows) {
    &[
        "C:/Windows/Fonts/seguisym.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/malgun.ttf",
        "C:/Windows/Fonts/tahoma.ttf",
        "C:/Windows/Fonts/Nirmala.ttc",
    ]
} else if cfg!(target_os = "macos") {
    &[
        "/System/Library/Fonts/Apple Symbols.ttf",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ]
} else {
    &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    ]
};

/// Lucide goes first because egui's own icon font also uses the private use area. The system's
/// fonts go last, so they only draw what egui's lack.
pub fn fonts(ctx: &egui::Context, system: bool) {
    let mut fonts = FontDefinitions::default();
    let lucide = FontData::from_static(include_bytes!("../../assets/lucide.ttf"));
    fonts.font_data.insert("lucide".into(), Arc::new(lucide));
    for family in fonts.families.values_mut() {
        family.insert(0, "lucide".into());
    }
    for path in SYSTEM_FONTS.iter().filter(|_| system) {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let name = path.to_string();
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(data)));
        for family in fonts.families.values_mut() {
            family.push(name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

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
