// No console window alongside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod diff;
mod document;
mod git;
mod images;
mod markdown;
mod settings;
mod terminal;
mod theme;
mod themes;
mod tree;
mod ui;
mod update;
mod worker;

use eframe::egui;
use std::path::PathBuf;

fn main() -> eframe::Result {
    // `feather [path]` opens that folder; with no argument the last repository is reopened.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Feather")
        .with_app_id("feather")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([640.0, 400.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport: ui::titlebar::configure(viewport),
        renderer: eframe::Renderer::Glow,
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "Feather",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))),
    )
}
