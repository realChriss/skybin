#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod media;
mod secret;
#[cfg(test)]
mod tests;
mod ui;
mod update;

fn main() -> eframe::Result {
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Skybin")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([720.0, 420.0]);
    // on macOS the Dock shows the .app bundle's icon, which this would override
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_icon(
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap(),
    );
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "skybin",
        options,
        Box::new(|cc| {
            ui::theme::apply(&cc.egui_ctx);
            ui::theme::fonts(&cc.egui_ctx, true);
            let profiles = cc
                .storage
                .and_then(|s| eframe::get_value(s, "profiles"))
                .unwrap_or_default();
            let profile = cc.storage.and_then(|s| eframe::get_value(s, "profile"));
            Ok(Box::new(app::App::new(profiles, profile)))
        }),
    )
}
