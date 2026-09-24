// Application state (Svelte 5 runes). Rust owns the view/pin/click-through state and pushes it
// via the `ui-state` event; the UI requests changes through commands and applies the result.

import { api, inTauri, onSnapshot, onUiState, type Unlisten } from "./ipc";
import type { TickTargets } from "./tick";
import type { ConnectionStatus, Settings, Snapshot, UiState, ViewMode, WindowView } from "./types";

class AppState {
  /** Latest snapshot; replaced wholesale on every `snapshot` event. */
  snapshot = $state.raw<Snapshot | null>(null);
  settings = $state.raw<Settings | null>(null);
  ui = $state.raw<UiState>({ view: "card", pinned: true, click_through: false, hotkey_error: null });
  connection = $state.raw<ConnectionStatus | null>(null);
  /** Wall clock used by every countdown/age; advanced by the tick scheduler. */
  now = $state(Date.now());
  /** View to return to when leaving settings. */
  previousView = $state<"pill" | "card">("card");
  ready = $state(false);
  error = $state<string | null>(null);
  readonly mock = !inTauri;

  #unlisten: Unlisten[] = [];

  /** Loads the initial state and subscribes to backend events. */
  async init(): Promise<void> {
    try {
      this.#unlisten.push(
        await onSnapshot((s) => {
          this.snapshot = s;
          this.now = Date.now();
        }),
        await onUiState((u) => this.#applyUi(u)),
      );
      const [snapshot, settings, ui, connection] = await Promise.all([
        api.getSnapshot(),
        api.getSettings(),
        api.getUiState(),
        api.connectionStatus(),
      ]);
      this.snapshot = snapshot;
      this.settings = settings;
      this.connection = connection;
      if (settings.view !== "settings") this.previousView = settings.view;
      this.#applyUi(ui);
      this.now = Date.now();
      this.ready = true;
    } catch (e) {
      this.error = errorText(e);
    }
  }

  dispose(): void {
    for (const u of this.#unlisten.splice(0)) u();
  }

  #applyUi(u: UiState): void {
    if (u.view !== "settings") this.previousView = u.view;
    this.ui = u;
  }

  /** Requests a view change; applied optimistically, confirmed by `ui-state`. */
  async setView(view: ViewMode): Promise<void> {
    if (view === this.ui.view) return;
    this.#applyUi({ ...this.ui, view });
    await this.#run(() => api.setView(view));
  }

  /** Leaves settings for the view it was opened from. */
  back(): Promise<void> {
    return this.setView(this.previousView);
  }

  async togglePinned(): Promise<void> {
    const pinned = !this.ui.pinned;
    this.ui = { ...this.ui, pinned };
    await this.#run(() => api.setPinned(pinned));
  }

  /** Local-only change (e.g. while a slider is dragged); commit with `updateSettings`. */
  previewSettings(patch: Partial<Settings>): void {
    if (this.settings) this.settings = { ...this.settings, ...patch };
  }

  /** Persists a settings change; the returned settings replace the local copy. */
  async updateSettings(patch: Partial<Settings>): Promise<void> {
    this.previewSettings(patch);
    await this.#run(async () => {
      this.settings = await api.updateSettings(patch);
    });
  }

  async refreshConnection(): Promise<void> {
    await this.#run(async () => {
      this.connection = await api.connectionStatus();
    });
  }

  async #run(fn: () => Promise<unknown>): Promise<void> {
    try {
      await fn();
      this.error = null;
    } catch (e) {
      this.error = errorText(e);
    }
  }

  /** The window of a kind, if present. */
  window(kind: string): WindowView | undefined {
    return this.snapshot?.windows.find((w) => w.kind === kind);
  }

  /** Every instant the UI renders relative to `now` (for the tick scheduler). */
  tickTargets(): TickTargets {
    const s = this.snapshot;
    if (!s) return { deadlines: [], pasts: [] };
    const deadlines: number[] = [];
    const pasts: number[] = [];
    for (const w of s.windows) {
      if (w.reset.type !== "unknown") deadlines.push(w.reset.at_ms);
      pasts.push(w.observed_at_ms);
    }
    if (s.session) pasts.push(s.session.last_active_ms);
    if (s.health.cli_last_capture_ms !== null) pasts.push(s.health.cli_last_capture_ms);
    if (s.health.transcripts_last_activity_ms !== null) pasts.push(s.health.transcripts_last_activity_ms);
    if (s.health.desktop.state === "ok" && s.health.desktop.last_sample_ms !== null) {
      pasts.push(s.health.desktop.last_sample_ms);
    }
    return { deadlines, pasts };
  }
}

export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return "Unknown error";
  }
}

export const app = new AppState();
