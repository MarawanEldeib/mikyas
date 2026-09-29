<script lang="ts">
  import { app } from "../stores.svelte";
  import { DEFAULT_CTX_THRESHOLDS } from "../thresholds";
  import ThresholdList from "./ThresholdList.svelte";
  import Toggle from "./Toggle.svelte";

  const s = $derived(app.settings);
</script>

{#if s}
  <h2 class="section">Context alerts</h2>
  <div class="group">
    <div class="row">
      <span class="label">Alert on high context</span>
      <Toggle label="Alert on high context" checked={s.ctx_alerts} onchange={(v) => app.patch({ ctx_alerts: v })} />
    </div>
    <fieldset class="sub" disabled={!s.ctx_alerts}>
      <legend class="sr">Context alert thresholds</legend>
      <ThresholdList
        list={s.ctx_thresholds}
        defaults={DEFAULT_CTX_THRESHOLDS}
        what="context"
        onchange={(next) => app.patch({ ctx_thresholds: next })}
      />
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
  .group > * + * {
    border-top: 1px solid var(--divider);
  }
  .label {
    min-width: 0;
  }
  /* The thresholds as a native group: `disabled` turns off every control in it at once. */
  .sub {
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }
  .sub:disabled {
    color: var(--fg-2);
  }
  .sub:disabled :global(:is(.stepper, .btn, .icon-btn)) {
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
