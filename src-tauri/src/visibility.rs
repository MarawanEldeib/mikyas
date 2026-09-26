//! Showing and hiding the widget, and why it is hidden (`UiState.hidden_reason`): by the user
//! (tray Show/Hide, the show/hide hotkey) or automatically while a fullscreen app has the focus.
//!
//! The user always wins: auto-hide never brings back a widget the user hid, and a widget the user
//! brought back during a fullscreen app stays until that app loses the focus. The widget is shown
//! without being activated, so it never takes the keyboard focus from the app in front.

use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::state::{HiddenReason, Shared};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Tray Show/Hide (menu item or left click) or the show/hide hotkey.
    UserToggle,
    /// An explicit request to see the widget (tray Settings/Compact, starting the app again).
    UserShow,
    /// A fullscreen app took the foreground.
    FullscreenStarted,
    /// Still fullscreen: re-hides the widget if something else showed it meanwhile.
    FullscreenOngoing,
    /// The fullscreen app lost the foreground.
    FullscreenEnded,
    /// Auto-hide was switched off.
    AutoHideOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Hide,
    Show,
}

/// The next hidden reason and window action for `event`; `visible` is whether the window is shown.
pub fn transition(reason: HiddenReason, visible: bool, event: Event) -> (HiddenReason, Action) {
    use HiddenReason as R;
    match event {
        Event::UserToggle if visible => (R::User, Action::Hide),
        Event::UserToggle | Event::UserShow => (R::None, Action::Show),
        Event::FullscreenStarted => match reason {
            R::None => (R::Fullscreen, Action::Hide),
            other => (other, Action::Nothing),
        },
        Event::FullscreenOngoing => match reason {
            R::Fullscreen if visible => (R::Fullscreen, Action::Hide),
            other => (other, Action::Nothing),
        },
        Event::FullscreenEnded | Event::AutoHideOff => match reason {
            R::Fullscreen => (R::None, Action::Show),
            other => (other, Action::Nothing),
        },
    }
}

/// Applies `event` to the window and `UiState.hidden_reason`, then emits `ui-state` if anything
/// changed. Call on the main thread (see [`dispatch`]).
pub fn apply(app: &AppHandle, shared: &Shared, event: Event) {
    let Some(window) = crate::window::get(app) else { return };
    let visible = window.is_visible().unwrap_or(false);
    // The UI lock is released before touching the window.
    let (changed, action) = {
        let mut ui = shared.ui();
        let (next, action) = transition(ui.hidden_reason, visible, event);
        let changed = next != ui.hidden_reason;
        ui.hidden_reason = next;
        (changed, action)
    };
    match action {
        Action::Nothing => {}
        Action::Hide => {
            let _ = window.hide();
        }
        Action::Show => {
            crate::platform::show_without_activating(&window);
            // Also keeps the window layer's own visibility state in sync (a no-op show now) and
            // brings the widget back on-screen if its monitor went away while it was hidden.
            crate::window::show(app);
        }
    }
    if changed || action != Action::Nothing {
        crate::window::emit_ui(app, shared);
    }
}

/// Queues [`apply`] on the main thread (for callers on other threads).
pub fn dispatch(app: &AppHandle, event: Event) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let shared = handle.state::<Arc<Shared>>().inner().clone();
        apply(&handle, &shared, event);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use HiddenReason as R;

    #[test]
    fn user_toggle_follows_the_window() {
        assert_eq!(transition(R::None, true, Event::UserToggle), (R::User, Action::Hide));
        assert_eq!(transition(R::User, false, Event::UserToggle), (R::None, Action::Show));
        // Hidden by auto-hide: the user can still bring it back over the fullscreen app.
        assert_eq!(transition(R::Fullscreen, false, Event::UserToggle), (R::None, Action::Show));
        assert_eq!(transition(R::User, false, Event::UserShow), (R::None, Action::Show));
        assert_eq!(transition(R::Fullscreen, false, Event::UserShow), (R::None, Action::Show));
    }

    #[test]
    fn auto_hide_never_overrides_the_user() {
        assert_eq!(transition(R::User, false, Event::FullscreenStarted), (R::User, Action::Nothing));
        assert_eq!(transition(R::User, false, Event::FullscreenOngoing), (R::User, Action::Nothing));
        assert_eq!(transition(R::User, false, Event::FullscreenEnded), (R::User, Action::Nothing));
        assert_eq!(transition(R::User, false, Event::AutoHideOff), (R::User, Action::Nothing));
    }

    #[test]
    fn fullscreen_hides_and_restores() {
        assert_eq!(transition(R::None, true, Event::FullscreenStarted), (R::Fullscreen, Action::Hide));
        // Not shown yet (page still loading): remembered, so the page-load show gets undone.
        assert_eq!(transition(R::None, false, Event::FullscreenStarted), (R::Fullscreen, Action::Hide));
        assert_eq!(transition(R::Fullscreen, true, Event::FullscreenOngoing), (R::Fullscreen, Action::Hide));
        assert_eq!(transition(R::Fullscreen, false, Event::FullscreenOngoing), (R::Fullscreen, Action::Nothing));
        assert_eq!(transition(R::Fullscreen, false, Event::FullscreenEnded), (R::None, Action::Show));
        assert_eq!(transition(R::Fullscreen, false, Event::AutoHideOff), (R::None, Action::Show));
    }

    #[test]
    fn a_widget_the_user_brought_back_stays_during_fullscreen() {
        // Shown by the user while the game is still in front…
        let (reason, _) = transition(R::Fullscreen, false, Event::UserToggle);
        // …so the ongoing fullscreen does not hide it again, and its end does not touch it.
        assert_eq!(transition(reason, true, Event::FullscreenOngoing), (R::None, Action::Nothing));
        assert_eq!(transition(reason, true, Event::FullscreenEnded), (R::None, Action::Nothing));
        assert_eq!(transition(reason, true, Event::AutoHideOff), (R::None, Action::Nothing));
    }
}
