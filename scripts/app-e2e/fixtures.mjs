// Synthetic fixtures for the app end-to-end test (scripts/app-e2e.mjs). Everything is generated
// at run time with the current time, so the transcript is always recent and nothing here is a
// data file that could be mistaken for (or replaced by) real usage data. Session ids follow the
// fixture rule (8 identical leading hex chars); paths use C:\Users\tester.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export const SESSION_ID = "aaaaaaaa-0000-4000-8000-000000000001";
export const MODEL_ID = "claude-opus-5-5";
export const MODEL_NAME = "Opus 5.5";
export const CTX_SIZE = 200_000;
/** Shown by the widget as "42" (5-hour) and "17" (weekly). */
export const FIVE_HOUR_PCT = 42;
export const SEVEN_DAY_PCT = 17;

/**
 * Settings that keep the test quiet and self-contained: no global hotkeys (the user's own widget
 * may be registered for them), no toasts, no update check.
 */
export const SETTINGS = {
  view: "card",
  pinned: false,
  hotkey: "",
  toggle_hotkey: "",
  thresholds: [],
  notify_reset: false,
  ctx_alerts: false,
  pace_alerts: false,
  reset_heads_up: false,
  weekly_recap: false,
  finished_alerts: false,
  connection_watchdog: false,
  check_updates: false,
  auto_hide_fullscreen: false,
  per_display_position: false,
  hide_hint_shown: true,
};

/**
 * Fills `root` with a widget data dir and a Claude Code config dir.
 * Returns `{ dataDir, claudeDir, webviewDir }`.
 */
export function writeFixtures(root, now = Date.now()) {
  const dataDir = join(root, "data");
  const claudeDir = join(root, "claude");
  const webviewDir = join(root, "webview");
  const captureDir = join(dataDir, "capture");
  const projectDir = join(claudeDir, "projects", "C--Users-tester-proj");
  for (const dir of [captureDir, projectDir, webviewDir]) mkdirSync(dir, { recursive: true });

  writeFileSync(join(dataDir, "settings.json"), JSON.stringify(SETTINGS, null, 2));

  const transcript = join(projectDir, `${SESSION_ID}.jsonl`);
  const line = {
    type: "assistant",
    isSidechain: false,
    sessionId: SESSION_ID,
    entrypoint: "cli",
    cwd: "C:\\Users\\tester\\proj",
    timestamp: new Date(now - 30_000).toISOString(),
    message: {
      model: MODEL_ID,
      usage: { input_tokens: 10, cache_creation_input_tokens: 0, cache_read_input_tokens: 49_990 },
    },
  };
  writeFileSync(transcript, JSON.stringify(line) + "\n");

  // The newest observation wins, so the capture is stamped "now".
  const resetSecs = (hours) => Math.floor((now + hours * 3_600_000) / 1000);
  const capture = {
    v: 1,
    session_id: SESSION_ID,
    written_at_ms: now,
    changed_at_ms: now,
    fingerprint: 1,
    transcript_path: transcript,
    model: { id: MODEL_ID, display_name: MODEL_NAME },
    context: { used_percentage: 25, context_window_size: CTX_SIZE },
    rate_limits: {
      five_hour: { used_percentage: FIVE_HOUR_PCT, resets_at: resetSecs(3) },
      seven_day: { used_percentage: SEVEN_DAY_PCT, resets_at: resetSecs(96) },
    },
  };
  writeFileSync(join(captureDir, `${SESSION_ID}.json`), JSON.stringify(capture));

  return { dataDir, claudeDir, webviewDir };
}
