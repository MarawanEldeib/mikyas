<script lang="ts" module>
  import type { RangeKey } from "../history";

  // The chosen range survives leaving and reopening the view (not restarts).
  let lastRange: RangeKey = "7d";
</script>

<script lang="ts">
  import { formatPct, liveWindow, windowLabel } from "../format";
  import { FETCH_DAYS, RANGES, barScale, dayBars, rangeDomain, resetsText, summaryText, windowSummary } from "../history";
  import { api } from "../ipc";
  import { app, errorText } from "../stores.svelte";
  import type { HistoryData, HistoryWindow } from "../types";
  import { untrack } from "svelte";
  import HistoryChart from "./HistoryChart.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";

  /** Reload with a new snapshot at most this often. */
  const REFRESH_MS = 60_000;
  /** Content width: 360 minus the 12px insets (the right one includes the scrollbar gutter). */
  const WIDTH = 336;

  let data = $state.raw<HistoryData | null>(null);
  let error = $state<string | null>(null);
  let slow = $state(false);
  let range = $state<RangeKey>(lastRange);
  /** Day bar being hovered or focused (its start); shown in the section header. */
  let picked = $state<number | null>(null);
  let loadedAt = 0;
  let loading = false;

  async function load(): Promise<void> {
    if (loading) return;
    loading = true;
    loadedAt = Date.now();
    // Only a slow load shows a message, so a fast one never flashes.
    const timer = setTimeout(() => (slow = true), 150);
    try {
      data = await api.getHistory(FETCH_DAYS);
      error = null;
    } catch (e) {
      error = errorText(e);
    } finally {
      clearTimeout(timer);
      slow = false;
      loading = false;
    }
  }

  // Loads on open, then again with a newer snapshot at most once a minute.
  $effect(() => {
    void app.snapshot;
    untrack(() => {
      if (Date.now() - loadedAt >= REFRESH_MS) void load();
    });
  });

  function choose(key: RangeKey) {
    range = key;
    lastRange = key;
  }

  function onRangeKey(e: KeyboardEvent) {
    const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const i = RANGES.findIndex((r) => r.key === range);
    const next = RANGES[(i + step + RANGES.length) % RANGES.length];
    choose(next.key);
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-range="${next.key}"]`)?.focus();
  }

  const spec = $derived(RANGES.find((r) => r.key === range) ?? RANGES[1]);
  const domain = $derived(data ? rangeDomain(data, spec.span) : null);
  const charts = $derived.by(() => {
    const ws = data?.windows ?? [];
    const main = ["five_hour", "seven_day"]
      .map((k) => ws.find((w) => w.kind === k))
      .filter((w): w is HistoryWindow => w !== undefined);
    return main.length ? main : ws.slice(0, 2);
  });
  const weekly = $derived(data?.windows.find((w) => w.kind === "seven_day") ?? null);
  const bars = $derived(weekly ? dayBars(weekly.days, spec.bars, app.now) : []);
  const scale = $derived(barScale(bars.map((b) => b.value)));
  const lead = $derived(charts[0] ?? null);
  const summary = $derived(lead && domain ? windowSummary(lead, domain) : null);

  function current(kind: string): number | null {
    const w = app.window(kind);
    return w ? liveWindow(w, app.now).pct : null;
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape" && !e.defaultPrevented) void app.back();
  }}
/>

<div class="view">
  <header class="bar">
    <IconButton icon="back" label="Back" size="m" onclick={() => app.back()} />
    <h1>History</h1>
    <div class="seg" role="radiogroup" aria-label="Time range" tabindex="-1" onkeydown={onRangeKey}>
      {#each RANGES as r (r.key)}
        <button
          type="button"
          role="radio"
          data-range={r.key}
          aria-checked={r.key === range}
          aria-label={r.name}
          tabindex={r.key === range ? 0 : -1}
          onclick={() => choose(r.key)}>{r.label}</button
        >
      {/each}
    </div>
  </header>

  <div class="body" data-no-drag>
    {#if error && !data}
      <div class="state" role="alert">
        <span class="state-icon crit" aria-hidden="true"><Icon name="warning" size={18} /></span>
        <strong>Couldn't load the history</strong>
        <p>{error}</p>
        <button type="button" class="btn" onclick={() => load()}>Try again</button>
      </div>
    {:else if !data}
      <div class="state" aria-busy="true">
        {#if slow}<p>Loading history…</p>{/if}
      </div>
    {:else if !domain || charts.length === 0}
      <div class="state">
        <span class="state-icon" aria-hidden="true"><Icon name="info" size={18} /></span>
        <strong>No history yet</strong>
        <p>Usage is recorded while the widget runs. Check back after using Claude for a while.</p>
      </div>
    {:else}
      {#each charts as w (w.kind)}
        <HistoryChart
          window={w}
          {domain}
          range={spec.key}
          label={windowLabel(w.kind)}
          rangeName={spec.name}
          current={current(w.kind)}
          now={app.now}
          width={WIDTH}
        />
      {/each}

      {#if bars.length}
        {@const detail = bars.find((b) => b.start === picked)?.detail}
        <section class="days" aria-labelledby="days-h">
          <div class="head">
            <h2 id="days-h">Weekly budget used per day</h2>
            <span
              class="scale"
              class:picked={detail}
              title={detail ? undefined : `Bar heights are scaled: a full bar is ${scale}% of the weekly limit`}
              >{detail ?? `full bar = ${scale}%`}</span
            >
          </div>
          <ol class="bars" class:narrow={bars.length > 7}>
            {#each bars as b (b.start)}
              <li>
                <button
                  type="button"
                  class="day"
                  class:today={b.today}
                  class:nodata={!b.hasData}
                  style:--p={(b.value / scale) * 100}
                  aria-label={b.full}
                  title={b.full}
                  onclick={() => (picked = b.start)}
                  onfocus={() => (picked = b.start)}
                  onblur={() => (picked = null)}
                  onmouseenter={() => (picked = b.start)}
                  onmouseleave={() => (picked = null)}
                >
                  <span class="col">
                    {#if b.hasData}
                      {#if b.value >= 0.5}<span class="fill"></span>{/if}
                      <span class="val">{formatPct(b.value)}</span>
                    {/if}
                  </span>
                  <span class="dl">{b.label}</span>
                </button>
              </li>
            {/each}
          </ol>
        </section>
      {/if}

      {#if summary && lead}
        {@const label = windowLabel(lead.kind)}
        <p class="summary" title="{summaryText(label, summary)} · {spec.name}">
          {#if summary.peak === null}
            <span>No {label} data</span>
          {:else}
            <span>{label} peak <b>{formatPct(summary.peak)}%</b></span>
            <span aria-hidden="true">·</span>
            {#if summary.resets > 0}
              <!-- Legend: the chart's baseline with a reset mark under it. -->
              <svg class="legend" width="11" height="10" viewBox="0 0 11 10" aria-hidden="true">
                <title>Reset marks under the charts</title>
                <line class="legend-base" x1="0" x2="11" y1="1.5" y2="1.5" />
                <line class="legend-tick" x1="5.5" x2="5.5" y1="4" y2="8" />
              </svg>
            {/if}
            <span>{resetsText(summary.resets)}</span>
          {/if}
          <span aria-hidden="true">·</span>
          <span>{spec.name}</span>
        </p>
      {/if}
      {#if error}<p class="stale-note" role="status">Showing the last loaded history ({error})</p>{/if}
    {/if}
  </div>
</div>

<style>
  /* Mock stage size (browser preview only); zero specificity so app.css can override it. */
  :global(:where(html.mock[data-view="history"])) {
    --mock-w: 360px;
    --mock-h: 380px;
  }
  .view {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 44px;
    /* The right 42px are the window controls' × (24px, 10px in) and an 8px gap. */
    padding: 0 42px 0 6px;
    flex: none;
    border-bottom: 1px solid var(--divider);
  }
  h1 {
    margin: 0;
    font-size: 14px;
    line-height: 20px;
    font-weight: 600;
  }
  .seg {
    display: flex;
    gap: 2px;
    margin-left: auto;
    padding: 2px;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
  }
  .seg button {
    min-width: 36px;
    height: 22px;
    padding: 0 8px;
    border: 0;
    border-radius: 3px;
    background: transparent;
    color: var(--fg-2);
    font-size: 11px;
    font-weight: 500;
    transition:
      background-color 120ms ease-out,
      color 120ms ease-out;
  }
  .seg button:hover {
    background: var(--fill-hover);
    color: var(--fg);
  }
  .seg button[aria-checked="true"] {
    background: rgb(var(--surface-rgb));
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    color: var(--fg);
    font-weight: 600;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
    overflow-y: auto;
    overscroll-behavior: contain;
    /* The 10px scrollbar gutter completes the 12px right inset (as in Settings). */
    padding: 10px 2px 10px 12px;
    scrollbar-gutter: stable;
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    height: 16px;
  }
  h2 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
  }
  .scale {
    color: var(--fg-2);
    font-size: 11px;
  }
  .scale.picked {
    color: var(--fg);
  }
  .days {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .bars {
    display: flex;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .bars > li {
    flex: 1;
    min-width: 0;
  }
  .day {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
  }
  .day:hover {
    background: var(--fill-hover);
  }
  /* Bars reach 32px at the scale maximum; the 12px above leave room for the value. */
  .col {
    position: relative;
    align-self: stretch;
    height: 44px;
    box-shadow: inset 0 -1px 0 var(--divider);
  }
  .fill {
    position: absolute;
    left: 50%;
    bottom: 0;
    width: 18px;
    height: max(2px, calc(var(--p) * 0.32px));
    border-radius: 3px 3px 0 0;
    background: color-mix(in srgb, var(--fg) 30%, transparent);
    transform: translateX(-50%);
  }
  .narrow .fill {
    width: 12px;
  }
  .today .fill {
    background: var(--accent);
  }
  .val {
    position: absolute;
    left: 0;
    right: 0;
    bottom: calc(max(2px, var(--p) * 0.32px) + 2px);
    color: var(--fg-2);
    font-size: 10px;
    line-height: 10px;
    text-align: center;
  }
  .today .val {
    color: var(--fg);
    font-weight: 600;
  }
  /* Two weeks leave no room for every value: today's shows, the rest on hover or focus. */
  .narrow .day:not(.today, :hover, :focus-visible) .val {
    visibility: hidden;
  }
  .dl {
    color: var(--fg-2);
    font-size: 10px;
    line-height: 12px;
  }
  .today .dl {
    color: var(--fg);
    font-weight: 600;
  }
  /* No rows that day: a dashed baseline instead of a 0% one. */
  .nodata .col {
    box-shadow: none;
    background: linear-gradient(to right, var(--divider) 50%, transparent 0) bottom / 4px 1px repeat-x;
  }
  .nodata .dl {
    color: var(--fg-3);
  }
  .summary {
    display: flex;
    align-items: center;
    gap: 5px;
    margin: 0;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 16px;
    white-space: nowrap;
  }
  .summary b {
    color: var(--fg);
    font-weight: 600;
  }
  /* Legend for the reset marks under the charts, drawn like them (HistoryChart .base/.reset). */
  .legend {
    flex: none;
    overflow: visible;
  }
  .legend-base {
    stroke: var(--fg-3);
    stroke-width: 1;
    opacity: 0.6;
  }
  .legend-tick {
    stroke: var(--fg-2);
    stroke-width: 1.5;
    stroke-linecap: round;
  }
  .stale-note {
    margin: 0;
    color: var(--warn);
    font-size: 11px;
  }
  .state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 0 28px 16px;
    text-align: center;
  }
  .state-icon {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    margin-bottom: 4px;
    border-radius: 10px;
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--fg-2);
  }
  .state-icon.crit {
    background: var(--crit-bg);
    color: var(--crit);
  }
  .state strong {
    font-size: 13px;
    font-weight: 600;
  }
  .state p {
    margin: 0;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
    overflow-wrap: anywhere;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    height: 28px;
    margin-top: 6px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    font-weight: 500;
  }
  .btn:hover {
    background: var(--fill-control-hover);
  }
</style>
