<script lang="ts">
  import Card from "./lib/components/Card.svelte";
  import DockBar from "./lib/components/DockBar.svelte";
  import HistoryView from "./lib/components/HistoryView.svelte";
  import Pill from "./lib/components/Pill.svelte";
  import SessionsView from "./lib/components/SessionsView.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import UpdateBanner from "./lib/components/UpdateBanner.svelte";
  import WindowControls from "./lib/components/WindowControls.svelte";
  import { menuAnchor, nativeMenuAllowed } from "./lib/controls";
  import { api, startDragging } from "./lib/ipc";
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
    // Appearance hooks for CSS (stream B): accent colour, gauge style, UI scale, dock edge.
    root.dataset.accent = app.settings?.accent ?? "auto";
    root.dataset.gauge = app.settings?.gauge_style ?? "ring";
    root.dataset.dock = app.settings?.dock ?? "off";
    root.style.setProperty("--ui-scale", String(app.settings?.ui_scale ?? 1));
  });

  const docked = $derived((app.settings?.dock ?? "off") !== "off" && !app.ui.dock_expanded);

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

  // WebView2's own menu (Back, Reload, Inspect…) makes no sense here: the app's native menu
  // replaces it, except in text fields and on selected text (cut, copy, paste).
  function oncontextmenu(e: MouseEvent) {
    if (nativeMenuAllowed(e.target, window.getSelection())) return;
    e.preventDefault();
    if (app.ui.click_through) return;
    const at = menuAnchor(e as PointerEvent, document.activeElement instanceof Element ? document.activeElement : null);
    api.showContextMenu(at).catch((err: unknown) => console.warn("show_context_menu failed", err));
  }
</script>

<svelte:window {oncontextmenu} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="widget" class:ghost={app.ui.click_through} style:--widget-opacity={opacity} {onmousedown}>
  <UpdateBanner />
  <!-- First in the DOM so Tab reaches the caption buttons before the view's content. -->
  <!-- Also while loading or after a failed start, so the widget can always be hidden or closed. -->
  <WindowControls />
  {#if !app.ready}
    {#if app.error}
      <div class="fatal" role="alert">
        <p title={app.error}>Couldn't load usage data: {app.error}</p>
        <button type="button" class="retry" onclick={() => void app.init()}>Retry</button>
      </div>
    {/if}
  {:else if docked}
    <DockBar />
  {:else if app.ui.view === "pill"}
    <Pill />
  {:else if app.ui.view === "settings"}
    <Settings />
  {:else if app.ui.view === "sessions"}
    <SessionsView />
  {:else if app.ui.view === "history"}
    <HistoryView />
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
  /* Fits any view's window (down to the 72px pill): the message scrolls, Retry stays visible. */
  .fatal {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 12px;
    color: var(--fg-2);
  }
  .fatal p {
    flex: 0 1 auto;
    min-height: 0;
    overflow-y: auto;
    margin: 0;
    /* Clear of the caption buttons in the top-right corner. */
    padding-right: 72px;
  }
  .retry {
    flex: none;
    align-self: flex-start;
    height: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    font-weight: 500;
  }
  .retry:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
