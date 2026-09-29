<script lang="ts" module>
  // Axis label widths, measured in the widget font at the labels' 10px.
  let measurer: CanvasRenderingContext2D | null | undefined;
  function labelWidth(text: string): number {
    if (measurer === undefined) {
      measurer = document.createElement("canvas").getContext("2d");
      if (measurer) measurer.font = `10px ${getComputedStyle(document.body).fontFamily}`;
    }
    return measurer ? measurer.measureText(text).width : text.length * 6;
  }
</script>

<script lang="ts">
  import { CRIT_AT, WARN_AT, fillColor, textColor } from "../color";
  import { formatClock, formatPct } from "../format";
  import {
    GRID_PCTS,
    chartGeometry,
    placeLabels,
    plotX,
    plotY,
    timeAxis,
    windowSummary,
    type Domain,
    type PlotBox,
    type RangeKey,
  } from "../history";
  import { plural } from "../plural";
  import type { HistoryWindow } from "../types";

  interface Props {
    window: HistoryWindow;
    domain: Domain;
    range: RangeKey;
    /** "5-hour", "7-day". */
    label: string;
    /** Accessible range name, e.g. "last 7 days". */
    rangeName: string;
    /** Live value from the snapshot, if the window is still reported. */
    current: number | null;
    now: number;
    width: number;
    height?: number;
  }

  let { window: w, domain, range, label, rangeName, current, now, width, height = 72 }: Props = $props();

  const uid = $props.id();
  const box = $derived<PlotBox>({ width, height, left: 24, right: 4, top: 6, bottom: 16 });
  const geo = $derived(chartGeometry(w.points, domain, box));
  const axis = $derived(timeAxis(domain, range));
  const labels = $derived(placeLabels(axis.labels, domain, box, labelWidth));
  const resets = $derived(w.resets_ms.filter((t) => t >= domain.from && t <= domain.to));
  const summary = $derived(windowSummary(w, domain));
  const value = $derived(current ?? geo.last?.pct ?? null);
  const base = $derived(plotY(0, box));
  const x = (t: number) => plotX(t, domain, box);
  const aria = $derived(
    [
      `${label} usage, ${rangeName}`,
      summary.peak === null ? "no data" : `peak ${formatPct(summary.peak)}%`,
      plural(resets.length, "reset"),
      value === null ? null : `now ${formatPct(value)}%`,
    ]
      .filter(Boolean)
      .join(", "),
  );
</script>

<section class="chart" aria-label="{label} history">
  <div class="head">
    <span class="name">{label}</span>
    {#if value !== null}
      <span class="now">now <b style:color={textColor(value)}>{formatPct(value)}%</b></span>
    {/if}
  </div>
  <svg {width} {height} viewBox="0 0 {width} {height}" role="img" aria-label={aria}>
    <defs>
      <!-- Usage bands (green < 40, orange < 70, red) along the y axis, like the card. -->
      <linearGradient id="band{uid}" gradientUnits="userSpaceOnUse" x1="0" y1={base} x2="0" y2={plotY(100, box)}>
        <stop offset="0" style:stop-color="var(--ok-fill)" />
        <stop offset={WARN_AT / 100} style:stop-color="var(--ok-fill)" />
        <stop offset={WARN_AT / 100} style:stop-color="var(--warn-fill)" />
        <stop offset={CRIT_AT / 100} style:stop-color="var(--warn-fill)" />
        <stop offset={CRIT_AT / 100} style:stop-color="var(--crit-fill)" />
        <stop offset="1" style:stop-color="var(--crit-fill)" />
      </linearGradient>
    </defs>
    {#each axis.lines as t (t)}
      <line class="day" x1={x(t)} x2={x(t)} y1={box.top} y2={base} />
    {/each}
    {#each GRID_PCTS as p (p)}
      <line class="grid" class:limit={p === 80} x1={box.left} x2={width - box.right} y1={plotY(p, box)} y2={plotY(p, box)} />
      <text class="ylabel" x={box.left - 5} y={plotY(p, box) + 3.5}>{p}</text>
    {/each}
    <line class="base" x1={box.left} x2={width - box.right} y1={base} y2={base} />
    {#if geo.line}
      <path class="area" d={geo.area} fill="url(#band{uid})" />
      <path class="line" d={geo.line} stroke="url(#band{uid})" />
    {:else}
      <text class="nodata" x={(box.left + width - box.right) / 2} y={(box.top + base) / 2 + 4}>No data in this range</text>
    {/if}
    {#each resets as t (t)}
      <line class="reset" x1={x(t)} x2={x(t)} y1={base + 2.5} y2={base + 6.5}>
        <title>Reset {formatClock(t, now)}</title>
      </line>
    {/each}
    {#if geo.last}
      <circle class="dot" cx={geo.last.x} cy={geo.last.y} r="2.5" style:fill={fillColor(geo.last.pct)} />
    {/if}
    {#each labels as l (l.t)}
      <text class="xlabel" x={l.x} y={height - 2}>{l.text}</text>
    {/each}
  </svg>
</section>

<style>
  .chart {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    height: 16px;
  }
  .name {
    font-size: 12px;
    font-weight: 600;
  }
  .now {
    color: var(--fg-2);
    font-size: 11px;
  }
  .now b {
    font-size: 12px;
    font-weight: 600;
  }
  svg {
    display: block;
    overflow: visible;
  }
  text {
    font-size: 10px;
    fill: var(--fg-2);
  }
  .ylabel {
    text-anchor: end;
  }
  .xlabel,
  .nodata {
    text-anchor: middle;
  }
  .nodata {
    font-size: 11px;
  }
  .grid,
  .base,
  .day {
    stroke: var(--divider);
    stroke-width: 1;
  }
  .grid.limit {
    stroke: var(--fg-3);
    stroke-dasharray: 2 3;
    opacity: 0.6;
  }
  .base {
    stroke: var(--fg-3);
    opacity: 0.6;
  }
  .area {
    fill-opacity: 0.2;
  }
  .line {
    fill: none;
    stroke-width: 1.5;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .reset {
    stroke: var(--fg-2);
    stroke-width: 1.5;
    stroke-linecap: round;
  }
  .dot {
    stroke: rgb(var(--surface-rgb));
    stroke-width: 1.5;
    paint-order: stroke;
  }
</style>
