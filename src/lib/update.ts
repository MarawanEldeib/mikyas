// Update-checker helpers for the UI: the "Update available" banner over the card / pill and the
// status line in Settings → System. The check itself runs in Rust (`check_updates_now`).

import { version as packageVersion } from "../../package.json";
import type { DockEdge, UiState, UpdateInfo } from "./types";

/** This build's version (package.json; CI keeps it equal to Cargo.toml and tauri.conf.json). */
export const APP_VERSION: string = packageVersion;

/** Whether the banner shows: an update is known, the card or pill is visible (not docked away)
 *  and interactive, and this version was not dismissed. */
export function bannerVisible(ui: UiState, dock: DockEdge, dismissed: string | null): boolean {
  return (
    ui.update !== null &&
    (ui.view === "card" || ui.view === "pill") &&
    (dock === "off" || ui.dock_expanded) &&
    !ui.click_through &&
    ui.update.version !== dismissed
  );
}

const DISMISS_KEY = "cuw.update.dismissed";

type KeyValue = Pick<Storage, "getItem" | "setItem">;

/** localStorage, or null where it is unavailable (it is a per-viewer convenience only). */
function localStore(): KeyValue | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** The version whose banner was dismissed, remembered across restarts when storage works. */
export function loadDismissed(store: KeyValue | null = localStore()): string | null {
  try {
    return store?.getItem(DISMISS_KEY) ?? null;
  } catch {
    return null;
  }
}

export function saveDismissed(version: string, store: KeyValue | null = localStore()): void {
  try {
    store?.setItem(DISMISS_KEY, version);
  } catch {
    // Not remembered; the banner comes back after a restart.
  }
}

/** Progress of an explicit "Check now". */
export type CheckState =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "done" }
  | { state: "error"; message: string };

export interface UpdateStatus {
  tone: "ok" | "info" | "warn";
  text: string;
  /** Release page to offer with a "View" link. */
  url: string | null;
}

/** The status line under the version: nothing while a check runs (the button says so), then a
 *  failed check, a known update (from this or the daily check), or "Up to date". */
export function updateStatus(check: CheckState, update: UpdateInfo | null): UpdateStatus | null {
  if (check.state === "checking") return null;
  if (check.state === "error") return { tone: "warn", text: check.message, url: null };
  if (update) return { tone: "info", text: `Version ${update.version} available`, url: update.url };
  if (check.state === "done") return { tone: "ok", text: "Up to date", url: null };
  return null;
}
