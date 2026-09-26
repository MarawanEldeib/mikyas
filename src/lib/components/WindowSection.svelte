<script lang="ts">
  import { clampPct, fillColor } from "../color";
  import { estimateTooltip, formatAgeShort, formatPct, resetLine, windowLabel } from "../format";
  import type { WindowView } from "../types";
  import BurnLine from "./BurnLine.svelte";
  import Icon from "./Icon.svelte";
  import Sparkline from "./Sparkline.svelte";

  interface Props {
    window: WindowView;
    now: number;
    /** Optional rows (Settings → Card rows). */
    sparkline?: boolean;
    burn?: boolean;
  }

  let { window: w, now, sparkline = true, burn = true }: Props = $props();

  const p = $derived(clampPct(w.pct));
  const label = $derived(windowLabel(w.kind));
  const span = $derived(w.kind === "five_hour" ? "24 hours" : "7 days");
  // Stale data, or a window that just reset and has no new reading yet, is drawn neutral: the
  // history before the reset should not be coloured by the fresh 0%.
  const muted = $derived(w.stale || w.phase === "reset_awaiting_data");
  const color = $derived(muted ? "var(--fg-3)" : fillColor(p));
  const tip = $derived(estimateTooltip(w.reset));
  // A reached limit's burn line says when it is usable again ("… at 21:36"); the reset line
  // then keeps only the countdown, so the clock shows once.
  const clock = $derived(!(burn && w.limit_reached));
</script>

<section class="win" class:no-burn={!burn} aria-label="{label} limit">
  <div class="top">
    <div class="figures">
      <div class="headline">
        <span class="pct" class:crit={w.limit_reached}>{formatPct(p)}<span class="unit">%</span></span>
        {#if w.limit_reached}
          <span class="lock" title="Limit reached"><Icon name="lock" size={13} /></span>
        {/if}
        <span class="label">{label}</span>
        {#if w.stale}
          <span class="stale" title="Last updated {formatAgeShort(w.observed_at_ms, now)} ago">· {formatAgeShort(w.observed_at_ms, now)} old</span>
        {/if}
      </div>
      <div class="reset" title={tip}>
        {resetLine(w, now, {}, clock)}{#if tip}<span class="pm" aria-hidden="true">±</span>{/if}
      </div>
    </div>
    {#if sparkline}
      <Sparkline points={w.spark} pct={p} stale={muted} width={100} height={36} label="{label} usage over the last {span}" />
    {/if}
  </div>
  <div class="bar" role="progressbar" aria-label="{label} usage" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p)}>
    <div class="fill" style:width="{p}%" style:background={color}></div>
  </div>
  {#if burn}
    <BurnLine window={w} {now} />
  {/if}
</section>

<style>
  .win {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    /* Constant height (burn line or not) keeps both sections aligned across states: headline 26
       + 1 + reset 14, then bar 4 and burn line 14, each after a 4px gap. Two sections fit the
       232px card exactly. Hiding the burn row takes its 14px line and 4px gap off (window.rs
       shrinks the card by as much). */
    min-height: 67px;
  }
  .win.no-burn {
    min-height: 49px;
  }
  .top {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 8px;
  }
  .figures {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  /* A fixed height (the 24px figure plus 2px of air above the reset line), so baseline-aligned
     runs of other sizes can't stretch the row and squeeze the bar and burn line below it. */
  .headline {
    display: flex;
    align-items: baseline;
    gap: 6px;
    height: 26px;
    white-space: nowrap;
  }
  .pct {
    font-family: var(--font-display);
    font-size: 24px;
    line-height: 24px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .pct.crit {
    color: var(--crit);
  }
  /* Inheriting the 24px line-height, this 14px run would make the figure's line box 28px. */
  .unit {
    font-size: 14px;
    line-height: 1;
    font-weight: 600;
    margin-left: 1px;
    color: var(--fg-2);
    letter-spacing: 0;
  }
  .lock {
    align-self: center;
    color: var(--crit);
    margin-left: -2px;
  }
  .label {
    font-size: 12px;
    font-weight: 600;
    color: var(--fg-2);
  }
  .stale {
    font-size: 11px;
    color: var(--fg-2);
  }
  .reset {
    font-size: 11px;
    line-height: 14px;
    color: var(--fg-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Marks an estimated reset; the tooltip carries the margin and confidence. */
  .pm {
    display: inline-block;
    margin-left: 4px;
    padding: 0 3px;
    border-radius: 3px;
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    font-size: 10px;
    line-height: 12px;
    color: var(--fg-2);
  }
  .bar {
    height: 4px;
    border-radius: 2px;
    background: var(--track);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 2px;
    min-width: 0;
    transition:
      width var(--dur) var(--ease),
      background-color var(--dur) ease-out;
  }
</style>
