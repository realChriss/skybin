use super::{ACCENT, RED};
use crate::app::{App, Dialog};
use eframe::egui::{self, Color32, Id, Key, RichText};

impl App {
    pub(crate) fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialog else { return };
        let focus = std::mem::take(&mut self.focus_dialog);
        let (mut submit, mut cancel, mut delete_profile) = (false, false, false);

        let text = |ui: &mut egui::Ui, s: &mut String, hint: &str, autofocus: bool| {
            let r = ui.add(
                egui::TextEdit::singleline(s)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );
            if focus && autofocus {
                r.request_focus();
            }
            r
        };

        let resp = egui::Modal::new(Id::unique("dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            // (primary label, is destructive, input valid, can delete profile)
            let (primary, danger, valid, deletable) = match d {
                Dialog::Profile { idx, p } => {
                    ui.heading(if idx.is_some() {
                        "Edit connection"
                    } else {
                        "New connection"
                    });
                    ui.add_space(8.0);
                    egui::Grid::new("profile")
                        .num_columns(2)
                        .spacing([10.0, 8.0])
                        .show(ui, |ui| {
                            for (label, field, hint, auto) in [
                                ("Name", &mut p.name, "My S3", true),
                                (
                                    "Endpoint",
                                    &mut p.endpoint,
                                    "https://s3.us-east-1.amazonaws.com",
                                    false,
                                ),
                                ("Region", &mut p.region, "us-east-1 / auto", false),
                                ("Access key", &mut p.access_key, "", false),
                            ] {
                                let l = ui.label(label);
                                text(ui, field, hint, auto).labelled_by(l.id);
                                ui.end_row();
                            }
                            let l = ui.label("Secret key");
                            ui.add(
                                egui::TextEdit::singleline(&mut p.secret_key)
                                    .password(true)
                                    .desired_width(f32::INFINITY),
                            )
                            .labelled_by(l.id);
                            ui.end_row();
                            ui.label("");
                            ui.checkbox(&mut p.path_style, "Path-style URLs")
                                .on_hover_text("Needed for MinIO and most self-hosted S3");
                            ui.end_row();
                        });
                    ui.label(
                        RichText::new(
                            "Keys are encrypted for your user account when the system supports it.",
                        )
                        .small()
                        .weak(),
                    );
                    let valid = !p.name.trim().is_empty() && !p.endpoint.trim().is_empty();
                    ("Save", false, valid, idx.is_some())
                }
                Dialog::NewBucket(name) => {
                    ui.heading("Create bucket");
                    text(ui, name, "bucket-name", true);
                    ("Create", false, !name.trim().is_empty(), false)
                }
                Dialog::DeleteBucket { name, empty_first } => {
                    ui.heading("Delete bucket");
                    ui.label(format!("Delete bucket “{name}”? This cannot be undone."));
                    ui.checkbox(empty_first, "Delete all objects in it first");
                    ("Delete", true, true, false)
                }
                Dialog::PublicUrl { bucket, url } => {
                    ui.heading("Public URL");
                    ui.label(
                        RichText::new(format!(
                            "Domain or CDN that serves “{bucket}” publicly. Empty = none."
                        ))
                        .small()
                        .weak(),
                    );
                    text(ui, url, "cdn.example.com", true);
                    ("Save", false, true, false)
                }
                Dialog::NewFolder(name) => {
                    ui.heading("New folder");
                    text(ui, name, "folder name", true);
                    (
                        "Create",
                        false,
                        !name.trim().trim_matches('/').is_empty(),
                        false,
                    )
                }
                Dialog::Rename { from, to } => {
                    ui.heading("Rename / move");
                    ui.label(
                        RichText::new("Full key — change the path to move it.")
                            .small()
                            .weak(),
                    );
                    text(ui, to, "new/key", true);
                    let valid = !to.trim().is_empty()
                        && to.trim().trim_end_matches('/') != from.key.trim_end_matches('/');
                    ("Rename", false, valid, false)
                }
                Dialog::Delete(entries) => {
                    ui.heading("Delete");
                    ui.label(format!(
                        "Delete {} item(s)? This cannot be undone.",
                        entries.len()
                    ));
                    for e in entries.iter().take(8) {
                        ui.label(RichText::new(format!("• {}", e.name)).weak());
                    }
                    if entries.len() > 8 {
                        ui.label(RichText::new(format!("… and {} more", entries.len() - 8)).weak());
                    }
                    if entries.iter().any(|e| e.is_dir) {
                        ui.colored_label(RED, "Folders are deleted with everything inside them.");
                    }
                    ("Delete", true, true, false)
                }
            };
            ui.add_space(12.0);
            egui::Sides::new().show(
                ui,
                |ui| {
                    if deletable
                        && ui
                            .button(RichText::new("Delete connection").color(RED))
                            .clicked()
                    {
                        delete_profile = true;
                    }
                },
                |ui| {
                    let btn = egui::Button::new(RichText::new(primary).color(Color32::WHITE))
                        .fill(if danger { RED } else { ACCENT });
                    if ui.add_enabled(valid, btn).clicked()
                        || (valid && ui.input(|i| i.key_pressed(Key::Enter)))
                    {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                },
            );
        });

        if delete_profile {
            if let Some(Dialog::Profile { idx: Some(i), .. }) = self.dialog.take() {
                self.profiles.remove(i);
                self.select_profile(None);
            }
        } else if submit {
            let d = self.dialog.take().unwrap();
            self.submit(d);
        } else if cancel || resp.should_close() {
            self.dialog = None;
        }
    }
}
