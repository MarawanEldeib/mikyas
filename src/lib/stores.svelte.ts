// Application state (Svelte 5 runes). Rust owns the view/pin/click-through state and pushes it
// via the `ui-state` event; the UI requests changes through commands and applies the result.

import { api, inTauri, onSnapshot, onUiState, type Unlisten } from "./ipc";
import { PatchQueue } from "./settings-queue";
import type { TickTargets } from "./tick";
import type { ConnectionStatus, Settings, Snapshot, UiState, ViewMode, WindowView } from "./types";

export class AppState {
  /** Latest snapshot; replaced wholesale on every `snapshot` event. */
  snapshot = $state.raw<Snapshot | null>(null);
  settings = $state.raw<Settings | null>(null);
  ui = $state.raw<UiState>({
    view: "card",
    pinned: true,
    click_through: false,
    hotkey_error: null,
    toggle_hotkey_error: null,
    dock_expanded: false,
    hidden_reason: "none",
    update: null,
    connection_lost: false,
  });
  connection = $state.raw<ConnectionStatus | null>(null);
  /** Wall clock used by every countdown/age; advanced by the tick scheduler. */
  now = $state(Date.now());
  /** View to return to when leaving settings, sessions or history. */
  previousView = $state<"pill" | "card">("card");
  ready = $state(false);
  error = $state<string | null>(null);
  readonly mock = !inTauri;

  #unSnapshot: Unlisten | null = null;
  #unUi: Unlisten | null = null;
  /** Set by the event listeners: pushed state is newer than a fetch started before it. */
  #pushedSnapshot = false;
  #pushedUi = false;
  #initializing = false;
  /** Settings as confirmed by Rust, with the patches still in flight and any local preview. */
  #settings = new PatchQueue<Settings>();
  /** Bumped on every connection write so a slow status read never replaces a newer one. */
  #connectionSeq = 0;

  /**
   * Subscribes to backend events and loads the initial state. Events that arrive while the
   * fetch is in flight win over the fetched values. Safe to call again after a failure (Retry).
   */
  async init(): Promise<void> {
    if (this.#initializing || (this.ready && this.#unSnapshot && this.#unUi)) return;
    this.#initializing = true;
    this.error = null;
    try {
      // Each subscription is made once; a retry only adds what a failed attempt left out.
      if (!this.#unSnapshot) {
        this.#pushedSnapshot = false;
        this.#unSnapshot = await onSnapshot((s) => {
          this.#pushedSnapshot = true;
          this.snapshot = s;
          this.now = Date.now();
        });
      }
      if (!this.#unUi) {
        this.#pushedUi = false;
        this.#unUi = await onUiState((u) => {
          this.#pushedUi = true;
          this.#applyUi(u);
        });
      }
      const connSeq = this.#connectionSeq;
      const [snapshot, settings, ui, connection] = await Promise.all([
        api.getSnapshot(),
        api.getSettings(),
        api.getUiState(),
        api.connectionStatus(),
      ]);
      if (!this.#pushedSnapshot) this.snapshot = snapshot;
      this.#settings.reset(settings);
      this.settings = this.#settings.current();
      if (connSeq === this.#connectionSeq) this.#setConnection(connection);
      if (settings.view === "pill" || settings.view === "card") this.previousView = settings.view;
      if (!this.#pushedUi) this.#applyUi(ui);
      else if (this.ui.view === "pill" || this.ui.view === "card") this.previousView = this.ui.view;
      this.now = Date.now();
      this.ready = true;
    } catch (e) {
      this.error = errorText(e);
    } finally {
      this.#initializing = false;
    }
  }

  dispose(): void {
    this.#unSnapshot?.();
    this.#unUi?.();
    this.#unSnapshot = null;
    this.#unUi = null;
  }

  #applyUi(u: UiState): void {
    const prev = this.ui;
    if (u.view === "pill" || u.view === "card") this.previousView = u.view;
    this.ui = u;
    // Settings shows the connection state: re-read it when Settings opens and whenever the
    // watchdog reports a change, so it is never the one loaded at startup.
    const opened = u.view === "settings" && prev.view !== "settings";
    if (this.ready && (opened || u.connection_lost !== prev.connection_lost)) void this.#refreshConnectionQuietly();
  }

  /**
   * Requests a view change. Applied once Rust has resized the window (not optimistically, so
   * a view never renders into the previous view's size); `ui-state` confirms it as well.
   */
  async setView(view: ViewMode): Promise<void> {
    if (view === this.ui.view) return;
    await this.#run(async () => {
      await api.setView(view);
      this.#applyUi({ ...this.ui, view });
    });
  }

  /** Leaves settings for the view it was opened from. */
  back(): Promise<void> {
    return this.setView(this.previousView);
  }

  async togglePinned(): Promise<void> {
    const before = this.ui.pinned;
    const pinned = !before;
    this.ui = { ...this.ui, pinned };
    await this.#run(
      () => api.setPinned(pinned),
      () => {
        // Unless a pushed ui-state has replaced the optimistic value meanwhile.
        if (this.ui.pinned === pinned) this.ui = { ...this.ui, pinned: before };
      },
    );
  }

  /** Local-only change (e.g. while a slider is dragged); commit with `updateSettings`. */
  previewSettings(patch: Partial<Settings>): void {
    this.#settings.preview(patch);
    this.settings = this.#settings.current();
  }

  /**
   * Persists a settings change, shown at once. A failure rolls it back, and the answer to an
   * older call never hides a newer change that is still on its way.
   */
  async updateSettings(patch: Partial<Settings>): Promise<void> {
    const id = this.#settings.begin(patch);
    this.settings = this.#settings.current();
    await this.#run(
      async () => {
        this.#settings.settle(id, await api.updateSettings(patch));
      },
      () => this.#settings.fail(id),
    );
    this.settings = this.#settings.current();
  }

  /** Fire-and-forget `updateSettings` for event handlers (errors land in `error`). */
  patch(patch: Partial<Settings>): void {
    void this.updateSettings(patch);
  }

  async refreshConnection(): Promise<void> {
    await this.#run(() => this.#readConnection());
  }

  /** Removes the statusline capture; the status is re-read afterwards even when it failed. */
  async disconnect(): Promise<void> {
    const seq = ++this.#connectionSeq;
    try {
      const status = await api.disconnectClaudeCode();
      if (seq === this.#connectionSeq) this.#setConnection(status);
    } finally {
      await this.#refreshConnectionQuietly();
    }
  }

  async #readConnection(): Promise<void> {
    const seq = ++this.#connectionSeq;
    const status = await api.connectionStatus();
    if (seq === this.#connectionSeq) this.#setConnection(status);
  }

  /** A background re-read: leaves `error` (which may belong to something else) alone. */
  async #refreshConnectionQuietly(): Promise<void> {
    try {
      await this.#readConnection();
    } catch (e) {
      console.warn("connection_status failed", e);
    }
  }

  #setConnection(status: ConnectionStatus): void {
    this.connection = status;
  }

  async #run(fn: () => Promise<unknown>, rollback?: () => void): Promise<void> {
    try {
      await fn();
      this.error = null;
    } catch (e) {
      rollback?.();
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
    // Every row of the Sessions view shows its own age.
    for (const row of s.sessions ?? []) pasts.push(row.last_active_ms);
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
