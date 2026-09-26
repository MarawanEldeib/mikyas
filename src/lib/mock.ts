// Browser-only mock backend: realistic, deterministic snapshots for every UI state, driven by
// URL params (?scenario=…&view=…&effect=…&ghost=1&conn=…&update=…&hidden=…). Loaded lazily by
// ipc.ts only when not running inside Tauri. All data here is synthetic.

import { ACCENTS } from "./color";
import { DAY, HOUR, MIN, SEC } from "./format";
import type { Backend, EventName, Unlisten } from "./ipc";
import { parseCardRows } from "./layout";
import { MOCK_HISTORY_DAYS, mockHistory } from "./mock-history";
import type {
  Burn,
  CommandName,
  HiddenReason,
  ConnectPreview,
  ConnectionStatus,
  DesktopHealth,
  EffectName,
  ResetInfo,
  SessionView,
  Settings,
  Snapshot,
  SparkPoint,
  UiState,
  UpdateInfo,
  ViewMode,
  Warning,
  WindowView,
} from "./types";

export const SCENARIOS = [
  "normal",
  "high",
  "limit",
  "stale",
  "desktop-only",
  "no-session",
  "reset",
  "estimated",
  "warnings",
  "onboarding",
] as const;
export type Scenario = (typeof SCENARIOS)[number];

const VIEWS: readonly ViewMode[] = ["pill", "card", "settings", "sessions", "history"];
const HIDDEN: readonly HiddenReason[] = ["none", "user", "fullscreen"];
/** ?update=1: already known at start; none: "Check now" finds nothing; error: it fails (like
 *  Tauri, with a string). By default "Check now" finds MOCK_UPDATE. */
const UPDATE_MODES = ["default", "1", "none", "error"] as const;
export const MOCK_UPDATE: UpdateInfo = {
  version: "0.2.0",
  url: "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v0.2.0",
};
const EFFECTS: readonly EffectName[] = ["auto", "mica", "acrylic", "blur", "none"];

/** Live-update period of the mock `snapshot` event. */
export const MOCK_TICK_MS = 5 * SEC;
export const SPARK_POINTS = 96;

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function hashSeed(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}

interface SparkSpec {
  now: number;
  /** Chart span (24 h or 7 d). */
  span: number;
  /** Window length (5 h or 7 d). */
  period: number;
  /** Anchor for window boundaries (the reset instant). */
  resetAt: number;
  /** Value the series must end on. */
  current: number;
  /** Index ranges [start, length) rendered as gaps. */
  gaps?: [number, number][];
  seed: number;
}

/** A plausible usage history: activity bursts during waking hours, dropping to 0 at resets. */
export function mockSpark(spec: SparkSpec): SparkPoint[] {
  const rand = mulberry32(spec.seed);
  const step = spec.span / SPARK_POINTS;
  const lastStart = Math.floor(spec.now / step) * step;
  const pts: { t_ms: number; win: number; v: number }[] = [];
  let cum = 0;
  let win = Number.NaN;
  let scale = 0;
  for (let i = 0; i < SPARK_POINTS; i++) {
    const t = lastStart - (SPARK_POINTS - 1 - i) * step;
    const w = Math.ceil((spec.resetAt - t) / spec.period);
    if (w !== win) {
      win = w;
      cum = 0;
      // Target peak for this window, spread over its buckets.
      scale = ((25 + rand() * 70) / (spec.period / step)) * 2.2;
    }
    const hour = new Date(t).getHours();
    const awake = hour >= 8 && hour <= 23;
    if (rand() < (awake ? 0.6 : 0.06)) cum += rand() * scale;
    pts.push({ t_ms: t, win: w, v: Math.min(100, cum) });
  }
  const gap = new Set<number>();
  for (const [start, len] of spec.gaps ?? []) {
    for (let i = start; i < start + len; i++) gap.add(i);
  }
  // Rescale the window of the latest reading so the series ends exactly on the live value.
  let last = pts.length - 1;
  while (last >= 0 && gap.has(last)) last--;
  if (last >= 0) {
    const curWin = pts[last].win;
    const cur = pts.slice(0, last + 1).filter((p) => p.win === curWin);
    const end = pts[last].v;
    cur.forEach((p, i) => {
      p.v = end > 0 ? (p.v / end) * spec.current : (spec.current * (i + 1)) / cur.length;
    });
  }
  return pts.map((p, i) => ({
    t_ms: p.t_ms,
    pct: gap.has(i) ? null : Math.round(Math.min(100, p.v) * 10) / 10,
  }));
}

