import { afterEach, describe, expect, it, vi } from "vitest";
import { MOCK_TICK_MS, SCENARIOS, SPARK_POINTS, buildSnapshot, createMockBackend } from "./mock";
import type { ConnectPreview, Settings, Snapshot, UiState } from "./types";

const T0 = Date.UTC(2026, 8, 24, 12, 0, 0);

describe("buildSnapshot", () => {
  it.each(SCENARIOS)("produces a well-formed snapshot for %s", (scenario) => {
    const s = buildSnapshot(scenario, T0, T0);
    expect(s.generated_ms).toBe(T0);
    const kinds = s.windows.map((w) => w.kind);
    if (kinds.length) expect(kinds.slice(0, 2)).toEqual(["five_hour", "seven_day"]);
    if (scenario === "extra") expect(kinds.slice(2)).toEqual(["seven_day_opus", "monthly_overage"]);
    for (const w of s.windows) {
      expect(w.label).toBeTruthy();
      expect(w.short).toBeTruthy();
      // Like Rust's `spark_span`: the window's length, at least a day and at most 30 days.
      const DAY = 24 * 3_600_000;
      const SPANS: Record<string, number> = { five_hour: DAY, seven_day: 7 * DAY, seven_day_opus: 7 * DAY, monthly_overage: 30 * DAY };
      expect(w.spark_span_ms, w.kind).toBe(SPANS[w.kind]);
    }
    for (const w of s.windows) {
      expect(w.pct).toBeGreaterThanOrEqual(0);
      expect(w.pct).toBeLessThanOrEqual(100);
      expect(w.spark).toHaveLength(SPARK_POINTS);
      for (const p of w.spark) {
        if (p.pct !== null) {
          expect(p.pct).toBeGreaterThanOrEqual(0);
          expect(p.pct).toBeLessThanOrEqual(100);
        }
      }
      // The latest reading ends on the live value.
      const last = [...w.spark].reverse().find((p) => p.pct !== null);
      expect(last?.pct).toBeCloseTo(w.pct, 0);
    }
  });

  it("is deterministic", () => {
    expect(buildSnapshot("normal", T0, T0)).toEqual(buildSnapshot("normal", T0, T0));
  });

  it("covers the special states", () => {
    expect(buildSnapshot("desktop-only", T0, T0).windows.every((w) => w.reset.type === "estimated")).toBe(true);
    expect(buildSnapshot("limit", T0, T0).windows[0].limit_reached).toBe(true);
    expect(buildSnapshot("reset", T0, T0).windows[0].phase).toBe("reset_awaiting_data");
    expect(buildSnapshot("stale", T0, T0).windows.every((w) => w.stale)).toBe(true);
    expect(buildSnapshot("no-session", T0, T0).session).toBeNull();
    expect(buildSnapshot("warnings", T0, T0).warnings.length).toBeGreaterThan(0);
    const onboarding = buildSnapshot("onboarding", T0, T0);
    expect(onboarding.windows).toEqual([]);
    expect(onboarding.health.desktop.state).toBe("not_found");
  });

  it("forecasts a limit hit in the high scenario", () => {
    const [five] = buildSnapshot("high", T0, T0).windows;
    expect(five.burn?.hits_limit_before_reset).toBe(true);
    expect(five.burn?.t100_ms).not.toBeNull();
  });
});

