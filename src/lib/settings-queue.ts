// Optimistic settings: the last value the backend confirmed, the patches still in flight (in call
// order) and a local-only preview (a slider being dragged) on top. A failed patch simply drops out
// (the rollback), and a response never hides a newer patch that is still pending.

export class PatchQueue<T extends object> {
  #base: T | null = null;
  #baseId = 0;
  #nextId = 0;
  #pending = new Map<number, Partial<T>>();
  #preview: Partial<T> = {};

  /** Replaces the confirmed value (initial load), keeping what is pending or previewed. */
  reset(base: T): void {
    this.#base = base;
  }

  /** The value to show, or null before the first `reset`. */
  current(): T | null {
    if (!this.#base) return null;
    let v: T = this.#base;
    for (const p of this.#pending.values()) v = { ...v, ...p };
    return { ...v, ...this.#preview };
  }

  /** `current()` once a value exists. */
  value(): T {
    const v = this.current();
    if (!v) throw new Error("PatchQueue has no value yet");
    return v;
  }

  /** Local-only change, shown until the same keys are committed with `begin`. */
  preview(patch: Partial<T>): void {
    this.#preview = { ...this.#preview, ...patch };
  }

  /** Starts sending a patch; returns its id for `settle` / `fail`. */
  begin(patch: Partial<T>): number {
    const rest = { ...this.#preview };
    for (const k of Object.keys(patch)) delete (rest as Record<string, unknown>)[k];
    this.#preview = rest;
    const id = ++this.#nextId;
    this.#pending.set(id, patch);
    return id;
  }

  /** The backend applied patch `id` and answered with the full value. */
  settle(id: number, response: T): void {
    this.#pending.delete(id);
    // An older call's answer (arriving late) must not replace a newer one's.
    if (id > this.#baseId) {
      this.#base = response;
      this.#baseId = id;
    }
  }

  /** Patch `id` failed: it no longer shows. */
  fail(id: number): void {
    this.#pending.delete(id);
  }
}