interface WinSpec {
  kind: "five_hour" | "seven_day";
  pct: number;
  /** Reset offset from load time; null = unknown. */
  resetIn: number | null;
  estimate?: { pm: number; confidence: "high" | "medium" | "low" };
  source: "cli" | "desktop";
  observedAgo: number;
  stale?: boolean;
  awaiting?: boolean;
  /** %/h; drives the burn forecast and the live creep. */
  slope?: number;
  gaps?: [number, number][];
}

interface ScenarioSpec {
  windows: WinSpec[];
  session: (t0: number) => SessionView | null;
  /** Other recent sessions for the Sessions view (the header session is added to the list). */
  others?: (t0: number) => SessionView[];
  desktop: (t0: number) => DesktopHealth;
  cliAgo: number | null;
  transcriptsAgo: number | null;
  warnings: Warning[];
  connection: ConnectionStatus;
  hotkeyError?: string;
}

const opus = (over: Partial<SessionView> = {}) => (t0: number): SessionView => ({
  key: "mock-session-1",
  model_id: "claude-opus-5-5",
  display_name: "Opus 5.5",
  ctx_pct: 34.2,
  ctx_tokens: 342_000,
  ctx_size: 1_000_000,
  ctx_basis: "statusline",
  ctx_is_estimate: false,
  entrypoint: "cli",
  last_active_ms: t0 - 2 * MIN,
  project: "claude-usage-widget",
  concurrent: 1,
  ...over,
});

const desktopOk = (ago: number) => (t0: number): DesktopHealth => ({ state: "ok", last_sample_ms: t0 - ago });
const connected: ConnectionStatus = { state: "connected", mode: "pipe", original: null };
const notConfigured: ConnectionStatus = { state: "not_configured" };

const NORMAL: ScenarioSpec = {
  windows: [
    { kind: "five_hour", pct: 29, resetIn: 3 * HOUR + 12 * MIN + 30 * SEC, source: "cli", observedAgo: 2 * MIN, slope: 6.4, gaps: [[40, 7]] },
    { kind: "seven_day", pct: 59, resetIn: 2 * DAY + 4 * HOUR + 20 * MIN, source: "cli", observedAgo: 2 * MIN, slope: 0.55 },
  ],
  session: opus(),
  desktop: desktopOk(9 * MIN),
  cliAgo: 2 * MIN,
  transcriptsAgo: 1 * MIN,
  warnings: [],
  connection: connected,
};

const sonnet = (over: Partial<SessionView>) => (t0: number): SessionView =>
  opus({
    model_id: "claude-sonnet-5",
    display_name: "Sonnet 5",
    ctx_size: 200_000,
    ...over,
  })(t0);

