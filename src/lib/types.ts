// UI contract. Mirrors the serde output of crates/core/src/engine/types.rs and the app's
// settings/commands. Keep in sync with Rust (Option<T> serialises as `null`).

export type Ms = number;

export type WindowKind = "five_hour" | "seven_day" | (string & {});
export type Source = "cli" | "desktop";
export type Confidence = "high" | "medium" | "low";

export type ResetInfo =
  { type: "exact"; at_ms: Ms } | { type: "estimated"; at_ms: Ms; plus_minus_ms: Ms; confidence: Confidence } | { type: "unknown" };

export type Phase = "active" | "reset_awaiting_data";

export interface Burn {
  slope_pct_per_h: number;
  t100_ms: Ms | null;
  /** May exceed 100 (clamp for display). */
  pct_at_reset: number | null;
  hits_limit_before_reset: boolean;
}

/** `pct: null` marks a gap; break the line there. */
export interface SparkPoint {
  t_ms: Ms;
  pct: number | null;
}

export interface WindowView {
  kind: WindowKind;
  pct: number;
  reset: ResetInfo;
  source: Source;
  observed_at_ms: Ms;
  stale: boolean;
  limit_reached: boolean;
  phase: Phase;
  burn: Burn | null;
  /** 96 points: last 24 h for five_hour, last 7 d for seven_day. */
  spark: SparkPoint[];
  /** Claude worked after this reading, so the real % is higher (shown as "38% ▲"). */
  worked_since: boolean;
}

export type Entrypoint = "cli" | "desktop" | "cowork" | "unknown";
export type CtxBasis = "statusline" | "identity" | "desktop_model" | "override" | "learned" | "heuristic" | "default";

export interface SessionView {
  /** Opaque stable key for lists (a hash, never the session id). */
  key: string;
  model_id: string | null;
  display_name: string | null;
  ctx_pct: number | null;
  ctx_tokens: number | null;
  ctx_size: number;
  ctx_basis: CtxBasis;
  ctx_is_estimate: boolean;
  entrypoint: Entrypoint;
  last_active_ms: Ms;
  project: string | null;
  concurrent: number;
}

export type DesktopHealth =
  | { state: "not_found" }
  | {
      state: "ok";
      last_sample_ms: Ms | null;
      /** Set when the file's format is newer than this build knows and was read best-effort. */
      newer_version?: number | null;
    }
  | { state: "schema_changed"; version: number }
  | { state: "unreadable" };

export interface SourceHealth {
  desktop: DesktopHealth;
  cli_last_capture_ms: Ms | null;
  transcripts_last_activity_ms: Ms | null;
}

export type Warning = { type: "account_mismatch" } | { type: "no_plan_limits" };

/** Emitted by Rust as the `snapshot` event and returned by `get_snapshot`. */
export interface Snapshot {
  generated_ms: Ms;
  /** five_hour first, then seven_day, then others. */
  windows: WindowView[];
  session: SessionView | null;
  /** Every session active in the last 12 h, newest first, max 8; includes `session`. */
  sessions: SessionView[];
  health: SourceHealth;
  warnings: Warning[];
}

/** One local calendar day of one window in the History view. */
export interface HistoryDay {
  /** Local midnight. */
  day_start_ms: Ms;
  /** Highest % seen that day. */
  peak_pct: number;
  /** Sum of increases that day (share of the limit consumed; resets don't subtract). */
  consumed_pct: number;
  /** Rows recorded that day; 0 means no data, unlike a day at 0%. */
  samples: number;
}

export interface HistoryWindow {
  kind: WindowKind;
  /** Hourly buckets (max % per hour) over [from_ms, to_ms]; null = gap. */
  points: SparkPoint[];
  /** Detected reset times (exact or inferred from drops), ascending. */
  resets_ms: Ms[];
  /** Oldest first. */
  days: HistoryDay[];
}

/** Returned by `get_history`. */
export interface HistoryData {
  from_ms: Ms;
  to_ms: Ms;
  /** five_hour first, then seven_day, then others. */
  windows: HistoryWindow[];
  /** The most days a request can return (all the history keeps); longer ranges are hidden. */
  max_days: number;
}

