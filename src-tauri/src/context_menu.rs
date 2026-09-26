//! The widget's right-click menu. It is built fresh for every popup from the current state, with
//! the tray menu's item ids: Tauri hands every menu event to the tray's handler
//! (`tray::on_menu`), so both menus share one set of actions.

use std::sync::Arc;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Manager, State, Window, Wry};

use crate::settings::Settings;
use crate::state::{Shared, UiState};

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Item {
        id: &'static str,
        label: &'static str,
        /// Shortcut text drawn at the right edge.
        accelerator: Option<String>,
    },
    Check {
        id: &'static str,
        label: &'static str,
        checked: bool,
        accelerator: Option<String>,
    },
    Separator,
}

/// The shortcut to show beside an item: the saved one, unless it is empty or failed to register.
fn shortcut(accel: &str, error: Option<&String>) -> Option<String> {
    let accel = accel.trim();
    (!accel.is_empty() && error.is_none()).then(|| accel.to_owned())
}

/// The menu for the current state, labelled and checked like the tray menu.
pub fn entries(ui: &UiState, settings: &Settings) -> Vec<Entry> {
    use Entry::{Check, Item, Separator};
    vec![
        Item {
            id: "hide",
            label: "Hide widget",
            accelerator: shortcut(&settings.toggle_hotkey, ui.toggle_hotkey_error.as_ref()),
        },
        Item {
            id: "compact",
            label: crate::tray::compact_label(ui.view),
            accelerator: None,
        },
        Check {
            id: "pin",
            label: "Pin on top",
            checked: ui.pinned,
            accelerator: None,
        },
        Check {
            id: "click_through",
            label: "Click-through",
            checked: ui.click_through,
            accelerator: shortcut(&settings.hotkey, ui.hotkey_error.as_ref()),
        },
        Item {
            id: "history",
            label: "History…",
            accelerator: None,
        },
        Item {
            id: "settings",
            label: "Settings…",
            accelerator: None,
        },
        Separator,
        Item {
            id: "quit",
            label: "Quit",
            accelerator: None,
        },
    ]
}

/// Builds the native menu. A shortcut Windows can't draw leaves its item without the text rather
/// than failing the menu.
fn build(app: &AppHandle, entries: &[Entry]) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    for entry in entries {
        match entry {
            Entry::Item { id, label, accelerator } => {
                let item = |accel: Option<&str>| MenuItem::with_id(app, *id, *label, true, accel);
                if menu.append(&item(accelerator.as_deref())?).is_err() {
                    menu.append(&item(None)?)?;
                }
            }
            Entry::Check {
                id,
                label,
                checked,
                accelerator,
            } => {
                let item = |accel: Option<&str>| CheckMenuItem::with_id(app, *id, *label, true, *checked, accel);
                if menu.append(&item(accelerator.as_deref())?).is_err() {
                    menu.append(&item(None)?)?;
                }
            }
            Entry::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(menu)
}

/// Pops the menu up at the cursor. Async like Tauri's own popup command: the menu's modal loop
/// then runs from the event loop, not inside the webview's IPC callback.
#[tauri::command]
pub async fn show_context_menu(window: Window, shared: State<'_, Arc<Shared>>) -> Result<(), String> {
    let ui = shared.ui().clone();
    let settings = shared.settings().clone();
    let menu = build(window.app_handle(), &entries(&ui, &settings)).map_err(|e| e.to_string())?;
    window.popup_menu(&menu).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ViewMode;
    use crate::state::HiddenReason;

    fn ui(view: ViewMode, pinned: bool, click_through: bool) -> UiState {
        UiState {
            view,
            pinned,
            click_through,
            hotkey_error: None,
            toggle_hotkey_error: None,
            dock_expanded: false,
            hidden_reason: HiddenReason::None,
            update: None,
        }
    }

    fn find<'a>(entries: &'a [Entry], wanted: &str) -> &'a Entry {
        entries
            .iter()
            .find(|e| matches!(e, Entry::Item { id, .. } | Entry::Check { id, .. } if *id == wanted))
            .unwrap_or_else(|| panic!("no {wanted}"))
    }

    #[test]
    fn items_in_order_with_the_tray_ids() {
        let entries = entries(&ui(ViewMode::Card, true, false), &Settings::default());
        let ids: Vec<&str> = entries
            .iter()
            .map(|e| match e {
                Entry::Item { id, .. } | Entry::Check { id, .. } => *id,
                Entry::Separator => "-",
            })
            .collect();
        assert_eq!(ids, ["hide", "compact", "pin", "click_through", "history", "settings", "-", "quit"]);
    }

    #[test]
    fn labels_and_checks_follow_the_state() {
        let s = Settings::default();
        let card = entries(&ui(ViewMode::Card, true, false), &s);
        assert!(matches!(find(&card, "compact"), Entry::Item { label: "Compact", .. }));
        assert!(matches!(find(&card, "pin"), Entry::Check { checked: true, .. }));
        assert!(matches!(find(&card, "click_through"), Entry::Check { checked: false, .. }));
        let pill = entries(&ui(ViewMode::Pill, false, true), &s);
        assert!(matches!(find(&pill, "compact"), Entry::Item { label: "Expanded", .. }));
        assert!(matches!(find(&pill, "pin"), Entry::Check { checked: false, .. }));
        assert!(matches!(find(&pill, "click_through"), Entry::Check { checked: true, .. }));
        // From a panel, "Compact" switches to the pill, as in the tray.
        let settings_view = entries(&ui(ViewMode::Settings, true, false), &s);
        assert!(matches!(find(&settings_view, "compact"), Entry::Item { label: "Compact", .. }));
    }

    #[test]
    fn shortcuts_are_shown_only_when_they_work() {
        let s = Settings {
            hotkey: " Ctrl+Alt+K ".into(),
            ..Settings::default()
        };
        let ok = entries(&ui(ViewMode::Card, true, false), &s);
        assert!(
            matches!(find(&ok, "click_through"), Entry::Check { accelerator: Some(a), .. } if a == "Ctrl+Alt+K")
        );
        assert!(matches!(find(&ok, "hide"), Entry::Item { accelerator: Some(a), .. } if a == "Ctrl+Alt+H"));
        assert!(matches!(find(&ok, "pin"), Entry::Check { accelerator: None, .. }));

        let mut failed = ui(ViewMode::Card, true, false);
        failed.hotkey_error = Some("taken".into());
        failed.toggle_hotkey_error = Some("taken".into());
        let broken = entries(&failed, &s);
        assert!(matches!(find(&broken, "click_through"), Entry::Check { accelerator: None, .. }));
        assert!(matches!(find(&broken, "hide"), Entry::Item { accelerator: None, .. }));

        let none = Settings {
            toggle_hotkey: String::new(),
            ..Settings::default()
        };
        assert!(matches!(
            find(&entries(&ui(ViewMode::Pill, true, false), &none), "hide"),
            Entry::Item { accelerator: None, .. }
        ));
    }
}