const SPECS: Record<Scenario, ScenarioSpec> = {
  normal: {
    ...NORMAL,
    others: (t0) => [
      sonnet({ key: "mock-session-2", ctx_pct: 61, ctx_tokens: 122_000, ctx_basis: "desktop_model", ctx_is_estimate: true, entrypoint: "desktop", project: "demo-app", last_active_ms: t0 - 38 * MIN })(t0),
      opus({ key: "mock-session-3", ctx_pct: 12.4, ctx_tokens: 124_000, ctx_basis: "identity", ctx_is_estimate: true, entrypoint: "cowork", project: null, last_active_ms: t0 - 2 * HOUR - 10 * MIN })(t0),
      sonnet({ key: "mock-session-4", ctx_pct: 87, ctx_tokens: 870_000, ctx_size: 1_000_000, project: "api-gateway", last_active_ms: t0 - 6 * HOUR - 40 * MIN })(t0),
    ],
  },
  high: {
    ...NORMAL,
    windows: [
      { kind: "five_hour", pct: 83, resetIn: 2 * HOUR + 5 * MIN, source: "cli", observedAgo: 1 * MIN, slope: 12 },
      { kind: "seven_day", pct: 72, resetIn: 2 * DAY + 4 * HOUR, source: "cli", observedAgo: 1 * MIN, slope: 0.7 },
    ],
    session: opus({ ctx_pct: 78.4, ctx_tokens: 784_000, concurrent: 2, last_active_ms: 0 }),
    others: (t0) => [
      sonnet({ key: "mock-session-2", ctx_pct: 91, ctx_tokens: 182_000, concurrent: 2, project: "demo-app", last_active_ms: t0 - 4 * MIN })(t0),
      opus({ key: "mock-session-3", ctx_pct: 45, ctx_tokens: 450_000, ctx_basis: "identity", ctx_is_estimate: true, concurrent: 2, entrypoint: "desktop", project: "infra-scripts", last_active_ms: t0 - 3 * HOUR })(t0),
    ],
    cliAgo: 1 * MIN,
  },
  limit: {
    ...NORMAL,
    windows: [
      { kind: "five_hour", pct: 100, resetIn: 47 * MIN + 10 * SEC, source: "cli", observedAgo: 3 * MIN },
      { kind: "seven_day", pct: 91, resetIn: DAY + 6 * HOUR, source: "cli", observedAgo: 3 * MIN, slope: 1.1 },
    ],
    session: opus({ ctx_pct: 12, ctx_tokens: 120_000 }),
  },
  stale: {
    ...NORMAL,
    windows: [
      { kind: "five_hour", pct: 41, resetIn: HOUR + 20 * MIN, source: "cli", observedAgo: 3 * HOUR, stale: true, gaps: [[84, 12]] },
      { kind: "seven_day", pct: 63, resetIn: 3 * DAY, source: "cli", observedAgo: 3 * HOUR, stale: true },
    ],
    session: opus({ last_active_ms: 0, ctx_pct: 51 }),
    desktop: desktopOk(2 * HOUR),
    cliAgo: 3 * HOUR,
    transcriptsAgo: 3 * HOUR,
  },
  "desktop-only": {
    windows: [
      { kind: "five_hour", pct: 34, resetIn: 2 * HOUR + 40 * MIN, estimate: { pm: 20 * MIN, confidence: "medium" }, source: "desktop", observedAgo: 4 * MIN, slope: 5 },
      { kind: "seven_day", pct: 61, resetIn: 4 * DAY + 2 * HOUR, estimate: { pm: 3 * HOUR, confidence: "low" }, source: "desktop", observedAgo: 4 * MIN },
    ],
    session: opus({
      model_id: "claude-sonnet-5",
      display_name: "Sonnet 5",
      ctx_pct: 21,
      ctx_tokens: 42_000,
      ctx_size: 200_000,
      ctx_basis: "desktop_model",
      ctx_is_estimate: true,
      entrypoint: "desktop",
      project: "demo-app",
    }),
    desktop: desktopOk(4 * MIN),
    cliAgo: null,
    transcriptsAgo: 6 * MIN,
    warnings: [],
    connection: notConfigured,
  },
  "no-session": { ...NORMAL, session: () => null },
  reset: {
    ...NORMAL,
    windows: [
      { kind: "five_hour", pct: 0, resetIn: -3 * MIN, source: "cli", observedAgo: 40 * MIN, awaiting: true },
      { kind: "seven_day", pct: 47, resetIn: 5 * DAY + 3 * HOUR, source: "cli", observedAgo: 40 * MIN, slope: 0.3 },
    ],
    session: opus({ last_active_ms: 0 }),
    cliAgo: 40 * MIN,
  },
  estimated: {
    ...NORMAL,
    windows: [
      { kind: "five_hour", pct: 52, resetIn: HOUR + 35 * MIN, estimate: { pm: 15 * MIN, confidence: "high" }, source: "desktop", observedAgo: 6 * MIN, slope: 9 },
      { kind: "seven_day", pct: 38, resetIn: 5 * DAY, estimate: { pm: 2 * HOUR, confidence: "medium" }, source: "desktop", observedAgo: 6 * MIN, slope: 0.25 },
    ],
    session: opus({ ctx_pct: 64, ctx_tokens: 128_000, ctx_size: 200_000, ctx_basis: "heuristic", ctx_is_estimate: true, display_name: "Opus 5.5" }),
    desktop: desktopOk(6 * MIN),
  },
  warnings: {
    ...NORMAL,
    windows: [
      { kind: "five_hour", pct: 22, resetIn: 4 * HOUR + 2 * MIN, source: "cli", observedAgo: 1 * MIN, slope: 3 },
      { kind: "seven_day", pct: 44, resetIn: 6 * DAY + 1 * HOUR, source: "cli", observedAgo: 1 * MIN },
    ],
    desktop: () => ({ state: "schema_changed", version: 3 }),
    warnings: [{ type: "account_mismatch" }, { type: "no_plan_limits" }],
    hotkeyError: "Ctrl+Alt+U is already used by another app",
  },
  onboarding: {
    windows: [],
    session: () => null,
    desktop: () => ({ state: "not_found" }),
    cliAgo: null,
    transcriptsAgo: null,
    warnings: [],
    connection: notConfigured,
  },
};

