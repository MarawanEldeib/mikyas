<script lang="ts">
  import Card from "./lib/components/Card.svelte";
  import Pill from "./lib/components/Pill.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import { startDragging } from "./lib/ipc";
  import { app } from "./lib/stores.svelte";
  import { createTicker, type Ticker } from "./lib/tick";
  import { onMount, untrack } from "svelte";

  const root = document.documentElement;

  // Browser-only stage options (?bg=dark|light|photo); ignored inside Tauri.
  if (app.mock) {
    root.classList.add("mock");
    root.dataset.bg = new URLSearchParams(location.search).get("bg") ?? "photo";
  }

  let ticker: Ticker | null = null;

  onMount(() => {
    void app.init();
    ticker = createTicker(
      () => app.tickTargets(),
      (now) => (app.now = now),
    );
    return () => {
      ticker?.stop();
      app.dispose();
    };
  });

  // Re-arm the tick whenever a new snapshot changes the set of displayed instants.
  $effect(() => {
    void app.snapshot;
    untrack(() => ticker?.refresh());
  });

  $effect(() => {
    root.dataset.effect = app.settings?.effect ?? "auto";
    root.dataset.view = app.ui.view;
    root.toggleAttribute("data-ghost", app.ui.click_through);
  });

  const opacity = $derived(
    app.settings ? (app.ui.click_through ? app.settings.ghost_opacity : app.settings.opacity) : 1,
  );

  const NO_DRAG = "button, a, input, select, textarea, label, pre, code, [role='switch'], [data-no-drag]";

  // Tauri 2's data-tauri-drag-region does not cover child elements, so dragging is started
  // manually. mousedown (not pointerdown) carries the click count in `detail`: the OS drag
  // loop swallows the second click, so a double-click is recognised here instead of dblclick.
  function onmousedown(e: MouseEvent) {
    if (e.button !== 0 || app.ui.click_through) return;
    if (e.target instanceof Element && e.target.closest(NO_DRAG)) return;
    if (e.detail >= 2) {
      if (app.ui.view === "pill") void app.setView("card");
      return;
    }
    startDragging();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="widget" class:ghost={app.ui.click_through} style:--widget-opacity={opacity} {onmousedown}>
  {#if !app.ready}
    {#if app.error}
      <p class="fatal" role="alert">Couldn't load usage data: {app.error}</p>
    {/if}
  {:else if app.ui.view === "pill"}
    <Pill />
  {:else if app.ui.view === "settings"}
    <Settings />
  {:else}
    <Card />
  {/if}
</div>

<style>
  .widget {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: var(--radius);
    background: rgb(var(--surface-rgb) / var(--surface-a));
    box-shadow:
      inset 0 0 0 1px var(--stroke),
      inset 0 1px 0 0 var(--highlight);
    opacity: var(--widget-opacity);
    transition: opacity 200ms ease-out;
  }
  .ghost,
  .ghost :global(*) {
    pointer-events: none;
  }
  .fatal {
    margin: 0;
    padding: 12px;
    color: var(--fg-2);
  }
</style>
