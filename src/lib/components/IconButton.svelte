<script lang="ts">
  import Icon, { type IconName } from "./Icon.svelte";

  interface Props {
    icon: IconName;
    /** Accessible name; also shown as the tooltip. */
    label: string;
    onclick: () => void;
    /** Toggle state (sets aria-pressed and fills the icon). */
    pressed?: boolean;
    size?: "s" | "m";
  }

  let { icon, label, onclick, pressed, size = "s" }: Props = $props();
</script>

<button type="button" class="icon-btn {size}" class:on={pressed} aria-label={label} aria-pressed={pressed} title={label} {onclick}>
  <Icon name={icon} size={size === "s" ? 14 : 16} filled={pressed} />
</button>

<style>
  .icon-btn {
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--fg-2);
    transition:
      background-color 120ms ease-out,
      color 120ms ease-out;
  }
  .s {
    width: 24px;
    height: 24px;
  }
  .m {
    width: 32px;
    height: 32px;
  }
  .icon-btn:hover {
    background: var(--fill-hover);
    color: var(--fg);
  }
  .icon-btn:active {
    background: var(--fill-press);
    color: var(--fg-2);
  }
  .on {
    color: var(--fg);
  }
</style>
