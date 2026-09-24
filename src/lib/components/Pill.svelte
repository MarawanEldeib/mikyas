<script lang="ts">
  import { ctxLabel, estimateTooltip, formatAge, formatPct, liveWindow, modelLabel, pillCountdown, splitUnits, windowLabel, windowShort } from "../format";
  import { notices } from "../notices";
  import { app } from "../stores.svelte";
  import type { WindowView } from "../types";
  import Ring from "./Ring.svelte";

  const snap = $derived(app.snapshot);
  const shown = $derived(pickWindows(snap?.windows ?? []).map((w) => liveWindow(w, app.now)));
  const warn = $derived(notices(snap));
  const session = $derived(snap?.session ?? null);

  function pickWindows(ws: WindowView[]): WindowView[] {
    const main = ["five_hour", "seven_day"]
      .map((k) => ws.find((w) => w.kind === k))
      .filter((w): w is WindowView => w !== undefined);
    return main.length ? main : ws.slice(0, 2);
  }

  function describe(w: WindowView): string {
    const parts = [`${windowLabel(w.kind)} limit ${formatPct(w.pct)}% used`];
    if (w.limit_reached) parts.push("limit reached");
    parts.push(`resets in ${pillCountdown(w, app.now)}`);
    if (w.stale) parts.push(`last updated ${formatAge(w.observed_at_ms, app.now)}`);
    const est = estimateTooltip(w.reset);
    if (est) parts.push(est);
    return parts.join(", ");
  }

  const hint = $derived(
    [session ? `${modelLabel(session)} · ${ctxLabel(session)}` : null, ...warn.map((n) => n.title), "Double-click for details"]
      .filter(Boolean)
      .join("\n"),
  );
</script>

<div class="pill" title={hint}>
  {#if shown.length === 0}
    <div class="empty">
      <span class="empty-title">No usage data yet</span>
      <span class="empty-sub">Double-click to set up</span>
    </div>
  {:else}
    {#each shown as w, i (w.kind)}
      {#if i > 0}<div class="sep" aria-hidden="true"></div>{/if}
      <div class="win" role="img" aria-label={describe(w)}>
        <Ring pct={w.pct} size={36} stale={w.stale} locked={w.limit_reached} />
        <div class="text">
          <span class="label">{windowShort(w.kind)}{#if w.stale}<span class="stale-tag">· stale</span>{/if}</span>
          <span class="count" class:crit={w.limit_reached}>{#each splitUnits(pillCountdown(w, app.now)) as run, j (j)}<span class:u={run.unit}>{run.text}</span>{/each}</span>
        </div>
      </div>
    {/each}
  {/if}
  {#if !app.ui.click_through}
    <!-- Keyboard path to the card (double-click is mouse-only). It ignores the pointer so the
         whole pill stays draggable, and draws its focus ring around the pill. -->
    <button type="button" class="expand" aria-label="Show details" onclick={() => app.setView("card")}></button>
  {/if}
  {#if warn.length}
    <span class="dot" aria-label="{warn.length} warning{warn.length > 1 ? 's' : ''}" role="img"></span>
  {/if}
</div>

<style>
  /* 240px wide: each half leaves 60px for text, enough for the widest countdown "~23h 59m". */
  .pill {
    position: relative;
    height: 100%;
    display: flex;
    align-items: center;
    padding: 0 10px;
    gap: 8px;
  }
  .win {
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .label {
    font-size: 11px;
    line-height: 14px;
    font-weight: 600;
    color: var(--fg-2);
    letter-spacing: 0.02em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stale-tag {
    margin-left: 4px;
    font-weight: 400;
  }
  .count {
    font-family: var(--font-display);
    font-size: 14px;
    line-height: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .count .u {
    font-size: 11.5px;
    font-weight: 500;
    color: var(--fg-2);
    letter-spacing: 0;
  }
  .count.crit {
    color: var(--crit);
  }
  .sep {
    flex: none;
    width: 1px;
    height: 32px;
    background: var(--divider);
  }
  .dot {
    position: absolute;
    top: 7px;
    right: 7px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn-fill);
    box-shadow: 0 0 0 1.5px rgb(var(--surface-rgb) / 0.9);
  }
  .expand {
    position: absolute;
    inset: 0;
    padding: 0;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    pointer-events: none;
  }
  .expand:focus-visible {
    outline-offset: -3px;
  }
  .empty {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-left: 2px;
  }
  .empty-title {
    font-weight: 600;
    font-size: 13px;
    line-height: 18px;
  }
  .empty-sub {
    color: var(--fg-2);
  }
</style>