function burnFor(w: WinSpec, pct: number, now: number, resetAt: number | null): Burn | null {
  if (!w.slope || w.stale || pct >= 99.5) return null;
  const t100 = now + ((100 - pct) / w.slope) * HOUR;
  const hits = resetAt !== null && t100 < resetAt;
  const pctAtReset = resetAt === null ? null : pct + (w.slope * (resetAt - now)) / HOUR;
  return {
    slope_pct_per_h: w.slope,
    t100_ms: hits || resetAt === null ? Math.round(t100) : null,
    pct_at_reset: pctAtReset === null ? null : Math.round(pctAtReset * 10) / 10,
    hits_limit_before_reset: hits,
  };
}

/** Deterministic snapshot of a scenario loaded at `t0`, as seen at `now`. */
export function buildSnapshot(scenario: Scenario, t0: number, now: number): Snapshot {
  const spec = SPECS[scenario];
  const windows: WindowView[] = spec.windows.map((w, i) => {
    const resetAt = w.resetIn === null ? null : t0 + w.resetIn;
    // Live creep at the burn rate so the 5 s updates exercise the transition path.
    const creep = w.slope && !w.stale && !w.awaiting ? (w.slope * Math.max(0, now - t0)) / HOUR : 0;
    const pct = Math.min(100, w.pct + creep);
    const reset: ResetInfo =
      resetAt === null
        ? { type: "unknown" }
        : w.estimate
          ? { type: "estimated", at_ms: resetAt, plus_minus_ms: w.estimate.pm, confidence: w.estimate.confidence }
          : { type: "exact", at_ms: resetAt };
    const period = w.kind === "five_hour" ? 5 * HOUR : 7 * DAY;
    return {
      kind: w.kind,
      pct: w.source === "desktop" ? Math.round(pct) : Math.round(pct * 10) / 10,
      reset,
      source: w.source,
      observed_at_ms: t0 - w.observedAgo,
      stale: w.stale ?? false,
      limit_reached: pct >= 99.5,
      phase: w.awaiting ? "reset_awaiting_data" : "active",
      burn: burnFor(w, pct, now, resetAt),
      spark: mockSpark({
        now,
        span: w.kind === "five_hour" ? DAY : 7 * DAY,
        period,
        resetAt: resetAt ?? t0 + period / 2,
        current: pct,
        gaps: w.gaps,
        seed: hashSeed(`${scenario}:${i}`),
      }),
    };
  });
  const session = spec.session(t0);
  if (session && session.last_active_ms === 0) session.last_active_ms = t0 - (spec.cliAgo ?? MIN);
  // Newest first, like Rust; the header session is the newest one here.
  const sessions = session ? [session, ...(spec.others?.(t0) ?? [])].sort((a, b) => b.last_active_ms - a.last_active_ms) : [];
  return {
    generated_ms: now,
    windows,
    session,
    sessions,
    health: {
      desktop: spec.desktop(t0),
      cli_last_capture_ms: spec.cliAgo === null ? null : t0 - spec.cliAgo,
      transcripts_last_activity_ms: spec.transcriptsAgo === null ? null : t0 - spec.transcriptsAgo,
    },
    warnings: spec.warnings,
  };
}

