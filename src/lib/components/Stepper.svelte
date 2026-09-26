<script lang="ts">
  import Icon from "./Icon.svelte";

  interface Props {
    value: number;
    min: number;
    max: number;
    step?: number;
    /** Accessible name. */
    label: string;
    suffix?: string;
    onchange: (value: number) => void;
    id?: string;
  }

  let { value, min, max, step = 1, label, suffix = "", onchange, id }: Props = $props();

  const clamp = (v: number) => Math.min(max, Math.max(min, Math.round(v / step) * step));

  function commit(raw: string | number) {
    const n = typeof raw === "number" ? raw : Number.parseFloat(raw);
    const v = Number.isFinite(n) ? clamp(n) : value;
    if (v !== value) onchange(v);
    return v;
  }
</script>

<div class="stepper" role="group" aria-label={label}>
  <button type="button" class="btn" aria-label="Decrease {label}" disabled={value <= min} onclick={() => commit(value - step)}>
    <Icon name="minus" size={12} />
  </button>
  <label class="field">
    <input
      {id}
      type="text"
      inputmode="numeric"
      aria-label={label}
      {value}
      onchange={(e) => (e.currentTarget.value = String(commit(e.currentTarget.value)))}
      onkeydown={(e) => {
        if (e.key === "ArrowUp") {
          e.preventDefault();
          commit(value + step);
        } else if (e.key === "ArrowDown") {
          e.preventDefault();
          commit(value - step);
        }
      }}
    />{#if suffix}<span class="suffix">{suffix}</span>{/if}
  </label>
  <button type="button" class="btn" aria-label="Increase {label}" disabled={value >= max} onclick={() => commit(value + step)}>
    <Icon name="plus" size={12} />
  </button>
</div>

<style>
  .stepper {
    display: inline-flex;
    align-items: center;
    height: 28px;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    flex: none;
  }
  .btn {
    display: grid;
    place-items: center;
    width: 24px;
    height: 28px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--fg-2);
    border-radius: var(--radius-s);
  }
  .btn:hover:not(:disabled) {
    background: var(--fill-hover);
    color: var(--fg);
  }
  .btn:disabled {
    color: var(--fg-3);
  }
  .field {
    display: flex;
    align-items: baseline;
    justify-content: center;
    min-width: 40px;
  }
  input {
    width: 26px;
    padding: 0;
    border: 0;
    background: transparent;
    text-align: right;
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    outline: none;
  }
  .field:focus-within {
    outline: 2px solid var(--focus);
    outline-offset: -1px;
    border-radius: 2px;
  }
  .suffix {
    color: var(--fg-2);
    font-size: 11px;
    margin-left: 1px;
  }
</style>
