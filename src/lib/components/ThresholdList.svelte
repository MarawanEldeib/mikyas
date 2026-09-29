<script lang="ts">
  import { addThreshold, normalizeThresholds, removeThreshold, setThreshold, thresholdBounds } from "../thresholds";
  import IconButton from "./IconButton.svelte";
  import Stepper from "./Stepper.svelte";

  // The saved alert thresholds as they are: one row per value (any number of them), each
  // removable, plus "Add alert". An empty list means no alerts.
  interface Props {
    list: readonly number[];
    defaults: readonly number[];
    /** What an alert is about, for the accessible names ("usage", "context"). */
    what: string;
    onchange: (next: number[]) => void;
  }

  let { list, defaults, what, onchange }: Props = $props();

  const values = $derived(normalizeThresholds(list));
  const added = $derived(addThreshold(values, defaults));
  const ordinal = (i: number) => (values.length === 1 ? "Alert" : `Alert ${i + 1}`);
</script>

{#each values as v, i (i)}
  {@const [min, max] = thresholdBounds(values, i)}
  <div class="row">
    <span class="label">{ordinal(i)} at</span>
    <div class="inline">
      <Stepper
        label="{ordinal(i)} {what} threshold"
        value={v}
        {min}
        {max}
        suffix="%"
        onchange={(n) => onchange(setThreshold(values, i, n))}
      />
      <IconButton icon="close" label="Remove {what} alert at {v}%" onclick={() => onchange(removeThreshold(values, i))} />
    </div>
  </div>
{/each}
<div class="row">
  <span class="label hint">{values.length ? "" : "No alerts"}</span>
  <button type="button" class="btn" disabled={added === null} onclick={() => added && onchange(added)}>Add alert</button>
</div>

<style>
  /* Same rules as Settings.svelte (grouped rows). */
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 40px;
    margin: 0;
    padding: 6px 12px;
  }
  .row + .row {
    border-top: 1px solid var(--divider);
  }
  .label {
    min-width: 0;
  }
  .hint {
    color: var(--fg-2);
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    height: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    font-weight: 500;
    flex: none;
  }
  .btn:hover:not(:disabled) {
    background: var(--fill-control-hover);
  }
  .btn:disabled {
    opacity: 0.5;
  }
</style>
