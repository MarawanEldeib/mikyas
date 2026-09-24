<script lang="ts">
  interface Props {
    checked: boolean;
    /** Accessible name (usually the row label). */
    label: string;
    onchange: (checked: boolean) => void;
    id?: string;
  }

  let { checked, label, onchange, id }: Props = $props();
</script>

<button
  {id}
  type="button"
  role="switch"
  class="toggle"
  class:on={checked}
  aria-checked={checked}
  aria-label={label}
  onclick={() => onchange(!checked)}
>
  <span class="knob"></span>
</button>

<style>
  .toggle {
    position: relative;
    flex: none;
    width: 40px;
    height: 20px;
    padding: 0;
    border-radius: 10px;
    border: 1px solid var(--fg-2);
    background: var(--fill-control);
    transition:
      background-color 160ms ease-out,
      border-color 160ms ease-out;
  }
  .toggle:hover {
    background: var(--fill-control-hover);
  }
  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--fg-2);
    transition:
      transform 160ms var(--ease),
      background-color 160ms ease-out;
  }
  .on {
    background: var(--accent);
    border-color: var(--accent);
  }
  .on:hover {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
  }
  .on .knob {
    transform: translateX(20px);
    background: var(--on-accent);
  }
</style>
