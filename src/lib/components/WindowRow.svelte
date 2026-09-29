<script lang="ts">
  // A compact row for a window beyond the main two (e.g. a weekly Opus limit): name, %, reset.
  import { clampPct } from "../color";
  import { estimateTooltip, formatAgeShort, formatPct, resetLine, windowLabel } from "../format";
  import type { WindowView } from "../types";
  import { mutedColor } from "../windows";
  import Icon from "./Icon.svelte";

  interface Props {
    window: WindowView;
    now: number;
  }

  let { window: w, now }: Props = $props();

  const p = $derived(clampPct(w.pct));
  const label = $derived(windowLabel(w));
  const color = $derived(mutedColor({ ...w, pct: p }));
  const tip = $derived(estimateTooltip(w.reset, now));
</script>

<div class="row" role="group" aria-label="{label} limit" data-kind={w.kind}>
  <span class="name" title={label}>{label}</span>
  <div class="bar" role="progressbar" aria-label="{label} usage" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p)}>
    <div class="fill" style:width="{p}%" style:background={color}></div>
  </div>
  <span class="pct" class:crit={w.limit_reached}>
    {#if w.limit_reached}<span class="lock" title="Limit reached"><Icon name="lock" size={11} /></span>{/if}{formatPct(p)}%
  </span>
  <span class="reset" title={w.stale ? `Last updated ${formatAgeShort(w.observed_at_ms, now)} ago` : tip}>
    {#if w.stale}{formatAgeShort(w.observed_at_ms, now)} old ·
    {/if}{resetLine(w, now, {}, false)}
  </span>
</div>

<style>
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 40px 44px auto;
    align-items: center;
    gap: 6px;
    height: 18px;
    flex: none;
    font-size: 11px;
    line-height: 14px;
    color: var(--fg-2);
    white-space: nowrap;
  }
  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 600;
  }
  .bar {
    height: 3px;
    border-radius: 2px;
    background: var(--track);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 2px;
    transition:
      width var(--dur) var(--ease),
      background-color var(--dur) ease-out;
  }
  .pct {
    display: inline-flex;
    align-items: center;
    justify-content: flex-end;
    gap: 2px;
    font-weight: 600;
    color: var(--fg);
    font-variant-numeric: tabular-nums;
  }
  .pct.crit,
  .lock {
    color: var(--crit);
  }
  .lock {
    display: inline-flex;
  }
  .reset {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: right;
  }
</style>
