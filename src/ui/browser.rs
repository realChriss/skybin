use super::{RED, human, icons};
use crate::app::{App, Dialog, Sort};
use crate::backend::Entry;
use eframe::egui::{self, Align, Key, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use std::collections::HashSet;

impl App {
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(bucket) = self.bucket.clone() else {
            return;
        };
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.prefix.is_empty(), egui::Button::new(icons::BACK))
                .on_hover_text("Up (Backspace)")
                .clicked()
            {
                self.go_up();
            }
            if ui
                .button(icons::REFRESH)
                .on_hover_text("Refresh (F5)")
                .clicked()
            {
                self.refresh();
            }
            ui.separator();
            let mut nav = None;
            if ui
                .add(egui::Button::new(RichText::new(&bucket).strong()).frame(false))
                .clicked()
            {
                nav = Some(String::new());
            }
            let mut acc = String::new();
            for part in self.prefix.split_terminator('/') {
                acc = format!("{acc}{part}/");
                ui.label(RichText::new(icons::CRUMB).weak());
                if ui.add(egui::Button::new(part).frame(false)).clicked() {
                    nav = Some(acc.clone());
                }
            }
            if let Some(p) = nav {
                self.navigate(self.bucket.clone(), p);
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.spacing_mut().button_padding = egui::vec2(7.0, 3.0);
            let n = self.selected.len();
            let btn = |ui: &mut egui::Ui, on: bool, icon: RichText, tip: &str| {
                ui.add_enabled(on, egui::Button::new(icon))
                    .on_hover_text(tip)
                    .on_disabled_hover_text(tip)
                    .clicked()
            };
            if btn(
                ui,
                true,
                icons::UPLOAD.into(),
                "Upload files (or drag & drop)",
            ) && let Some(files) = rfd::FileDialog::new().pick_files()
            {
                self.upload(files);
            }
            if btn(ui, true, icons::UPLOAD_FOLDER.into(), "Upload folder")
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.upload(vec![dir]);
            }
            if btn(ui, true, icons::NEW_FOLDER.into(), "New folder") {
                self.open_dialog(Dialog::NewFolder(String::new()));
            }
            ui.separator();
            if btn(ui, n > 0, icons::DOWNLOAD.into(), "Download") {
                self.download();
            }
            if btn(ui, n == 1, icons::EDIT.into(), "Rename / move (F2)") {
                self.ask_rename();
            }
            if btn(
                ui,
                n > 0,
                RichText::new(icons::TRASH).color(RED),
                "Delete (Del)",
            ) {
                self.ask_delete();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(format!("{}  Filter", icons::SEARCH))
                        .desired_width(180.0),
                );
            });
        });
        ui.add_space(4.0);
    }

    pub(crate) fn table(&mut self, ui: &mut egui::Ui) {
        let f = self.filter.to_lowercase();
        let view: Vec<usize> = (0..self.entries.len())
            .filter(|&i| f.is_empty() || self.entries[i].name.to_lowercase().contains(&f))
            .collect();

        if view.is_empty() {
            ui.centered_and_justified(|ui| {
                if self.loading {
                    ui.spinner();
                } else if f.is_empty() {
                    ui.label(RichText::new("Empty folder — drop files here to upload").weak());
                } else {
                    ui.label(RichText::new("No matches").weak());
                }
            });
            return;
        }

        let mut click: Option<(usize, egui::Modifiers)> = None;
        let mut open = None;
        let mut menu_action: Option<(&str, Entry)> = None;
        let mut sort_by = None;
        let has_public = self.public_url("").is_some();
        // Modifiers as they were at the click itself, not at the end of the frame.
        let mods = ui.input(|i| {
            let at_click = i.events.iter().rev().find_map(|e| match e {
                egui::Event::PointerButton {
                    pressed: false,
                    modifiers,
                    ..
                } => Some(*modifiers),
                _ => None,
            });
            at_click.unwrap_or(i.modifiers)
        });

        TableBuilder::new(ui)
            .striped(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(120.0).clip(true))
            .column(Column::exact(90.0))
            .column(Column::exact(140.0))
            .header(24.0, |mut h| {
                for (col, label) in [
                    (Sort::Name, "Name"),
                    (Sort::Size, "Size"),
                    (Sort::Modified, "Modified"),
                ] {
                    let arrow = match self.sort {
                        (c, true) if c == col => format!(" {}", icons::SORT_ASC),
                        (c, false) if c == col => format!(" {}", icons::SORT_DESC),
                        _ => String::new(),
                    };
                    // The cell's own response: a button inside would lose the click to the clickable header row.
                    h.col(|ui| {
                        let text = RichText::new(format!("{label}{arrow}")).strong();
                        let r = ui.add(egui::Label::new(text).sense(Sense::click()));
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            sort_by = Some(col);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, view.len(), |mut row| {
                    let vi = row.index();
                    let e = &self.entries[view[vi]];
                    row.set_selected(self.selected.contains(&e.key));
                    row.col(|ui| {
                        ui.label(if e.is_dir {
                            format!("{}  {}", icons::FOLDER, e.name)
                        } else {
                            format!("{}  {}", icons::FILE, e.name)
                        });
                    });
                    row.col(|ui| {
                        if !e.is_dir {
                            ui.label(RichText::new(human(e.size)).weak());
                        }
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(
                                e.modified
                                    .get(..16)
                                    .unwrap_or(&e.modified)
                                    .replace('T', " "),
                            )
                            .weak(),
                        );
                    });
                    let r = row.response();
                    if r.double_clicked() {
                        open = Some(e.clone());
                    } else if r.clicked() {
                        click = Some((vi, mods));
                    } else if r.secondary_clicked() && !self.selected.contains(&e.key) {
                        click = Some((vi, egui::Modifiers::NONE));
                    }
                    r.context_menu(|ui| {
                        let first = if e.is_dir {
                            (icons::FOLDER_OPEN, "Open")
                        } else {
                            (icons::INFO, "Properties")
                        };
                        for ((icon, label), act) in [
                            (first, "open"),
                            ((icons::DOWNLOAD, "Download"), "download"),
                            ((icons::EDIT, "Rename / move"), "rename"),
                            ((icons::COPY, "Copy key"), "copy"),
                            ((icons::LINK, "Copy URL (7 days)"), "url"),
                            ((icons::GLOBE, "Copy public URL"), "public"),
                            ((icons::TRASH, "Delete"), "delete"),
                        ] {
                            if (act == "url" || act == "public") && e.is_dir
                                || act == "public" && !has_public
                            {
                                continue;
                            }
                            if ui.button(format!("{icon}  {label}")).clicked() {
                                menu_action = Some((act, e.clone()));
                            }
                        }
                    });
                });
            });

        if let Some(col) = sort_by {
            self.sort = (
                col,
                if self.sort.0 == col {
                    !self.sort.1
                } else {
                    true
                },
            );
            self.sort_entries();
        }
        if let Some((vi, m)) = click {
            let key = self.entries[view[vi]].key.clone();
            let anchor = self
                .anchor
                .as_ref()
                .and_then(|a| view.iter().position(|&i| &self.entries[i].key == a));
            if m.command {
                if !self.selected.remove(&key) {
                    self.selected.insert(key.clone());
                }
                self.anchor = Some(key);
            } else if m.shift
                && let Some(a) = anchor
            {
                self.selected = (a.min(vi)..=a.max(vi))
                    .map(|i| self.entries[view[i]].key.clone())
                    .collect();
            } else {
                self.selected = HashSet::from([key.clone()]);
                self.anchor = Some(key);
            }
        }
        if let Some(e) = open {
            self.open(&e);
        }
        match menu_action {
            Some(("open", e)) => self.open(&e),
            Some(("copy", e)) => ui.ctx().copy_text(e.key),
            Some(("url", e)) => self.copy_url(&e.key),
            Some(("public", e)) => {
                if let Some(url) = self.public_url(&e.key) {
                    ui.ctx().copy_text(url);
                    self.status = Ok("Copied public link".into());
                }
            }
            Some(("download", _)) => self.download(),
            Some(("rename", _)) => self.ask_rename(),
            Some(("delete", _)) => self.ask_delete(),
            _ => {}
        }
        if self.dialog.is_none()
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|i| i.modifiers.command && i.key_pressed(Key::A))
        {
            self.selected = view.iter().map(|&i| self.entries[i].key.clone()).collect();
        }
    }
}
