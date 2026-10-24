use super::{RED, human};
use crate::app::{App, Preview, image_uri};
use crate::media::{self, Audio, Kind};
use crate::update;
use eframe::egui::{self, Align, Layout, RichText, vec2};
use std::time::Duration;

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn audio_controls(ui: &mut egui::Ui, audio: &Audio) {
    ui.horizontal(|ui| {
        if ui.button(if audio.playing() { "⏸" } else { "▶" }).clicked() {
            audio.toggle();
        }
        let pos = audio.pos();
        let total = audio.total.map_or_else(|| "-:--".to_string(), clock);
        ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(40.0);
        let mut t = pos.as_secs_f32();
        let end = audio.total.map_or(t, |d| d.as_secs_f32()).max(0.1);
        if ui
            .add(egui::Slider::new(&mut t, 0.0..=end).show_value(false))
            .changed()
        {
            audio.seek(Duration::from_secs_f32(t));
        }
        ui.label(
            RichText::new(format!("{} / {total}", clock(pos)))
                .monospace()
                .small(),
        );
    });
    if audio.playing() {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
}

impl App {
    pub(crate) fn update_bar(&mut self, ui: &mut egui::Ui) {
        let mut install = false;
        ui.horizontal(|ui| {
            if let Some(tag) = &self.new_version {
                let current = update::VERSION.unwrap_or_default();
                ui.label(format!("Skybin {tag} is available (you have {current})."));
                install = ui.button("Update").clicked();
            } else if let Some(line) = &self.update_line {
                ui.label(line);
            }
        });
        if install {
            self.install_update();
        }
    }

    pub(crate) fn props_panel(&mut self, ui: &mut egui::Ui) {
        let Some((key, _)) = &self.props else { return };
        let key = key.clone();
        let mut close = false;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.strong("Properties");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = ui.button("✖").clicked()
            });
        });
        ui.separator();
        ui.add(egui::Label::new(RichText::new(&key).monospace()).wrap());
        if ui.small_button("📋 Copy key").clicked() {
            ui.ctx().copy_text(key.clone());
        }
        self.preview_ui(ui, &key);
        ui.add_space(8.0);
        let Some((_, r)) = &self.props else { return };
        egui::ScrollArea::vertical().show(ui, |ui| match r {
            Ok(rows) if rows.is_empty() => {
                ui.spinner();
            }
            Ok(rows) => {
                egui::Grid::new("props")
                    .num_columns(2)
                    .striped(true)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(RichText::new(k).weak());
                            ui.add(egui::Label::new(v).wrap());
                            ui.end_row();
                        }
                    });
            }
            Err(e) => {
                ui.colored_label(RED, e);
            }
        });
        if close {
            self.props = None;
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui, key: &str) {
        let Some((_, preview)) = &self.preview else {
            return;
        };
        ui.add_space(8.0);
        match preview {
            Preview::Loading => {
                ui.spinner();
            }
            Preview::Image(bytes) => {
                let size = vec2(ui.available_width(), 320.0);
                ui.add(egui::Image::from_bytes(image_uri(key), bytes.clone()).max_size(size));
            }
            Preview::Audio(audio) => audio_controls(ui, audio),
            Preview::Note(note) => {
                ui.label(RichText::new(note).weak());
            }
        }
        let open = if media::kind(key) == Some(Kind::Video) {
            ui.button("▶ Play in browser").clicked()
        } else {
            ui.small_button("↗ Open in browser").clicked()
        };
        if open {
            self.open_in_browser(key);
        }
    }

    pub(crate) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.busy > 0 {
                ui.spinner();
            }
            match &self.status {
                Ok(s) => ui.label(RichText::new(s).weak()),
                Err(e) => ui.colored_label(RED, format!("⚠ {e}")),
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.bucket.is_some() {
                    let sel = self.selection();
                    let size: u64 = sel.iter().map(|e| e.size).sum();
                    let txt = if sel.is_empty() {
                        format!("{} items", self.entries.len())
                    } else {
                        format!(
                            "{} of {} selected · {}",
                            sel.len(),
                            self.entries.len(),
                            human(size)
                        )
                    };
                    ui.label(RichText::new(txt).weak());
                }
            });
        });
    }
}