// ---------------------------------------------------------------------------------------------
// App shell contract (src-tauri)

/** Window sizes (logical px): pill 240×72, card 320×232, settings 320×440, sessions 320×300,
 *  history 360×380 — all multiplied by `ui_scale`. Only pill/card are persisted. */
export type ViewMode = "pill" | "card" | "settings" | "sessions" | "history";
export type EffectName = "auto" | "mica" | "acrylic" | "blur" | "none";
/** Chrome accent. "auto" = neutral chrome; usage colours always follow the thresholds. */
export type Accent = "auto" | "blue" | "violet" | "teal" | "green" | "amber" | "rose";
export type GaugeStyle = "ring" | "bar";
export type DockEdge = "off" | "left" | "right" | "top";
/** What the widget's × does: hide to the tray, or quit the app. */
export type CloseAction = "hide" | "quit";
/** Which % the tray icon shows; "worst" = the higher of 5-hour and weekly, "off" = coloured dot. */
export type TrayNumber = "worst" | "five_hour" | "seven_day" | "off";

export interface CardRows {
  sparklines: boolean;
  burn: boolean;
  session: boolean;
  sources: boolean;
}

/** Persisted in %LOCALAPPDATA%\Mikyas\settings.json. */
export interface Settings {
  schema_version: number;
  view: ViewMode;
  pinned: boolean;
  /** Normal window opacity, 0.3..1. */
  opacity: number;
  /** Opacity while click-through ("ghost") is on, 0.15..1. */
  ghost_opacity: number;
  effect: EffectName;
  /** Ascending, e.g. [80, 95]. */
  thresholds: number[];
  notify_reset: boolean;
  /** Accelerator string, e.g. "Ctrl+Alt+U". */
  hotkey: string;
  /** Minutes after which data is shown as stale. */
  stale_min: number;
  /** Context-window size overrides by base model id, e.g. {"claude-opus-5-5": 1000000}. */
  ctx_overrides: Record<string, number>;
  show_project: boolean;
  start_with_windows: boolean;
  /** Toast when the active session's context crosses a threshold (default true). */
  ctx_alerts: boolean;
  /** Ascending context-% thresholds (default [80, 90]). */
  ctx_thresholds: number[];
  /** Global show/hide shortcut, "" = none (default "Ctrl+Alt+H"). */
  toggle_hotkey: string;
  /** Hide while a fullscreen video or app is in front (default true). */
  auto_hide_fullscreen: boolean;
  /** "Updates: Off / Notify me": opt-in daily GitHub Releases check — the app's only network
   *  call; it never downloads or installs anything (default false). */
  check_updates: boolean;
  accent: Accent;
  gauge_style: GaugeStyle;
  /** 0.85..1.3, scales the UI and the window (default 1). */
  ui_scale: number;
  card_rows: CardRows;
  dock: DockEdge;
  /** Default "hide". */
  close_action: CloseAction;
  /** Toast when the current pace reaches a limit before it resets (default true). */
  pace_alerts: boolean;
  /** Toast shortly before a capped limit reopens (default true). */
  reset_heads_up: boolean;
  /** One summary toast when the weekly limit resets (default true). */
  weekly_recap: boolean;
  /** Toast when a long Claude turn ends (default true). */
  finished_alerts: boolean;
  /** Minimum turn length for that toast, 1..60 (default 3). */
  finished_min_minutes: number;
  /** Warn when Claude Code's status line stops running the capture (default true). */
  connection_watchdog: boolean;
  /** Remember the position per monitor setup (default true). */
  per_display_position: boolean;
  /** Default "worst". */
  tray_number: TrayNumber;
  /** Internal: the first-hide toast was shown. No UI control; update_settings ignores it. */
  hide_hint_shown: boolean;
}

export type HiddenReason = "none" | "user" | "fullscreen";

/** One release newer than this build. */
export interface ReleaseInfo {
  version: string;
  /** Always one of this repository's https://github.com/…/releases/ pages. */
  url: string;
  /** Up to 5 short plain-text bullets (≤ 80 chars) from the release notes. Shown as text only. */
  notes: string[];
}

