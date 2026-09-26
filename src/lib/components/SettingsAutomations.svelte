<script lang="ts">
  import { app } from "../stores.svelte";
  import type { TrayNumber } from "../types";
  import Stepper from "./Stepper.svelte";
  import Toggle from "./Toggle.svelte";

  const TRAY_NUMBERS: { value: TrayNumber; label: string }[] = [
    { value: "worst", label: "Highest limit" },
    { value: "five_hour", label: "5-hour" },
    { value: "seven_day", label: "Weekly" },
    { value: "off", label: "Off (dot)" },
  ];

  const uid = $props.id();
  const s = $derived(app.settings);

</script>

{#if s}
  <h2 class="section">Automations</h2>
  <div class="group">
    <div class="row">
      <span class="label">
        Warn before a limit runs out
        <span class="sub">When the current pace hits 100% before the reset</span>
      </span>
      <Toggle label="Warn before a limit runs out" checked={s.pace_alerts} onchange={(v) => app.patch({ pace_alerts: v })} />
    </div>
    <div class="row">
      <span class="label">
        Heads-up before a limit reopens
        <span class="sub">10 min before a 5-hour reset, 1 h before a weekly one</span>
      </span>
      <Toggle label="Heads-up before a limit reopens" checked={s.reset_heads_up} onchange={(v) => app.patch({ reset_heads_up: v })} />
    </div>
    <div class="row">
      <span class="label">
        Weekly recap
        <span class="sub">A summary when the weekly limit resets</span>
      </span>
      <Toggle label="Weekly recap" checked={s.weekly_recap} onchange={(v) => app.patch({ weekly_recap: v })} />
    </div>
    <div class="row">
      <span class="label">Notify when Claude finishes</span>
      <Toggle label="Notify when Claude finishes" checked={s.finished_alerts} onchange={(v) => app.patch({ finished_alerts: v })} />
    </div>
    <fieldset class="nested" disabled={!s.finished_alerts}>
      <legend class="sr">Finished notification length</legend>
      <div class="row">
        <span class="label">For turns longer than</span>
        <Stepper
          label="Minimum turn length in minutes"
          value={s.finished_min_minutes}
          min={1}
          max={60}
          suffix="m"
          onchange={(v) => app.patch({ finished_min_minutes: v })}
        />
      </div>
    </fieldset>
    <div class="row">
      <span class="label">
        Warn if the connection breaks
        <span class="sub">When Claude Code's status line is changed</span>
      </span>
      <Toggle label="Warn if the connection breaks" checked={s.connection_watchdog} onchange={(v) => app.patch({ connection_watchdog: v })} />
    </div>
    <div class="row">
      <span class="label">
        Remember position per display
        <span class="sub">Each monitor setup keeps its own spot</span>
      </span>
      <Toggle label="Remember position per display" checked={s.per_display_position} onchange={(v) => app.patch({ per_display_position: v })} />
    </div>
    <div class="row">
      <label class="label" for="{uid}-tray">Tray icon number</label>
      <select id="{uid}-tray" value={s.tray_number} onchange={(e) => app.patch({ tray_number: e.currentTarget.value as TrayNumber })}>
        {#each TRAY_NUMBERS as t (t.value)}
          <option value={t.value}>{t.label}</option>
        {/each}
      </select>
    </div>
  </div>
{/if}

<style>
  /* Section chrome copied from Settings.svelte (scoped styles do not cross components). */
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
  .sub {
    display: block;
    margin-top: 1px;
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
  /* The stepper as a native group, like SettingsContext.svelte: `disabled` turns it off. */
  .nested {
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }
  .nested:disabled {
    color: var(--fg-2);
  }
  .nested:disabled :global(.stepper) {
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
