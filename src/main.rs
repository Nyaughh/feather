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
mod store;
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

    let from_shell = options.from_shell || cli::from_terminal();
    let mut error = None;
    let initial = match &options.path {
        None => None,
        Some(path) => {
            let cwd = std::env::current_dir().unwrap_or_default();
            match cli::resolve(&cwd, path) {
                Ok(target) => Some(target),
                Err(e) if from_shell => fail(&e, 1),
                // Started from a shortcut there's no terminal to say so in:
                // the window says it instead.
                Err(e) => {
                    error = Some(format!("Can't open {e}"));
                    None
                }
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
        if cfg!(not(debug_assertions)) && from_shell {
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
        // A path that couldn't be opened shouldn't turn into the last
        // repository either: open empty, with the reason showing.
        new_window: options.window == WindowChoice::New || error.is_some(),
        target: initial,
        error,
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
        // A window opened beside others starts where the system puts it,
        // rather than exactly on top of the one whose place was saved.
        persist_window: !instance::others_running(),
        ..Default::default()
    };
    eframe::run_native(
        "Feather",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, startup)))),
    )
}