/** Every published release newer than this build (notify-only: nothing is ever downloaded). */
export interface UpdateInfo {
  /** Newest version (releases[0].version). */
  latest: string;
  /** Number of newer versions (releases.length). */
  count: number;
  /** Newest first. */
  releases: ReleaseInfo[];
  /** "Later" was chosen for `latest`: no banner until a newer version appears. */
  dismissed: boolean;
}

/** Emitted by Rust as the `ui-state` event and returned by `get_ui_state`. */
export interface UiState {
  view: ViewMode;
  pinned: boolean;
  click_through: boolean;
  /** Set when the global hotkey could not be registered. */
  hotkey_error: string | null;
  /** Set when the show/hide hotkey could not be registered. */
  toggle_hotkey_error: string | null;
  /** Docked widget is slid out (only meaningful when settings.dock != "off"). */
  dock_expanded: boolean;
  hidden_reason: HiddenReason;
  /** Newer releases, when the update checker found some. */
  update: UpdateInfo | null;
  /** Connect had wrapped the status line and it no longer does (Reconnect / Dismiss banner). */
  connection_lost: boolean;
}

export type ShellKind = "bash" | "cmd" | "pwsh" | "legacy_power_shell";
export type WrapMode = "pipe" | "pipe_grouped" | "argv" | "default";

export type ConnectionStatus =
  | { state: "not_configured" }
  | { state: "foreign"; command: string | null }
  | { state: "connected"; mode: WrapMode; original: string | null }
  | { state: "error"; message: string };

/** Result of `connect_claude_code` with `dryRun: true` (and, without it, of the real connect). */
export interface ConnectPreview {
  before: string | null;
  after: string;
  shell: ShellKind;
  /** Human-readable notes, e.g. project-level statusLine overrides found. */
  warnings: string[];
  /** Only for a real connect: did the self-test show identical statusline output? */
  selftest_ok: boolean | null;
}

/**
 * Tauri commands (invoke names, camelCase args):
 *  get_snapshot() -> Snapshot
 *  get_settings() -> Settings
 *  update_settings({ patch: Partial<Settings> }) -> Settings
 *  get_ui_state() -> UiState
 *  set_view({ view: ViewMode }) -> void
 *  set_pinned({ pinned: boolean }) -> void
 *  toggle_click_through() -> void
 *  connection_status() -> ConnectionStatus
 *  connect_claude_code({ dryRun: boolean }) -> ConnectPreview
 *  disconnect_claude_code() -> ConnectionStatus
 *  open_data_folder() -> void
 *  open_third_party_notices() -> void                    (the bundled THIRD_PARTY_NOTICES.md; no network)
 *  quit_app() -> void
 *  hide_widget() -> void                                 (the ×: hides like the tray, hint once)
 *  show_context_menu({ x?, y? }) -> void                 (native right-click menu at the cursor, or at x/y
 *                                                          logical px when opened from the keyboard)
 *  get_history({ days: number }) -> HistoryData          (days clamped to 1..max_days)
 *  set_dock_expanded({ expanded: boolean, force?: boolean }) -> void
 *                                   (pointer enter/leave while docked; force: the card's "–")
 *  check_updates_now() -> UpdateInfo | null              (explicit click; network)
 *  open_url({ url: string }) -> void                     (GitHub release pages only)
 *  dismiss_update({ version: string }) -> void           ("Later": hide until a newer version)
 *  dismiss_connection_warning() -> void                  (the connection-lost banner's Dismiss)
 * Events: "snapshot" (Snapshot), "ui-state" (UiState).
 */
export type CommandName =
  | "get_snapshot"
  | "get_settings"
  | "update_settings"
  | "get_ui_state"
  | "set_view"
  | "set_pinned"
  | "toggle_click_through"
  | "connection_status"
  | "connect_claude_code"
  | "disconnect_claude_code"
  | "open_data_folder"
  | "open_third_party_notices"
  | "quit_app"
  | "hide_widget"
  | "show_context_menu"
  | "get_history"
  | "set_dock_expanded"
  | "check_updates_now"
  | "open_url"
  | "dismiss_update"
  | "dismiss_connection_warning";
