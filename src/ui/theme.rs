use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Stroke, Theme, vec2};
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
    for (name, data) in [
        ("lucide", &include_bytes!("../../assets/lucide.ttf")[..]),
        ("geist", include_bytes!("../../assets/geist.ttf")),
        ("geist-mono", include_bytes!("../../assets/geist-mono.ttf")),
    ] {
        let data = Arc::new(FontData::from_static(data));
        fonts.font_data.insert(name.into(), data);
    }
    for (family, text) in [
        (FontFamily::Proportional, "geist"),
        (FontFamily::Monospace, "geist-mono"),
    ] {
        let list = fonts.families.entry(family).or_default();
        list.insert(0, text.into());
        list.insert(0, "lucide".into());
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
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.window_margin = 16.into();
        s.interaction.selectable_labels = false;

        let v = &mut s.visuals;
        v.panel_fill = Color32::from_rgb(20, 21, 24);
        v.window_fill = Color32::from_rgb(32, 33, 38);
        v.window_stroke = Stroke::new(1.0, Color32::from_rgb(48, 50, 56));
        v.extreme_bg_color = Color32::from_rgb(18, 19, 22);
        v.faint_bg_color = Color32::from_rgb(31, 32, 37);
        v.selection.bg_fill = Color32::from_rgb(33, 53, 86);
        v.widgets.noninteractive.fg_stroke.color = Color32::from_gray(215);
        v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(38, 40, 45);
        v.weak_text_color = Some(Color32::from_gray(160));
        v.window_corner_radius = 10.into();
        v.menu_corner_radius = 8.into();
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = 5.into();
        }
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(40, 42, 47);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(46, 48, 54));
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 50, 56);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(64, 67, 74));
    });
}
