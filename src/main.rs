// No console window alongside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cli;
mod diff;
mod document;
mod git;
mod graph;
mod images;
mod instance;
mod markdown;
mod settings;
mod shell_command;
mod terminal;
mod theme;
mod themes;
mod tree;
mod ui;
mod update;
mod worker;

use cli::{Request, Target, WindowChoice};
use eframe::egui;
use std::ffi::OsString;

fn main() -> eframe::Result {
    let options = match cli::parse(std::env::args_os().skip(1)) {
        Ok(Request::Help) => {
            cli::print(cli::USAGE, false);
            return Ok(());
        }
        Ok(Request::Version) => {
            cli::print(&format!("feather {}\n", env!("CARGO_PKG_VERSION")), false);
            return Ok(());
        }
        Ok(Request::Open(options)) => options,
        Err(e) => fail(&format!("{e}\nRun 'feather --help' to see the options."), 2),
    };

    let initial = match &options.path {
        None => None,
        Some(path) => {
            let cwd = std::env::current_dir().unwrap_or_default();
            match cli::resolve(&cwd, path) {
                Ok(target) => Some(target),
                // Started from a shortcut there's nowhere to say so: open anyway.
                Err(_) if !cli::from_terminal() => None,
                Err(e) => fail(&e, 1),
            }
        }
    };

    if !options.wait {
        if let Some(target) = &initial
            && instance::hand_off(target, options.window)
        {
            return Ok(());
        }
        // From a terminal, run the window in the background and give the
        // prompt back (debug builds stay put, to keep their logs in view).
        if cfg!(not(debug_assertions)) && cli::from_terminal() {
            let mut args: Vec<OsString> = vec!["--wait".into()];
            if options.window == WindowChoice::New {
                args.push("--new-window".into());
            }
            if let Some(target) = &initial {
                let (Target::Folder(path) | Target::File(path) | Target::NewFile(path)) = target;
                args.push("--".into());
                args.push(path.into());
            }
            match cli::spawn_detached(&args) {
                Ok(()) => return Ok(()),
                // Run the window here instead.
                Err(e) => cli::print(&format!("feather: {e}\n"), true),
            }
        }
    }
    run(app::Startup {
        new_window: options.window == WindowChoice::New,
        target: initial,
    })
}

fn fail(message: &str, code: i32) -> ! {
    cli::print(&format!("feather: {message}\n"), true);
    std::process::exit(code)
}

fn run(startup: app::Startup) -> eframe::Result {
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
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, startup)))),
    )
}
