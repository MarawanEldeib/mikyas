<script lang="ts">
  import { ACCENTS, ACCENT_NAMES } from "../color";
  import { CARD_ROWS, UI_SIZES, nearestUiSize } from "../layout";
  import { app } from "../stores.svelte";
  import type { CardRows, DockEdge, GaugeStyle, Settings } from "../types";
  import Toggle from "./Toggle.svelte";

  // Settings → Layout: accent, gauge style, UI size, edge dock, and the card's optional rows.

  const uid = $props.id();
  const s = $derived(app.settings);
  const size = $derived(nearestUiSize(s?.ui_scale ?? 1));

  const GAUGES: { value: GaugeStyle; label: string }[] = [
    { value: "ring", label: "Rings" },
    { value: "bar", label: "Bars" },
  ];
  const DOCKS: { value: DockEdge; label: string }[] = [
    { value: "off", label: "Off" },
    { value: "left", label: "Left edge" },
    { value: "right", label: "Right edge" },
    { value: "top", label: "Top edge" },
  ];

  function update(patch: Partial<Settings>) {
    void app.updateSettings(patch);
  }

  function setRow(rows: CardRows, key: keyof CardRows, on: boolean) {
    update({ card_rows: { ...rows, [key]: on } });
  }
</script>

{#if s}
  <h2 class="section">Layout</h2>
  <div class="group">
    <div class="row col">
      <span class="label spread" id="{uid}-accent">Accent colour <span class="val">{ACCENT_NAMES[s.accent]}</span></span>
      <div class="swatches" role="radiogroup" aria-labelledby="{uid}-accent">
        {#each ACCENTS as a (a)}
          <label class="swatch-hit" title={ACCENT_NAMES[a]}>
            <input type="radio" name="{uid}-accent" value={a} checked={s.accent === a} aria-label={ACCENT_NAMES[a]} onchange={() => update({ accent: a })} />
            <span class="swatch" class:auto={a === "auto"} style:--c={a === "auto" ? undefined : `var(--accent-${a})`}></span>
          </label>
        {/each}
      </div>
    </div>
    <div class="row">
      <span class="label" id="{uid}-gauge">Compact gauges</span>
      <div class="seg" role="radiogroup" aria-labelledby="{uid}-gauge">
        {#each GAUGES as g (g.value)}
          <label>
            <input type="radio" name="{uid}-gauge" value={g.value} checked={s.gauge_style === g.value} onchange={() => update({ gauge_style: g.value })} />
            <span>{g.label}</span>
          </label>
        {/each}
      </div>
    </div>
    <div class="row col">
      <span class="label" id="{uid}-size">Size</span>
      <div class="seg fill" role="radiogroup" aria-labelledby="{uid}-size">
        {#each UI_SIZES as z (z.value)}
          <label>
            <input type="radio" name="{uid}-size" value={z.value} checked={size === z.value} onchange={() => update({ ui_scale: z.value })} />
            <span>{z.label}</span>
          </label>
        {/each}
      </div>
    </div>
    <div class="row">
      <label class="label" for="{uid}-dock">Dock to screen edge</label>
      <select id="{uid}-dock" value={s.dock} onchange={(e) => update({ dock: e.currentTarget.value as DockEdge })}>
        {#each DOCKS as d (d.value)}
          <option value={d.value}>{d.label}</option>
        {/each}
      </select>
    </div>
    {#if s.dock !== "off"}
      <p class="row hint">Tucks into a strip on the edge and slides out when you point at it. Drag it out to move it along the edge.</p>
    {/if}
  </div>

  <h2 class="section">Card rows</h2>
  <div class="group">
    {#each CARD_ROWS as r (r.key)}
      <div class="row">
        <span class="label">{r.label}</span>
        <Toggle label="Show {r.label.toLowerCase()}" checked={s.card_rows[r.key]} onchange={(v) => setRow(s.card_rows, r.key, v)} />
      </div>
    {/each}
  </div>
{/if}

<style>
  /* Row and group styles copied from Settings.svelte (scoped styles don't cross components). */
  .section {
    margin: 16px 0 6px 2px;
    font-size: 12px;
    line-height: 16px;
    font-weight: 600;
  }
  .group {
    border-radius: 6px;
    background: var(--fill-card);
    box-shadow: inset 0 0 0 1px var(--stroke);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 40px;
    margin: 0;
    padding: 6px 12px;
  }
  .group > :global(* + *) {
    border-top: 1px solid var(--divider);
  }
  .row.col {
    flex-direction: column;
    align-items: stretch;
    gap: 6px;
    padding: 8px 12px 10px;
  }
  .label {
    min-width: 0;
  }
  .spread {
    display: flex;
    justify-content: space-between;
  }
  .val {
    color: var(--fg-2);
  }
  .hint {
    display: block;
    min-height: 0;
    padding-top: 8px;
    padding-bottom: 8px;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
  select {
    height: 28px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    flex: none;
  }
  select:hover {
    background: var(--fill-control-hover);
  }
  option {
    background: Canvas;
    color: CanvasText;
  }

  /* Native radios, visually hidden over their swatch or segment: arrow keys move the
     selection and the focus ring is drawn on the visible shape. */
  input[type="radio"] {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
  }

  .swatches {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin: 0 -4px;
  }
  .swatch-hit {
    position: relative;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
  }
  .swatch {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--c);
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.14);
    transition:
      transform 120ms ease-out,
      box-shadow 120ms ease-out;
  }
  /* "Default" keeps the neutral stock chrome: drawn half light, half dark. */
  .swatch.auto {
    background: linear-gradient(135deg, #f3f3f3 50%, #2b2b2b 50%);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
  }
  input:hover + .swatch {
    transform: scale(1.1);
  }
  input:checked + .swatch {
    box-shadow:
      0 0 0 2px rgb(var(--surface-rgb)),
      0 0 0 4px var(--c, var(--fg-2));
  }
  input:focus-visible + .swatch {
    outline: 2px solid var(--focus);
    outline-offset: 5px;
  }

  .seg {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    flex: none;
  }
  .seg.fill {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }
  .seg label {
    position: relative;
    display: block;
  }
  .seg span {
    display: grid;
    place-items: center;
    height: 24px;
    min-width: 52px;
    padding: 0 10px;
    border-radius: 3px;
    color: var(--fg-2);
    white-space: nowrap;
    transition:
      background-color 120ms ease-out,
      color 120ms ease-out;
  }
  .seg input:hover + span {
    background: var(--fill-hover);
    color: var(--fg);
  }
  .seg input:checked + span {
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 600;
  }
  .seg input:checked:hover + span {
    background: var(--accent-hover);
  }
  .seg input:focus-visible + span {
    outline: 2px solid var(--focus);
    outline-offset: 1px;
  }
</style>