function pick<T extends string>(value: string | null, allowed: readonly T[], fallback: T): T {
  return value !== null && (allowed as readonly string[]).includes(value) ? (value as T) : fallback;
}

const CAPTURE = String.raw`"%LOCALAPPDATA%\ClaudeUsageWidget\cuw-capture.exe"`;
const FOREIGN_CMD = "npx -y ccstatusline@latest";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Creates the mock backend for the given URL params. */
export function createMockBackend(params: URLSearchParams): Backend {
  const scenario = pick(params.get("scenario"), SCENARIOS, "normal");
  const spec = SPECS[scenario];
  const t0 = Date.now();
  const view = pick(params.get("view"), VIEWS, "card");
  const updateMode = pick(params.get("update"), UPDATE_MODES, "default");
  const listeners = new Map<EventName, Set<(p: unknown) => void>>();

  let snapshot = buildSnapshot(scenario, t0, t0);
  let settings: Settings = {
    schema_version: 1,
    view: view === "pill" ? "pill" : "card",
    pinned: true,
    opacity: 1,
    ghost_opacity: 0.45,
    effect: pick(params.get("effect"), EFFECTS, "auto"),
    thresholds: [80, 95],
    notify_reset: true,
    hotkey: "Ctrl+Alt+U",
    stale_min: 15,
    ctx_overrides: { "claude-opus-5-5": 1_000_000 },
    show_project: true,
    start_with_windows: false,
    ctx_alerts: true,
    ctx_thresholds: [80, 90],
    toggle_hotkey: "Ctrl+Alt+H",
    auto_hide_fullscreen: true,
    check_updates: false,
    // Appearance: ?accent=teal &gauge=bar &scale=1.15 &dock=left|right|top &rows=sparklines,sources
    // (rows lists the card rows to show; "none" hides them all).
    accent: pick(params.get("accent"), ACCENTS, "auto"),
    gauge_style: pick(params.get("gauge"), ["ring", "bar"] as const, "ring"),
    ui_scale: Math.min(1.3, Math.max(0.85, Number(params.get("scale")) || 1)),
    card_rows: parseCardRows(params.get("rows")),
    dock: pick(params.get("dock"), ["off", "left", "right", "top"] as const, "off"),
  };
  let ui: UiState = {
    view,
    pinned: true,
    click_through: params.get("ghost") === "1",
    hotkey_error: spec.hotkeyError ?? null,
    toggle_hotkey_error: null,
    dock_expanded: false,
    hidden_reason: pick(params.get("hidden"), HIDDEN, "none"),
    update: updateMode === "1" ? MOCK_UPDATE : null,
  };
  const connParam = params.get("conn");
  let connection: ConnectionStatus =
    connParam === "foreign"
      ? { state: "foreign", command: FOREIGN_CMD }
      : connParam === "error"
        ? { state: "error", message: "~/.claude/settings.json is not valid JSON (line 12)" }
        : connParam === "connected"
          ? connected
          : connParam === "not_configured"
            ? notConfigured
            : spec.connection;

  const emit = (event: EventName, payload: unknown) => {
    for (const cb of listeners.get(event) ?? []) cb(structuredClone(payload));
  };

  setInterval(() => {
    snapshot = buildSnapshot(scenario, t0, Date.now());
    emit("snapshot", snapshot);
  }, MOCK_TICK_MS);

  const preview = (): ConnectPreview => {
    const before = connection.state === "foreign" ? connection.command : null;
    return {
      before,
      after: before ? `${CAPTURE} --wrap -- ${before}` : CAPTURE,
      shell: "pwsh",
      warnings:
        scenario === "warnings"
          ? [String.raw`C:\Users\tester\code\demo-app\.claude\settings.json sets its own statusLine; sessions in that project will not be captured.`]
          : [],
      selftest_ok: null,
    };
  };

  const handlers: Record<CommandName, (args: Record<string, unknown>) => unknown | Promise<unknown>> = {
    get_snapshot: () => snapshot,
    get_settings: () => settings,
    update_settings: (args) => {
      const patch = args.patch as Partial<Settings>;
      settings = { ...settings, ...patch };
      if (patch.hotkey !== undefined && ui.hotkey_error) {
        // Registering a different shortcut succeeds in the mock.
        ui = { ...ui, hotkey_error: null };
        emit("ui-state", ui);
      }
      return settings;
    },
    get_ui_state: () => ui,
    set_view: (args) => {
      const v = pick(String(args.view), VIEWS, ui.view);
      ui = { ...ui, view: v };
      if (v === "pill" || v === "card") settings = { ...settings, view: v };
      emit("ui-state", ui);
    },
    set_pinned: (args) => {
      ui = { ...ui, pinned: Boolean(args.pinned) };
      settings = { ...settings, pinned: ui.pinned };
      emit("ui-state", ui);
    },
    toggle_click_through: () => {
      ui = { ...ui, click_through: !ui.click_through };
      emit("ui-state", ui);
    },
    connection_status: () => connection,
    connect_claude_code: async (args) => {
      await sleep(args.dryRun ? 250 : 900);
      const p = preview();
      if (args.dryRun) return p;
      connection = { state: "connected", mode: p.before ? "pipe_grouped" : "pipe", original: connection.state === "foreign" ? connection.command : null };
      return { ...p, selftest_ok: true };
    },
    disconnect_claude_code: async () => {
      await sleep(300);
      connection = connection.state === "connected" && connection.original ? { state: "foreign", command: connection.original } : notConfigured;
      return connection;
    },
    open_data_folder: () => console.info("[mock] open_data_folder"),
    quit_app: () => console.info("[mock] quit_app"),
    get_history: async (args) => {
      // ?history=slow|error previews the History view's loading and error states.
      const mode = params.get("history");
      await sleep(mode === "slow" ? 60 * SEC : 120);
      if (mode === "error") throw new Error("history.jsonl could not be read (mock error)");
      return mockHistory({
        windows: spec.windows.map((w) => ({
          kind: w.kind,
          pct: snapshot.windows.find((live) => live.kind === w.kind)?.pct ?? w.pct,
          resetIn: w.resetIn,
          integers: w.source === "desktop",
          silentFor: w.stale ? w.observedAgo : undefined,
        })),
        t0,
        now: Date.now(),
        days: Number(args.days ?? MOCK_HISTORY_DAYS),
        seed: hashSeed(`${scenario}:history`),
      });
    },
    set_dock_expanded: (args) => {
      ui = { ...ui, dock_expanded: Boolean(args.expanded) };
      emit("ui-state", ui);
    },
    check_updates_now: async () => {
      await sleep(400);
      if (updateMode === "error") throw "Couldn't reach GitHub; check your connection";
      ui = { ...ui, update: updateMode === "none" ? null : MOCK_UPDATE };
      emit("ui-state", ui);
      return ui.update;
    },
    open_url: (args) => console.info("[mock] open_url", args.url),
  };

  return {
    async invoke<T>(cmd: CommandName, args: Record<string, unknown> = {}): Promise<T> {
      const result = await handlers[cmd](args);
      return structuredClone(result) as T;
    },
    async listen<T>(event: EventName, cb: (payload: T) => void): Promise<Unlisten> {
      const set = listeners.get(event) ?? new Set();
      listeners.set(event, set);
      const fn = cb as (p: unknown) => void;
      set.add(fn);
      return () => set.delete(fn);
    },
    async startDragging() {},
  };
}
