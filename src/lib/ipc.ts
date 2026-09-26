// Typed bridge to the Rust side. Inside Tauri this wraps invoke/listen/startDragging from
// @tauri-apps/api; in a plain browser (dev server, screenshots) it lazily loads the mock
// backend so the mock never ships in the Tauri code path.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  CommandName,
  ConnectPreview,
  ConnectionStatus,
  HistoryData,
  Settings,
  Snapshot,
  UiState,
  UpdateInfo,
  ViewMode,
} from "./types";

export type Unlisten = () => void;
export type EventName = "snapshot" | "ui-state";

/** The minimal surface both the real and the mock backend implement. */
export interface Backend {
  invoke<T>(cmd: CommandName, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: EventName, cb: (payload: T) => void): Promise<Unlisten>;
  startDragging(): Promise<void>;
}

/** True inside the Tauri webview. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const tauriBackend: Backend = {
  invoke: (cmd, args) => tauriInvoke(cmd, args),
  listen: (event, cb) => tauriListen(event, (e) => cb(e.payload as never)),
  // getCurrentWindow() throws outside Tauri, so it is only resolved here.
  startDragging: () => getCurrentWindow().startDragging(),
};

let backendPromise: Promise<Backend> | null = null;

/** The active backend (Tauri, or the mock in a plain browser). */
export function backend(): Promise<Backend> {
  backendPromise ??= inTauri
    ? Promise.resolve(tauriBackend)
    : import("./mock").then((m) => m.createMockBackend(new URLSearchParams(window.location.search)));
  return backendPromise;
}

async function call<T>(cmd: CommandName, args?: Record<string, unknown>): Promise<T> {
  return (await backend()).invoke<T>(cmd, args);
}

/** Typed wrappers for every Tauri command in the UI contract. */
export const api = {
  getSnapshot: () => call<Snapshot>("get_snapshot"),
  getSettings: () => call<Settings>("get_settings"),
  updateSettings: (patch: Partial<Settings>) => call<Settings>("update_settings", { patch }),
  getUiState: () => call<UiState>("get_ui_state"),
  setView: (view: ViewMode) => call<void>("set_view", { view }),
  setPinned: (pinned: boolean) => call<void>("set_pinned", { pinned }),
  toggleClickThrough: () => call<void>("toggle_click_through"),
  connectionStatus: () => call<ConnectionStatus>("connection_status"),
  connectClaudeCode: (dryRun: boolean) => call<ConnectPreview>("connect_claude_code", { dryRun }),
  disconnectClaudeCode: () => call<ConnectionStatus>("disconnect_claude_code"),
  openDataFolder: () => call<void>("open_data_folder"),
  quitApp: () => call<void>("quit_app"),
  hideWidget: () => call<void>("hide_widget"),
  /** `at` (logical px from the window's top-left) for keyboard-opened menus; `null` = at the cursor. */
  showContextMenu: (at: { x: number; y: number } | null = null) =>
    call<void>("show_context_menu", at ? { x: at.x, y: at.y } : {}),
  getHistory: (days: number) => call<HistoryData>("get_history", { days }),
  /** `force`: the user asked (the card's "–"), so Rust skips its pointer-still-inside check. */
  setDockExpanded: (expanded: boolean, force = false) => call<void>("set_dock_expanded", { expanded, force }),
  checkUpdatesNow: () => call<UpdateInfo | null>("check_updates_now"),
  openUrl: (url: string) => call<void>("open_url", { url }),
  dismissConnectionWarning: () => call<void>("dismiss_connection_warning"),
};

/** Subscribes to `snapshot` events. */
export async function onSnapshot(cb: (s: Snapshot) => void): Promise<Unlisten> {
  return (await backend()).listen<Snapshot>("snapshot", cb);
}

/** Subscribes to `ui-state` events. */
export async function onUiState(cb: (u: UiState) => void): Promise<Unlisten> {
  return (await backend()).listen<UiState>("ui-state", cb);
}

/** Begins a native window drag (no-op in the mock). Errors are logged, never thrown. */
export function startDragging(): void {
  // Called synchronously from mousedown so the OS drag starts while the button is held.
  const p = inTauri ? tauriBackend.startDragging() : Promise.resolve();
  p.catch((e: unknown) => console.warn("startDragging failed", e));
}
