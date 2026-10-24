mod browser;
mod dialogs;
mod panels;
mod sidebar;
pub mod theme;

use crate::app::App;
use eframe::egui::{self, Color32, Id, Key, RichText, vec2};

pub const RED: Color32 = Color32::from_rgb(220, 80, 80);
pub const ACCENT: Color32 = Color32::from_rgb(45, 100, 180);

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
}

impl App {
    /// Separate from `eframe::App::ui` so headless tests can drive it without a `Frame`.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.begin_frame(&ctx);
        self.shortcuts(&ctx);

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
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if self.bucket.is_none() {
                ui.centered_and_justified(|ui| {
                    let msg = if self.profiles.is_empty() {
                        "Add a connection to get started"
                    } else {
                        "Select a bucket"
                    };
                    ui.label(RichText::new(msg).weak().size(16.0));
                });
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
