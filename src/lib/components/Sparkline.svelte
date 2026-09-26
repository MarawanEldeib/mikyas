<script lang="ts">
  import { fillColor } from "../color";
  import { sparkGeometry, sparkY } from "../sparkline";
  import type { SparkPoint } from "../types";

  interface Props {
    points: SparkPoint[];
    /** Current value; picks the colour band. */
    pct: number;
    width?: number;
    height?: number;
    stale?: boolean;
    /** Accessible description, e.g. "Usage over the last 24 hours". */
    label: string;
    /** Sits under the window controls: fades out while they show. */
    underControls?: boolean;
  }

  let { points, pct, width = 112, height = 40, stale = false, label, underControls = false }: Props = $props();

  const uid = $props.id();
  const opts = $derived({ width, height, padX: 3, padY: 3 });
  const geo = $derived(sparkGeometry(points, opts));
  const y80 = $derived(sparkY(80, opts));
  // A chrome accent recolours the line (--spark); the default keeps the usage band's colour.
  const color = $derived(stale ? "var(--fg-3)" : `var(--spark, ${fillColor(pct)})`);
</script>

<svg
  class="spark"
  {width}
  {height}
  viewBox="0 0 {width} {height}"
  role="img"
  aria-label={label}
  data-under-controls={underControls ? "" : undefined}
>
  <title>{label}</title>
  <defs>
    <linearGradient id="g{uid}" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color={color} stop-opacity="0.32" />
      <stop offset="1" stop-color={color} stop-opacity="0" />
    </linearGradient>
  </defs>
  <line class="limit" x1="0" x2={width} y1={y80} y2={y80} />
  <line class="base" x1="0" x2={width} y1={height - 3} y2={height - 3} />
  {#if geo.area}
    <path d={geo.area} fill="url(#g{uid})" />
    <path class="line" d={geo.line} stroke={color} />
  {/if}
  {#if geo.dot}
    <circle class="dot" cx={geo.dot.x} cy={geo.dot.y} r="2.5" fill={color} />
  {/if}
</svg>

<style>
  .spark {
    display: block;
    flex: none;
    overflow: visible;
  }
  .limit {
    stroke: var(--fg-3);
    stroke-width: 1;
    stroke-dasharray: 2 3;
    opacity: 0.6;
  }
  .base {
    stroke: var(--divider);
    stroke-width: 1;
  }
  .line {
    fill: none;
    stroke-width: 1.5;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .dot {
    stroke: rgb(var(--surface-rgb));
    stroke-width: 1.5;
    paint-order: stroke;
  }
</style>
