<script lang="ts">
  import { app } from "../stores.svelte";
  import type { Settings } from "../types";
  import Stepper from "./Stepper.svelte";
  import Toggle from "./Toggle.svelte";

  const s = $derived(app.settings);
  // Two ascending thresholds; a hand-edited list with fewer entries falls back to the defaults.
  const t = $derived.by(() => {
    const [first = 80, second = 90] = s?.ctx_thresholds ?? [];
    return [first, Math.min(100, Math.max(first + 1, second))] as const;
  });

  function update(patch: Partial<Settings>) {
    void app.updateSettings(patch);
  }
</script>

{#if s}
  <h2 class="section">Context alerts</h2>
  <div class="group">
    <div class="row">
      <span class="label">Alert on high context</span>
      <Toggle label="Alert on high context" checked={s.ctx_alerts} onchange={(v) => update({ ctx_alerts: v })} />
    </div>
    <fieldset class="sub" disabled={!s.ctx_alerts}>
      <legend class="sr">Context alert thresholds</legend>
      <div class="row">
        <span class="label">First alert at</span>
        <Stepper label="First context alert threshold" value={t[0]} min={10} max={t[1] - 1} suffix="%" onchange={(v) => update({ ctx_thresholds: [v, t[1]] })} />
      </div>
      <div class="row">
        <span class="label">Second alert at</span>
        <Stepper label="Second context alert threshold" value={t[1]} min={t[0] + 1} max={100} suffix="%" onchange={(v) => update({ ctx_thresholds: [t[0], v] })} />
      </div>
    </fieldset>
  </div>
{/if}

<style>
  /* Same rules as Settings.svelte (section heading, grouped rows). */
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
  .row + .row,
  .group > * + * {
    border-top: 1px solid var(--divider);
  }
  .label {
    min-width: 0;
  }
  /* The thresholds as a native group: `disabled` turns off both steppers at once. */
  .sub {
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }
  .sub:disabled {
    color: var(--fg-2);
  }
  .sub:disabled :global(.stepper) {
    opacity: 0.5;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
