// Update-checker helpers for the UI: the "Update available" banner over the card / pill, the list
// of missed versions it opens, and the status line in Settings → System. The check itself runs in
// Rust (`check_updates_now`). Notify-only: "Update" opens the latest release page, where the user
// downloads the installer; the app never downloads or installs anything by itself.

import { version as packageVersion } from "../../package.json";
import type { DockEdge, UiState, UpdateInfo } from "./types";

/** This build's version (package.json; CI keeps it equal to Cargo.toml and tauri.conf.json). */
export const APP_VERSION: string = packageVersion;

/** Every release page of the repository starts with this (Rust enforces it too). */
export const RELEASES_PREFIX = "https://github.com/MarawanEldeib/mikyas/releases/";
export const MAX_NOTES = 5;
export const MAX_NOTE_CHARS = 80;

/** Whether the banner shows: updates are known and not put off with "Later", the card or pill is
 *  visible (not docked away) and interactive. On the card it would cover the footer's health
 *  warnings (`warnings`), which matter more; Settings → System still lists the updates. */
export function bannerVisible(ui: UiState, dock: DockEdge, warnings = false): boolean {
  return (
    ui.update !== null &&
    ui.update.count > 0 &&
    !ui.update.dismissed &&
    (ui.view === "pill" || (ui.view === "card" && !warnings)) &&
    (dock === "off" || ui.dock_expanded) &&
    !ui.click_through
  );
}

/** The banner's words: "Update available: v1.2.0" or "3 updates available" (shorter on the pill). */
export function bannerText(update: UpdateInfo, short = false): string {
  if (update.count > 1) return short ? `${update.count} updates` : `${update.count} updates available`;
  return short ? `v${update.latest} available` : `Update available: v${update.latest}`;
}

/** The latest release page ("Update" opens it), when it is one of this repository's. */
export function latestUrl(update: UpdateInfo | null): string | null {
  const url = update?.releases[0]?.url;
  return url && url.startsWith(RELEASES_PREFIX) ? url : null;
}

export interface UpdateRow {
  version: string;
  latest: boolean;
  /** Plain text, shown with text interpolation only (never as HTML). */
  notes: string[];
}

/** One row per missed version, newest first, with at most five short notes each (Rust already
 *  cleans them; this keeps the list tidy whatever arrives). */
export function updateRows(update: UpdateInfo | null): UpdateRow[] {
  return (update?.releases ?? []).map((r, i) => ({
    version: r.version,
    latest: i === 0,
    notes: r.notes
      .map((n) => n.trim())
      .filter((n) => n.length > 0)
      .slice(0, MAX_NOTES)
      .map((n) =>
        [...n].length > MAX_NOTE_CHARS
          ? `${[...n]
              .slice(0, MAX_NOTE_CHARS - 1)
              .join("")
              .trimEnd()}…`
          : n,
      ),
  }));
}

/** Progress of an explicit "Check now". */
export type CheckState = { state: "idle" } | { state: "checking" } | { state: "done" } | { state: "error"; message: string };

export interface UpdateStatus {
  tone: "ok" | "info" | "warn";
  text: string;
}

/** The status line under the version: nothing while a check runs (the button says so), then a
 *  failed check, known updates (from this or the daily check), or "Up to date". */
export function updateStatus(check: CheckState, update: UpdateInfo | null): UpdateStatus | null {
  if (check.state === "checking") return null;
  if (check.state === "error") return { tone: "warn", text: check.message };
  if (update && update.count > 0) {
    const text = update.count > 1 ? `${update.count} updates available (newest ${update.latest})` : `Version ${update.latest} available`;
    return { tone: "info", text };
  }
  if (check.state === "done") return { tone: "ok", text: "Up to date" };
  return null;
}
