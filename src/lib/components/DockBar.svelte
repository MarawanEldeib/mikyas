<script lang="ts">
  import { clampPct } from "../color";
  import { startDock } from "../dock";
  import { formatPct, liveWindow, windowLabel, windowShort } from "../format";
  import { api } from "../ipc";
  import { notices } from "../notices";
  import { app } from "../stores.svelte";
  import { mainWindows, mutedColor } from "../windows";
  import { WORKED_SINCE_TIP, showWorkedSince } from "../worked";
  import Icon from "./Icon.svelte";

  // The collapsed edge-dock strip: vertical meters on a side edge, one line on the top edge.
  // Pointing at it slides the widget out (dock.ts slides it back in after the pointer leaves).
  const dock = startDock(app, api.setDockExpanded);

  interface Meter {
    key: string;
    short: string;
    name: string;
    pct: number | null;
    color: string;
    stale: boolean;
    locked: boolean;
    /** Claude worked after this reading: the real value is higher ("▲"). */
    worked: boolean;
  }

  const vertical = $derived(app.settings?.dock !== "top");
  const snap = $derived(app.snapshot);
  const warn = $derived(notices(snap).length > 0);

  const meters = $derived.by((): Meter[] => {
    const shown = mainWindows(snap?.windows ?? []).map((w) => liveWindow(w, app.now));
    if (shown.length === 0) {
      // No data yet: keep the strip's shape with empty meters.
      return ["five_hour", "seven_day"].map((kind) => ({
        key: kind,
        short: windowShort(kind),
        name: windowLabel(kind),
        pct: null,
        color: "var(--fg-3)",
        stale: true,
        locked: false,
        worked: false,
      }));
    }
    return shown.map((w) => {
      return {
        key: w.kind,
        short: windowShort(w.kind),
        name: windowLabel(w.kind),
        pct: clampPct(w.pct),
        color: mutedColor(w),
        stale: w.stale,
        locked: w.limit_reached,
        worked: showWorkedSince(w),
      };
    });
  });

  const label = $derived(
    [
      ...meters.map((m) =>
        m.pct === null
          ? `${m.name}: no data`
          : `${m.name} ${formatPct(m.pct)}% used${m.locked ? ", limit reached" : ""}${m.stale ? ", stale" : ""}${m.worked ? `, ${WORKED_SINCE_TIP}` : ""}`,
      ),
      warn ? "Warnings" : null,
      "Show details",
    ]
      .filter(Boolean)
      .join(". "),
  );

  // Tells app.css the strip is showing (the browser mock sizes its stage from it).
  $effect(() => {
    const root = document.documentElement;
    root.toggleAttribute("data-dock-collapsed", true);
    return () => root.removeAttribute("data-dock-collapsed");
  });
</script>

<!-- A button: keyboard/touch users can slide it out too, and App's drag handler skips buttons,
     so the strip itself is never dragged. -->
<button
  type="button"
  class="dock"
  class:v={vertical}
  class:h={!vertical}
  aria-label={label}
  onpointerenter={() => dock.hover()}
  onclick={() => dock.expand()}
>
  {#each meters as m, i (m.key)}
    {#if i > 0}<span class="sep" aria-hidden="true"></span>{/if}
    <span class="meter" class:stale={m.stale}>
      <span class="label">{m.short}</span>
      <span class="track"><span class="fill" style:--p="{m.pct ?? 0}%" style:background={m.color}></span></span>
      <span class="pct" class:crit={m.locked}>
        {#if m.locked}<Icon name="lock" size={11} />{:else if m.pct === null}–{:else}{formatPct(m.pct)}<span class="u">%</span
          >{#if m.worked}<span class="worked" title={WORKED_SINCE_TIP}>▲</span>{/if}{/if}
      </span>
    </span>
  {/each}
  {#if warn}<span class="dot" aria-hidden="true"></span>{/if}
</button>

<style>
  .dock {
    position: relative;
    display: flex;
    width: 100%;
    height: 100%;
    margin: 0;
    padding: 0;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--fg);
  }
  .dock:focus-visible {
    outline-offset: -3px;
  }
  .meter {
    display: flex;
    min-width: 0;
    min-height: 0;
  }
  .label {
    font-size: 10px;
    line-height: 12px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--fg-2);
    white-space: nowrap;
  }
  .track {
    position: relative;
    border-radius: 3px;
    background: var(--track);
    overflow: hidden;
  }
  .fill {
    position: absolute;
    left: 0;
    bottom: 0;
    border-radius: inherit;
    transition:
      width var(--dur) var(--ease),
      height var(--dur) var(--ease),
      background-color var(--dur) ease-out;
  }
  .pct {
    display: flex;
    align-items: baseline;
    justify-content: center;
    font-family: var(--font-display);
    font-size: 11px;
    line-height: 14px;
    font-weight: 600;
    letter-spacing: -0.01em;
    white-space: nowrap;
  }
  .u {
    font-size: 8.5px;
    color: var(--fg-2);
    margin-left: 0.5px;
  }
  .pct.crit {
    align-self: center;
    color: var(--crit);
  }
  .worked {
    font-size: 7px;
    font-weight: 400;
    color: var(--fg-2);
    margin-left: 1px;
  }
  /* Stale (or no data): neutral fill, secondary text and a dotted track, like the stale source
     badges. */
  .stale .pct {
    color: var(--fg-2);
  }
  .stale .track {
    background: transparent;
    outline: 1px dotted var(--fg-3);
    outline-offset: -1px;
  }
  .dot {
    position: absolute;
    top: 5px;
    right: 5px;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--warn-fill);
    box-shadow: 0 0 0 1.5px rgb(var(--surface-rgb) / 0.9);
  }

  /* Left/right edge, 36×156: two stacked vertical meters. */
  .v {
    flex-direction: column;
    align-items: stretch;
    gap: 7px;
    padding: 10px 0 8px;
  }
  .v .meter {
    flex: 1;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }
  .v .track {
    flex: 1;
    width: 6px;
  }
  .v .fill {
    right: 0;
    height: var(--p);
  }
  .v .sep {
    flex: none;
    height: 1px;
    margin: 0 9px;
    background: var(--divider);
  }

  /* Top edge, 232×34: "5h 22%" over a thin bar, twice. */
  .h {
    align-items: center;
    gap: 11px;
    padding: 0 14px;
  }
  .h .meter {
    flex: 1;
    display: grid;
    grid-template-columns: auto auto;
    justify-content: space-between;
    align-items: baseline;
    row-gap: 3px;
  }
  .h .label {
    font-size: 11px;
    line-height: 14px;
  }
  .h .pct {
    grid-area: 1 / 2;
    font-size: 12px;
  }
  .h .u {
    font-size: 9.5px;
  }
  .h .track {
    grid-area: 2 / 1 / 3 / 3;
    height: 3px;
    border-radius: 1.5px;
  }
  .h .fill {
    top: 0;
    width: var(--p);
  }
  .h .sep {
    flex: none;
    width: 1px;
    height: 18px;
    background: var(--divider);
  }
</style>
