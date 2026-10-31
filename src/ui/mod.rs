mod browser;
mod dialogs;
pub mod icons;
mod panels;
mod sidebar;
pub mod theme;

use crate::app::App;
use eframe::egui::{self, Color32, Id, Key, RichText, vec2};

/// No single red reads both as text on the dark panels and behind white text, so there are two.
pub const RED: Color32 = Color32::from_rgb(240, 110, 110);
pub const RED_FILL: Color32 = Color32::from_rgb(200, 60, 60);
pub const ACCENT: Color32 = Color32::from_rgb(45, 100, 180);
pub const FOLDER: Color32 = Color32::from_rgb(110, 160, 225);
const CONTENT: Color32 = Color32::from_rgb(27, 28, 32);
pub const ROW: f32 = 28.0;

pub fn ghost(text: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text).frame_when_inactive(false)
}

pub fn primary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(ACCENT)
}

pub fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

pub fn skeleton(ui: &mut egui::Ui, rows: usize, cols: &[f32]) {
    let fill = ui.visuals().widgets.inactive.weak_bg_fill;
    for i in 0..rows {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW), egui::Sense::hover());
        let mut right = rect.right();
        let mut spans = vec![];
        for &w in cols.iter().rev() {
            right -= w;
            spans.push((right, w));
        }
        spans.push((rect.left() + 4.0, right - rect.left() - 12.0));
        for (c, (x, w)) in spans.into_iter().enumerate() {
            let f = [0.7, 0.45, 0.6, 0.35, 0.55][(i + c) % 5];
            let bar = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - 5.0),
                vec2((w * f).max(0.0), 10.0),
            );
            ui.painter().rect_filled(bar, 3.0, fill);
        }
    }
}

pub fn empty_state<R>(
    ui: &mut egui::Ui,
    icon: RichText,
    title: &str,
    detail: &str,
    action: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        ui.label(icon.size(32.0));
        ui.add_space(4.0);
        ui.label(RichText::new(title).size(16.0));
        if !detail.is_empty() {
            ui.add(egui::Label::new(RichText::new(detail).weak()).wrap());
        }
        ui.add_space(8.0);
        action(ui)
    })
    .inner
}

pub fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut u) = (n as f64, 0);
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "profiles", &self.profiles);
        eframe::set_value(storage, "profile", &self.profile);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(dir) = &self.cache {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl App {
    /// Separate from `eframe::App::ui` so headless tests can drive it without a `Frame`.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.begin_frame(&ctx);
        self.shortcuts(&ctx);

        if self.new_version.is_some() || self.update_line.is_some() {
            egui::Panel::top("update")
                .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(vec2(10.0, 6.0)))
                .show(ui, |ui| self.update_bar(ui));
        }
        egui::Panel::bottom("status")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(vec2(10.0, 4.0)))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("side")
            .resizable(true)
            .default_size(230.0)
            .size_range(170.0..=400.0)
            .show(ui, |ui| self.sidebar(ui));
        if self.props.is_some() {
            egui::Panel::right("props")
                .resizable(true)
                .default_size(300.0)
                .show(ui, |ui| self.props_panel(ui));
        }
        egui::CentralPanel::default_margins()
            .frame(egui::Frame::central_panel(ui.style()).fill(CONTENT))
            .show(ui, |ui| {
                if self.bucket.is_none() {
                    let bucket = RichText::new(icons::BUCKET).weak();
                    if self.profiles.is_empty() {
                        let detail =
                            "Any S3-compatible storage works, like AWS, Cloudflare R2 or MinIO.";
                        if empty_state(
                            ui,
                            bucket,
                            "Add a connection to get started",
                            detail,
                            |ui| ui.add(primary("Add connection")).clicked(),
                        ) {
                            self.new_profile();
                        }
                    } else if let Some(e) = self.buckets_err.clone() {
                        let icon = RichText::new(icons::WARNING).color(RED);
                        if empty_state(ui, icon, "Couldn't load buckets", &e, |ui| {
                            ui.button("Retry").clicked()
                        }) {
                            self.refresh_buckets();
                        }
                    } else if self.profile.is_none() {
                        empty_state(ui, bucket, "Select a connection", "", |_| ());
                    } else {
                        empty_state(ui, bucket, "Select a bucket", "", |_| ());
                    }
                    return;
                }
                self.toolbar(ui);
                self.table(ui);
            });

        self.dialogs(&ctx);
        self.drag_and_drop(&ctx);
    }

    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.bucket.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |k| ctx.input(|i| i.key_pressed(k));
        if pressed(Key::Delete) {
            self.ask_delete();
        } else if pressed(Key::F2) {
            self.ask_rename();
        } else if pressed(Key::F5) {
            self.refresh();
        } else if pressed(Key::Backspace) && !self.prefix.is_empty() {
            self.go_up();
        } else if pressed(Key::Escape) {
            self.selected.clear();
        } else if pressed(Key::Enter)
            && let [e] = &self.selection()[..]
        {
            self.open(&e.clone());
        }
    }

    pub(crate) fn drag_and_drop(&mut self, ctx: &egui::Context) {
        let (hovering, dropped) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .collect::<Vec<_>>(),
            )
        });
        if self.bucket.is_none() {
            return;
        }
        if !dropped.is_empty() {
            self.upload(dropped);
        }
        if hovering {
            let rect = ctx.content_rect();
            let p = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::unique("drop"),
            ));
            p.rect_filled(rect, 0.0, Color32::from_black_alpha(190));
            p.rect_stroke(
                rect.shrink(12.0),
                8.0,
                egui::Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Inside,
            );
            let target = format!(
                "Drop to upload to {}/{}",
                self.bucket.as_deref().unwrap_or_default(),
                self.prefix
            );
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                target,
                egui::FontId::proportional(20.0),
                Color32::WHITE,
            );
        }
    }
}
