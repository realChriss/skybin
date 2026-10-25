use super::{RED, human};
use crate::app::{App, Preview, image_uri};
use crate::media::Playback;
use crate::update;
use eframe::egui::{self, Align, Layout, RichText, vec2};
use std::time::Duration;

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn controls(ui: &mut egui::Ui, media: &mut dyn Playback) {
    ui.horizontal(|ui| {
        if ui.button(if media.playing() { "⏸" } else { "▶" }).clicked() {
            media.toggle();
        }
        let total = media.total().map_or_else(|| "-:--".to_string(), clock);
        // Seeking on every frame of a held drag replays the same few milliseconds of sound,
        // so the drag target is kept aside and only applied on release.
        let id = ui.make_persistent_id("scrub");
        let scrub = ui.data(|d| d.get_temp::<f32>(id));
        let mut t = scrub.unwrap_or(media.pos().as_secs_f32());
        let end = media.total().map_or(t, |d| d.as_secs_f32()).max(0.1);
        // Right-to-left so the time takes its size first and the bar fills the rest;
        // a guessed bar width overflows the resizable panel and widens it every frame.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{} / {total}", clock(Duration::from_secs_f32(t))))
                    .monospace()
                    .small(),
            );
            ui.spacing_mut().slider_width = ui.available_width();
            let slider = ui.add(egui::Slider::new(&mut t, 0.0..=end).show_value(false));
            if slider.is_pointer_button_down_on() {
                ui.data_mut(|d| d.insert_temp(id, t));
            } else if scrub.is_some() || slider.changed() {
                ui.data_mut(|d| d.remove::<f32>(id));
                media.seek(Duration::from_secs_f32(t));
            }
        });
    });
    if media.playing() {
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
        let (mut close, mut open) = (false, false);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.strong("Properties");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = ui.button("✖").clicked();
                open = ui.button("↗").on_hover_text("Open in browser").clicked();
            });
        });
        if open {
            self.open_in_browser(&key);
        }
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
        let Some((_, preview)) = &mut self.preview else {
            return;
        };
        ui.add_space(8.0);
        let size = vec2(ui.available_width(), 320.0);
        match preview {
            Preview::Loading => {
                ui.spinner();
            }
            Preview::Image(bytes) => {
                ui.add(egui::Image::from_bytes(image_uri(key), bytes.clone()).max_size(size));
            }
            Preview::Audio(audio) => controls(ui, audio),
            Preview::Video(video) => {
                video.update();
                match video.texture() {
                    Some(t) => ui.add(egui::Image::new(t).max_size(size)),
                    None => ui.spinner(),
                };
                controls(ui, video);
            }
            Preview::Note(note) => {
                ui.label(RichText::new(note.as_str()).weak());
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::accesskit::Role;
    use egui::{Event, PointerButton};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// Always playing, so the position moves on every frame like real playback.
    #[derive(Default)]
    struct Playing {
        pos: Duration,
        seeks: Vec<Duration>,
    }

    impl Playback for Playing {
        fn playing(&self) -> bool {
            true
        }
        fn pos(&self) -> Duration {
            self.pos
        }
        fn total(&self) -> Option<Duration> {
            Some(Duration::from_secs(100))
        }
        fn toggle(&mut self) {}
        fn seek(&mut self, to: Duration) {
            self.seeks.push(to);
            self.pos = to;
        }
    }

    #[test]
    fn holding_the_seek_bar_seeks_once_on_release() {
        let mut h = Harness::builder()
            .with_size([320.0, 60.0])
            .with_accessibility_check(false)
            .build_ui_state(
                |ui, p: &mut Playing| {
                    p.pos += Duration::from_millis(16);
                    controls(ui, p);
                },
                Playing::default(),
            );
        h.step();
        // the whole row fits the panel instead of pushing it wider
        let time = h.get_by(|n| {
            n.role() == Role::Label && n.value().is_some_and(|v| v.ends_with("/ 1:40"))
        });
        assert!(time.rect().right() <= 320.0);
        let at = h.get_by_role(Role::Slider).rect().center();
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        h.event(Event::PointerMoved(at));
        h.event(button(true));
        for _ in 0..10 {
            h.step();
        }
        assert!(h.state().seeks.is_empty());
        h.event(button(false));
        h.step();
        h.step();
        let seeks = &h.state().seeks;
        assert_eq!(seeks.len(), 1);
        assert!((seeks[0].as_secs_f32() - 50.0).abs() < 10.0, "{seeks:?}");
    }
}
