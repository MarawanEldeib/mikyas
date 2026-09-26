import { afterEach, describe, expect, it, vi } from "vitest";
import { MOCK_TICK_MS, SCENARIOS, SPARK_POINTS, buildSnapshot, createMockBackend } from "./mock";
import type { ConnectPreview, Settings, Snapshot, UiState } from "./types";

const T0 = Date.UTC(2026, 8, 24, 12, 0, 0);

describe("buildSnapshot", () => {
  it.each(SCENARIOS)("produces a well-formed snapshot for %s", (scenario) => {
    const s = buildSnapshot(scenario, T0, T0);
    expect(s.generated_ms).toBe(T0);
    const kinds = s.windows.map((w) => w.kind);
    if (kinds.length) expect(kinds).toEqual(["five_hour", "seven_day"]);
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
    const expanded = async (query: string) => (await createMockBackend(new URLSearchParams(query)).invoke<UiState>("get_ui_state")).dock_expanded;
    for (const view of ["settings", "sessions", "history"]) expect(await expanded(`dock=left&view=${view}`)).toBe(true);
    expect(await expanded("dock=top&view=card")).toBe(false);
    expect(await expanded("dock=right&view=pill")).toBe(false);
    expect(await expanded("dock=off&view=settings")).toBe(false);
  });

  it("previews, connects and disconnects", async () => {
    vi.useFakeTimers();
    const b = createMockBackend(new URLSearchParams("conn=foreign"));
    const dry = b.invoke<ConnectPreview>("connect_claude_code", { dryRun: true });
    await vi.advanceTimersByTimeAsync(1000);
    const preview = await dry;
    expect(preview.before).not.toBeNull();
    expect(preview.after).toContain("--wrap");
    expect(preview.selftest_ok).toBeNull();

    const real = b.invoke<ConnectPreview>("connect_claude_code", { dryRun: false });
    await vi.advanceTimersByTimeAsync(1000);
    expect((await real).selftest_ok).toBe(true);
    expect((await b.invoke<{ state: string }>("connection_status")).state).toBe("connected");

    const off = b.invoke<{ state: string }>("disconnect_claude_code");
    await vi.advanceTimersByTimeAsync(1000);
    expect((await off).state).toBe("foreign");
  });
});