describe("createMockBackend", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("implements the commands and emits events", async () => {
    vi.useFakeTimers();
    const b = createMockBackend(new URLSearchParams("scenario=warnings&view=pill&conn=foreign"));
    const ui = await b.invoke<UiState>("get_ui_state");
    expect(ui.view).toBe("pill");
    expect(ui.hotkey_error).not.toBeNull();

    const states: UiState[] = [];
    const snaps: Snapshot[] = [];
    const off = await b.listen<UiState>("ui-state", (u) => states.push(u));
    await b.listen<Snapshot>("snapshot", (s) => snaps.push(s));

    await b.invoke("set_view", { view: "settings" });
    expect(states.at(-1)?.view).toBe("settings");
    const settings = await b.invoke<Settings>("get_settings");
    expect(settings.view).toBe("pill");

    const next = await b.invoke<Settings>("update_settings", { patch: { hotkey: "Ctrl+Alt+K" } });
    expect(next.hotkey).toBe("Ctrl+Alt+K");
    expect(states.at(-1)?.hotkey_error).toBeNull();

    off();
    await b.invoke("set_pinned", { pinned: false });
    expect(states.at(-1)?.pinned).not.toBe(false);

    vi.advanceTimersByTime(MOCK_TICK_MS);
    expect(snaps).toHaveLength(1);
  });

  it("starts a docked panel slid out, like the app", async () => {
    vi.useFakeTimers();
    const expanded = async (query: string) =>
      (await createMockBackend(new URLSearchParams(query)).invoke<UiState>("get_ui_state")).dock_expanded;
    for (const view of ["settings", "sessions", "history"]) expect(await expanded(`dock=left&view=${view}`)).toBe(true);
    expect(await expanded("dock=top&view=card")).toBe(false);
    expect(await expanded("dock=right&view=pill")).toBe(false);
    expect(await expanded("dock=off&view=settings")).toBe(false);
  });

  it("hides from the ×, logs the hint once, and never lets a patch touch the hint flag", async () => {
    vi.useFakeTimers();
    const info = vi.spyOn(console, "info").mockImplementation(() => {});
    const b = createMockBackend(new URLSearchParams("close=quit"));
    const states: UiState[] = [];
    await b.listen<UiState>("ui-state", (u) => states.push(u));
    const before = await b.invoke<Settings>("get_settings");
    expect(before.close_action).toBe("quit");
    expect(before.hide_hint_shown).toBe(false);
    expect((await b.invoke<Settings>("update_settings", { patch: { hide_hint_shown: true } })).hide_hint_shown).toBe(false);

    await b.invoke("hide_widget");
    await b.invoke("hide_widget");
    expect(states.at(-1)?.hidden_reason).toBe("user");
    expect(info.mock.calls.filter(([m]) => String(m).includes("toast"))).toHaveLength(1);
    const after = await b.invoke<Settings>("update_settings", { patch: { hide_hint_shown: false, close_action: "hide" } });
    expect(after.hide_hint_shown).toBe(true);
    expect(after.close_action).toBe("hide");

    await b.invoke("show_context_menu");
    expect(info).toHaveBeenCalledWith("[mock] show_context_menu");
    expect((await createMockBackend(new URLSearchParams()).invoke<Settings>("get_settings")).close_action).toBe("hide");
    info.mockRestore();
  });

  it("defaults the automations, and dismisses the connection-lost banner (?lost=1)", async () => {
    vi.useFakeTimers();
    const b = createMockBackend(new URLSearchParams("lost=1"));
    const s = await b.invoke<Settings>("get_settings");
    expect(s).toMatchObject({
      pace_alerts: true,
      reset_heads_up: true,
      weekly_recap: true,
      finished_alerts: true,
      finished_min_minutes: 3,
      connection_watchdog: true,
      per_display_position: true,
      tray_number: "worst",
    });
    const snap = await b.invoke<Snapshot>("get_snapshot");
    expect(snap.windows.every((w) => w.worked_since === false)).toBe(true);
    expect((await b.invoke<UiState>("get_ui_state")).connection_lost).toBe(true);
    const states: UiState[] = [];
    await b.listen<UiState>("ui-state", (u) => states.push(u));
    await b.invoke("dismiss_connection_warning");
    expect(states.at(-1)?.connection_lost).toBe(false);
    expect((await createMockBackend(new URLSearchParams()).invoke<UiState>("get_ui_state")).connection_lost).toBe(false);
  });

  it("starts a docked pill or card slid out with ?expanded=1", async () => {
    vi.useFakeTimers();
    const ui = (q: string) => createMockBackend(new URLSearchParams(q)).invoke<UiState>("get_ui_state");
    expect((await ui("dock=right&view=card&expanded=1")).dock_expanded).toBe(true);
    expect((await ui("dock=right&view=pill&expanded=1")).dock_expanded).toBe(true);
    expect((await ui("dock=off&view=card&expanded=1")).dock_expanded).toBe(false);
  });

  it("previews, connects and disconnects", async () => {
    vi.useFakeTimers();
    const b = createMockBackend(new URLSearchParams("conn=foreign"));
    const dry = b.invoke<ConnectPreview>("connect_claude_code", { dryRun: true });
    await vi.advanceTimersByTimeAsync(1000);
    const preview = await dry;
    expect(preview.before).toBe("npx -y ccstatusline@latest");
    expect(preview.selftest_ok).toBeNull();

    const real = b.invoke<ConnectPreview>("connect_claude_code", { dryRun: false });
    await vi.advanceTimersByTimeAsync(1000);
    expect((await real).selftest_ok).toBe(true);
    expect(await b.invoke("connection_status")).toEqual({ state: "connected", mode: "pipe", original: "npx -y ccstatusline@latest" });

    const off = b.invoke<{ state: string }>("disconnect_claude_code");
    await vi.advanceTimersByTimeAsync(1000);
    expect((await off).state).toBe("foreign");
  });

  // The forms crates/core/src/cmdline.rs writes: the shim path quoted with forward slashes, and
  // under Bash (Claude Code's shell when Git for Windows is installed) `--tee | <original>`, or
  // `--default` when there was no statusline.
  it("previews the command the real Connect writes", async () => {
    vi.useFakeTimers();
    const shim = `"C:/Users/tester/AppData/Local/Mikyas/bin/mikyas-capture.exe"`;
    const connect = async (q: string) => {
      const b = createMockBackend(new URLSearchParams(q));
      const dry = b.invoke<ConnectPreview>("connect_claude_code", { dryRun: true });
      await vi.advanceTimersByTimeAsync(1000);
      const real = b.invoke<ConnectPreview>("connect_claude_code", { dryRun: false });
      await vi.advanceTimersByTimeAsync(1000);
      await real;
      return { preview: await dry, status: await b.invoke("connection_status") };
    };

    const wrapped = await connect("conn=foreign");
    expect(wrapped.preview).toMatchObject({ shell: "bash", after: `${shim} --tee | npx -y ccstatusline@latest` });
    expect(wrapped.status).toMatchObject({ mode: "pipe" });

    const fresh = await connect("conn=not_configured");
    expect(fresh.preview).toMatchObject({ shell: "bash", before: null, after: `${shim} --default` });
    expect(fresh.status).toEqual({ state: "connected", mode: "default", original: null });
    // Already connected without a statusline of its own: the same `--default` form.
    expect(await createMockBackend(new URLSearchParams("conn=connected")).invoke("connection_status")).toEqual(fresh.status);
  });
});
