//! Claude Usage Widget app shell: window, tray, hotkey, notifications and the data pipeline
//! around the token-free `cuw-core` engine.

mod cli;
mod commands;
mod connect;
mod dock;
mod fullscreen;
mod history_view;
mod hotkey;
mod notify;
mod pipeline;
mod platform;
mod settings;
mod state;
mod tray;
mod updates;
mod visibility;
mod watcher;
mod window;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use cuw_core::paths::Paths;
use cuw_core::time::now_ms;
use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_window_state::StateFlags;

use crate::pipeline::{Dirty, Engine};
use crate::state::Shared;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Some(code) = cli::handle_args() {
        std::process::exit(code);
    }

    let paths = Paths::detect();
    let settings = settings::load(&paths.settings_file());

    // The first snapshot is computed before the window exists, so the UI never starts empty.
    let mut engine = Engine::new(paths.clone());
    let first = engine.tick(now_ms(), &settings, &Dirty::all());
    let shared = Arc::new(Shared::new(paths, settings, first.snapshot.clone()));
    let startup_alerts = first.alerts;

    let app = tauri::Builder::default()
        // Must be registered first: a second launch just surfaces the running widget.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let shared = app.state::<Arc<Shared>>().inner().clone();
            visibility::apply(app, &shared, visibility::Event::UserShow);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(hotkey::handler)
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::POSITION)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::get_settings,
            commands::update_settings,
            commands::get_ui_state,
            commands::set_view,
            commands::set_pinned,
            commands::toggle_click_through,
            commands::connection_status,
            commands::connect_claude_code,
            commands::disconnect_claude_code,
            commands::open_data_folder,
            commands::quit_app,
            history_view::get_history,
            dock::set_dock_expanded,
            updates::check_updates_now,
            updates::open_url,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // Reflect the real autostart registration (it may have been changed outside the app).
            if let Ok(enabled) = handle.autolaunch().is_enabled() {
                shared.settings().start_with_windows = enabled;
            }
            let settings = shared.settings().clone();
            tray::create(&handle, &shared)?;
            window::create(&handle, &settings)?;
            hotkey::register_all(&handle, &shared);
            fullscreen::start(&handle, shared.clone())?;
            updates::start(&handle, shared.clone())?;

            let (tx, rx) = mpsc::channel();
            *state::lock(&shared.pipeline) = Some(tx);
            let thread_shared = shared.clone();
            let thread_handle = handle.clone();
            std::thread::Builder::new()
                .name("cuw-pipeline".into())
                .spawn(move || pipeline::run(thread_handle, thread_shared, engine, rx))?;
            for event in &startup_alerts {
                notify::show_alert(&handle, event);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Claude Usage Widget");

    app.run(|app, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            // Hiding the widget must never end the app; only Quit (or the OS) does.
            let quitting = app.state::<Arc<Shared>>().quitting.load(Ordering::SeqCst);
            if code.is_none() && !quitting {
                api.prevent_exit();
            }
        }
    });
}
