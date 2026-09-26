<script lang="ts">
  import { clampPct } from "../color";
  import { startDock } from "../dock";
  import { ctxLabel, formatPct, liveWindow, modelLabel, pillCountdown, splitUnits, windowShort } from "../format";
  import { api } from "../ipc";
  import { connectionBannerVisible } from "../connection";
  import { notices } from "../notices";
  import { describeWindow } from "../pill";
  import { app } from "../stores.svelte";
  import { mainWindows, mutedColor } from "../windows";
  import { WORKED_SINCE_TIP, showWorkedSince } from "../worked";
  import Icon from "./Icon.svelte";
  import Ring from "./Ring.svelte";

  const snap = $derived(app.snapshot);
  const shown = $derived(mainWindows(snap?.windows ?? []).map((w) => liveWindow(w, app.now)));
  // The reconnect banner only fits the card: in the pill the loss is a warning (dot + tooltip).
  const warn = $derived([
    ...notices(snap),
    ...(connectionBannerVisible(app.ui, app.settings?.connection_watchdog ?? true)
      ? [{ title: "Status line was changed — open the card to reconnect" }]
      : []),
  ]);
  const session = $derived(snap?.session ?? null);
  const bars = $derived(app.settings?.gauge_style === "bar");
  // The window controls (WindowControls.svelte) take the top-right corner while shown; the bars'
  // first countdown, the right ring's stale tag and the warning dot there fade out meanwhile.

  // Slides the widget back into its strip when the pointer leaves (edge dock).
  startDock(app, api.setDockExpanded);

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
  {:else if bars}
    <!-- One grid for both rows so the bars line up whatever the labels and countdowns say. -->
    <div class="bars">
      {#each shown as w, i (w.kind)}
        <div class="brow" role="img" aria-label={describeWindow(w, app.now)}>
          <span class="label"
            >{windowShort(w.kind)}{#if w.stale}<span class="stale-tag">· stale</span>{/if}</span
          >
          <span class="track"><span class="bfill" style:width="{clampPct(w.pct)}%" style:background={mutedColor(w)}></span></span>
          <span class="bpct" class:crit={w.limit_reached} class:muted={w.stale}>
            {#if w.limit_reached}<Icon name="lock" size={13} />{:else}{formatPct(clampPct(w.pct))}<span class="u">%</span
              >{#if showWorkedSince(w)}<span class="worked" title={WORKED_SINCE_TIP}>▲</span>{/if}{/if}
          </span>
          <span class="count bcount" class:crit={w.limit_reached} data-under-controls={i === 0 ? "" : undefined}
            >{#each splitUnits(pillCountdown(w, app.now)) as run, j (j)}<span class:u={run.unit}>{run.text}</span>{/each}</span
          >
        </div>
      {/each}
    </div>
  {:else}
    {#each shown as w, i (w.kind)}
      {#if i > 0}<div class="sep" aria-hidden="true"></div>{/if}
      <div class="win" role="img" aria-label={describeWindow(w, app.now)}>
        <Ring pct={w.pct} size={36} stale={w.stale} locked={w.limit_reached} />
        <div class="text">
          <span class="label"
            >{windowShort(w.kind)}{#if showWorkedSince(w)}<span class="worked" title={WORKED_SINCE_TIP}>▲</span>{/if}{#if w.stale}<span
                class="stale-tag"
                data-under-controls={i === shown.length - 1 ? "" : undefined}>· stale</span
              >{/if}</span
          >
          <span class="count" class:crit={w.limit_reached}
            >{#each splitUnits(pillCountdown(w, app.now)) as run, j (j)}<span class:u={run.unit}>{run.text}</span>{/each}</span
          >
        </div>
      </div>
    {/each}
  {/if}
  {#if !app.ui.click_through}
    <!-- Keyboard path to the card (double-click is mouse-only). It ignores the pointer so the
         whole pill stays draggable, and draws its focus ring around the pill. -->
    <button type="button" class="expand" aria-label="Show details" data-focus-home onclick={() => app.setView("card")}></button>
  {/if}
  {#if warn.length}
    <span class="dot" aria-label="{warn.length} warning{warn.length > 1 ? 's' : ''}" role="img" data-under-controls></span>
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
  /* "▲": Claude worked after this reading (ring mode: after the label, the % is in the ring). */
  .worked {
    margin-left: 2px;
    font-size: 8px;
    font-weight: 400;
    color: var(--fg-2);
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
  /* Bars: label · bar · % · countdown, two rows. Fixed number columns keep the bars from
     twitching as the countdown text changes; the widest countdown "~23h 59m" fits in 56px. */
  .bars {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) 36px 56px;
    column-gap: 8px;
    row-gap: 10px;
    padding: 0 2px;
  }
  .brow {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: subgrid;
    align-items: center;
    min-width: 0;
  }
  .track {
    height: 6px;
    border-radius: 3px;
    background: var(--track);
    overflow: hidden;
  }
  .bfill {
    display: block;
    height: 100%;
    border-radius: 3px;
    transition:
      width var(--dur) var(--ease),
      background-color var(--dur) ease-out;
  }
  .bpct {
    display: flex;
    justify-content: flex-end;
    align-items: baseline;
    font-family: var(--font-display);
    font-size: 14px;
    line-height: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
    white-space: nowrap;
  }
  .bpct .u {
    font-size: 10.5px;
    font-weight: 600;
    color: var(--fg-2);
    margin-left: 0.5px;
  }
  .bpct.muted {
    color: var(--fg-2);
  }
  .bpct.crit {
    align-self: center;
    color: var(--crit);
  }
  .bcount {
    justify-self: end;
    max-width: 100%;
    font-size: 12.5px;
    line-height: 18px;
  }
  .bcount .u {
    font-size: 10.5px;
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
