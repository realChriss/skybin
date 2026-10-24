use super::{RED, human};
use crate::app::App;
use crate::update;
use eframe::egui::{self, Align, Layout, RichText};

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
        let Some((key, r)) = &self.props else { return };
        let mut close = false;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.strong("Properties");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = ui.button("✖").clicked()
            });
        });
        ui.separator();
        ui.add(egui::Label::new(RichText::new(key).monospace()).wrap());
        if ui.small_button("📋 Copy key").clicked() {
            ui.ctx().copy_text(key.clone());
        }
        ui.add_space(8.0);
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
