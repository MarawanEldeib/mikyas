<script lang="ts">
  import { clampPct, fillColor } from "../color";
  import { formatPct } from "../format";
  import Icon from "./Icon.svelte";

  interface Props {
    pct: number;
    size?: number;
    stroke?: number;
    /** Draw the arc in a neutral colour (data is stale). */
    stale?: boolean;
    /** Limit reached: show a lock glyph instead of the number. */
    locked?: boolean;
  }

  let { pct, size = 44, stroke = 4, stale = false, locked = false }: Props = $props();

  const r = $derived((size - stroke) / 2);
  const c = $derived(2 * Math.PI * r);
  const p = $derived(clampPct(pct));
  const offset = $derived(c * (1 - p / 100));
  const color = $derived(stale ? "var(--fg-3)" : fillColor(p));
</script>

<div class="ring" style:width="{size}px" style:height="{size}px">
  <svg width={size} height={size} viewBox="0 0 {size} {size}" aria-hidden="true">
    <circle class="track" cx={size / 2} cy={size / 2} {r} stroke-width={stroke} />
    {#if p >= 0.5}
      <circle
        class="arc"
        cx={size / 2}
        cy={size / 2}
        {r}
        stroke-width={stroke}
        stroke={color}
        stroke-dasharray={c}
        stroke-dashoffset={offset}
        transform="rotate(-90 {size / 2} {size / 2})"
      />
    {/if}
  </svg>
  <div class="center" class:locked style:font-size="{Math.round(size * 0.33 * 2) / 2}px">
    {#if locked}
      <Icon name="lock" size={Math.round(size * 0.34)} />
    {:else}
      <span class="num">{formatPct(p)}</span><span class="unit">%</span>
    {/if}
  </div>
</div>

<style>
  .ring {
    position: relative;
    flex: none;
  }
  svg {
    display: block;
  }
  .track {
    fill: none;
    stroke: var(--track);
  }
  .arc {
    fill: none;
    stroke-linecap: round;
    transition:
      stroke-dashoffset var(--dur) var(--ease),
      stroke var(--dur) ease-out;
  }
  .center {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: var(--font-display);
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .num {
    line-height: 1;
  }
  .unit {
    font-size: 0.66em;
    line-height: 1;
    margin-left: 0.5px;
    margin-top: 1px;
    color: var(--fg-2);
    font-weight: 600;
  }
  .locked {
    color: var(--crit);
  }
</style>
