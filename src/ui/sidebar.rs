use crate::app::{App, Dialog};
use crate::backend::Profile;
use eframe::egui::{self, Align, Layout, RichText};

impl App {
    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let heading = ui.label(RichText::new("CONNECTION").small().weak());
        let name = self
            .cur_profile()
            .map(|p| p.name)
            .unwrap_or_else(|| "None".into());
        // Right-to-left so the buttons take their size first and the picker fills the rest;
        // sizing from `available_width()` in a resizable panel otherwise makes it grow every frame.
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("➕").on_hover_text("New connection").clicked() {
                    self.open_dialog(Dialog::Profile {
                        idx: None,
                        p: Profile {
                            region: "us-east-1".into(),
                            ..Default::default()
                        },
                    });
                }
                if ui.button("✏").on_hover_text("Edit connection").clicked()
                    && let Some(i) = self.profile
                {
                    self.open_dialog(Dialog::Profile {
                        idx: Some(i),
                        p: self.profiles[i].clone(),
                    });
                }
                let mut pick = None;
                let combo = egui::ComboBox::from_id_salt("profile")
                    .selected_text(name)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (i, p) in self.profiles.iter().enumerate() {
                            if ui
                                .selectable_label(self.profile == Some(i), &p.name)
                                .clicked()
                            {
                                pick = Some(i);
                            }
                        }
                    });
                combo.response.labelled_by(heading.id);
                if pick.is_some() && pick != self.profile {
                    self.select_profile(pick);
                }
            });
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("BUCKETS").small().weak());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let on = self.profile.is_some();
                if ui
                    .add_enabled(on, egui::Button::new("⟳").small())
                    .on_hover_text("Refresh buckets")
                    .clicked()
                {
                    self.refresh_buckets();
                }
                if ui
                    .add_enabled(on, egui::Button::new("➕").small())
                    .on_hover_text("Create bucket")
                    .clicked()
                {
                    self.open_dialog(Dialog::NewBucket(String::new()));
                }
            });
        });
        ui.separator();

        if self.profiles.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No connections yet").weak());
                if ui.button("Add connection").clicked() {
                    self.open_dialog(Dialog::Profile {
                        idx: None,
                        p: Profile {
                            region: "us-east-1".into(),
                            ..Default::default()
                        },
                    });
                }
            });
            return;
        }

        let mut clicked = None;
        let mut delete = None;
        let mut public = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    for b in &self.buckets {
                        let sel = self.bucket.as_ref() == Some(b);
                        let r = ui.selectable_label(sel, format!("🗄  {b}"));
                        if r.clicked() {
                            clicked = Some(b.clone());
                        }
                        r.context_menu(|ui| {
                            if ui.button("🌐  Public URL…").clicked() {
                                public = Some(b.clone());
                            }
                            if ui.button("🗑  Delete bucket…").clicked() {
                                delete = Some(b.clone());
                            }
                        });
                    }
                });
            });
        if let Some(b) = clicked {
            self.navigate(Some(b), String::new());
        }
        if let Some(bucket) = public {
            let url = self
                .cur_profile()
                .and_then(|p| p.public_urls.get(&bucket).cloned())
                .unwrap_or_default();
            self.open_dialog(Dialog::PublicUrl { bucket, url });
        }
        if let Some(name) = delete {
            self.open_dialog(Dialog::DeleteBucket {
                name,
                empty_first: false,
            });
        }
    }
}
