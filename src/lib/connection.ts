// The card's "status line was changed" banner (the Rust connection watchdog sets
// `ui.connection_lost`): when it shows, and its Reconnect action.

import type { UiState } from "./types";

/** Shown while the watchdog reports the loss, it is switched on, and the widget is interactive. */
export function connectionBannerVisible(ui: UiState, watchdog: boolean): boolean {
  return ui.connection_lost && watchdog && !ui.click_through;
}

export type ReconnectState = { state: "idle" } | { state: "busy" } | { state: "error"; message: string };

interface ReconnectApi {
  connectClaudeCode(dryRun: boolean): Promise<{ selftest_ok?: boolean | null } | unknown>;
  dismissConnectionWarning(): Promise<void>;
}

export const SELFTEST_FAILED =
  "Reconnected, but your status line printed something different through the widget. Check it in Settings → Claude Code.";

/** Runs the normal Connect (no preview). Once connected, the dismiss only clears the banner (the
 *  watchdog records nothing while the status line is ours), in case its own re-check is late. A
 *  failed self-test is reported like Settings does, not as a silent success. */
export async function reconnect(api: ReconnectApi): Promise<ReconnectState> {
  let selftestFailed = false;
  try {
    const result = await api.connectClaudeCode(false);
    selftestFailed = (result as { selftest_ok?: boolean | null } | null)?.selftest_ok === false;
  } catch (e) {
    return { state: "error", message: e instanceof Error ? e.message : String(e) };
  }
  if (selftestFailed) return { state: "error", message: SELFTEST_FAILED };
  try {
    await api.dismissConnectionWarning();
  } catch {
    // The watchdog's next check clears it.
  }
  return { state: "idle" };
}
