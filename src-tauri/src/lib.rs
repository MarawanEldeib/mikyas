//! Mikyas app shell: window, tray, hotkey, notifications and the data pipeline
//! around the token-free `mikyas-core` engine.

mod cli;
mod commands;
mod connect;
mod context_menu;
mod diag;
mod display_positions;
mod dock;
mod fullscreen;
mod history_view;
mod hotkey;
mod localtime;
mod migrate;
mod pipeline;
mod platform;
mod settings;
mod state;
mod toast;
mod tray;
mod tray_icon;
mod updates;
mod visibility;
mod watchdog;
mod watcher;
mod window;

/// The toast module's former name, kept until every caller says `crate::toast`.
use toast as notify;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use mikyas_core::paths::Paths;
use mikyas_core::time::now_ms;
use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_window_state::StateFlags;

use crate::pipeline::{Dirty, PipelineState};
use crate::state::Shared;

/// Process hardening that must run before anything loads a DLL: `main` calls it first.
///
/// Forwards to the platform layer's DLL search-order restriction once that exists; until then
/// it does nothing (merge note: call `platform::restrict_dll_search()` here).
pub fn restrict_dll_search() {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Some(code) = cli::handle_args() {
        std::process::exit(code);
    }

    let paths = Paths::detect();
    diag::init(paths.data_root());
    // Once, before anything reads the data folder: the move from a former name of the app
    // (SovaWatch or Claude Usage Widget), or the rest of one cut short on an earlier start. It
    // runs before the single-instance check (every step can run twice without harm; a second
    // launch then exits there, without a notification), and before `.build()` below sets up the
    // window-state plugin, which is when that plugin reads the window position the move copies.
    let migrated = migrate::run(&paths, &migrate::legacy_roots(), connect::find_sidecar().as_deref(), now_ms());
    let settings = settings::load(&paths.settings_file());
    // Reads only: a second launch exits in the single-instance plugin before anything is written.
    let mut pipeline_state = PipelineState::new(paths.clone());
    let preview = pipeline_state.preview(now_ms(), &settings);
    let shared = Arc::new(Shared::new(paths, settings, preview));

    let app = tauri::Builder::default()
        // Must be registered first: a second launch just surfaces the running widget (and exits
        // while the plugins are set up, before the `setup` below runs).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let shared = app.state::<Arc<Shared>>().inner().clone();
            visibility::apply(app, &shared, visibility::Event::UserShow);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(hotkey::handler).build())
        .plugin(tauri_plugin_window_state::Builder::default().with_state_flags(StateFlags::POSITION).build())
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
            commands::open_third_party_notices,
            commands::quit_app,
            commands::hide_widget,
            context_menu::show_context_menu,
            history_view::get_history,
            dock::set_dock_expanded,
            updates::check_updates_now,
            updates::open_url,
            updates::dismiss_update,
            watchdog::dismiss_connection_warning,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // "Start with Windows" was on in the old app: its Run entry has the old name.
            if let migrate::Outcome::Moved { from, autostart: true, .. } = &migrated {
                if let Err(e) = handle.autolaunch().enable() {
                    diag::log(&format!("move from {}: autostart: {e}", from.display_name));
                }
            }
            // Reflect the real autostart registration (it may have been changed outside the app).
            if let Ok(enabled) = handle.autolaunch().is_enabled() {
                shared.settings().start_with_windows = enabled;
            }
            let settings = shared.settings().clone();
            // The first full tick runs before the window exists, so the UI never starts empty.
            let first = pipeline_state.tick(now_ms(), &settings, &Dirty::all());
            *state::lock(&shared.snapshot) = first.snapshot.clone();
            tray::create(&handle, &shared)?;
            window::create(&handle, &settings)?;
            hotkey::register_all(&handle, &shared);
            fullscreen::start(&handle, shared.clone())?;
            updates::start(&handle, shared.clone())?;
            watchdog::start(&handle, shared.clone())?;
            display_positions::start(&handle, shared.clone())?;

            let (tx, rx) = mpsc::channel();
            *state::lock(&shared.pipeline) = Some(tx);
            let thread_shared = shared.clone();
            let thread_handle = handle.clone();
            std::thread::Builder::new()
                .name("mikyas-pipeline".into())
                .spawn(move || pipeline::run(thread_handle, thread_shared, pipeline_state, rx))?;
            for event in &first.alerts {
                notify::show_alert(&handle, event);
            }
            if let Some((title, body)) = migrate::notice(&migrated) {
                notify::show(&handle, &title, &body);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Mikyas");

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
