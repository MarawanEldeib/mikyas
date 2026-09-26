<script lang="ts">
  import { controlAction, controlLabel, inControlZone, windowControls, type ControlsState, type WindowControl } from "../controls";
  import { startDock } from "../dock";
  import { api } from "../ipc";
  import { app } from "../stores.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  // Minimize / expand / close in the top-right corner, shown while the pointer is over the header
  // strip (the whole pill) or one of them has keyboard focus. Meanwhile anything marked
  // data-under-controls fades out, so the group never covers a label or a number.

  const ICONS: Record<WindowControl, IconName> = { minimize: "minimize", expand: "expand", close: "close" };

  const view = $derived<ControlsState>({
    view: app.ui.view,
    dock: app.settings?.dock ?? "off",
    dockExpanded: app.ui.dock_expanded,
    clickThrough: app.ui.click_through,
  });
  const controls = $derived(windowControls(view));
  const close = $derived(app.settings?.close_action ?? "hide");

  // "Hot" while the pointer is in the control zone; the attribute on <html> drives the CSS below.
  let hot = $state(false);
  $effect(() => {
    document.documentElement.toggleAttribute("data-controls-hot", hot && controls.length > 0);
  });
  function onpointermove(e: PointerEvent) {
    hot = inControlZone(app.ui.view, e.clientY, app.settings?.ui_scale ?? 1);
  }

  function run(control: WindowControl) {
    const action = controlAction(control, view, close);
    const failed = (e: unknown) => console.warn(`${action.type} failed`, e);
    switch (action.type) {
      case "view":
        void app.setView(action.view);
        break;
      case "dock-collapse":
        startDock(app, api.setDockExpanded).collapse();
        break;
      case "hide":
        api.hideWidget().catch(failed);
        break;
      case "quit":
        api.quitApp().catch(failed);
        break;
    }
  }
</script>

<svelte:document {onpointermove} onpointerleave={() => (hot = false)} />

{#if controls.length}
  <div class="controls" data-window-controls data-view={app.ui.view} role="group" aria-label="Window">
    {#each controls as c (c)}
      {@const label = controlLabel(c, view, close)}
      <button type="button" class:close={c === "close"} aria-label={label} title={label} onclick={() => run(c)}>
        <Icon name={ICONS[c]} size={12} />
      </button>
    {/each}
  </div>
{/if}

<style>
  .controls {
    position: absolute;
    z-index: 3;
    top: 8px;
    right: 8px;
    display: flex;
    gap: 2px;
    padding: 2px;
    border-radius: 6px;
    /* Opaque: whatever lies under the group is faded out, never half covered. */
    background: rgb(var(--surface-rgb));
    box-shadow:
      inset 0 0 0 1px var(--stroke-control),
      0 2px 6px rgb(0 0 0 / 0.14);
    opacity: 0;
    transition: opacity 150ms ease-out;
  }
  :global(html[data-controls-hot]) .controls,
  .controls:focus-within {
    opacity: 1;
  }
  :global([data-under-controls]) {
    transition: opacity 150ms ease-out;
  }
  :global(html[data-controls-hot] [data-under-controls]),
  :global(.widget:has([data-window-controls]:focus-within) [data-under-controls]) {
    opacity: 0;
  }
  /* The 72px pill: tucked into the corner, clear of the right ring's "7d" label. */
  .controls[data-view="pill"] {
    top: 4px;
    right: 4px;
  }
  /* Panels: centred on the 44px header bar, which leaves the group its corner. */
  .controls:is([data-view="settings"], [data-view="sessions"], [data-view="history"]) {
    top: 10px;
    right: 10px;
  }
  button {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--fg-2);
    transition:
      background-color 120ms ease-out,
      color 120ms ease-out;
  }
  button:hover {
    background: var(--fill-hover);
    color: var(--fg);
  }
  button:active {
    background: var(--fill-press);
    color: var(--fg-2);
  }
  /* Windows' caption Close red, in both themes. */
  .close:hover {
    background: #c42b1c;
    color: #ffffff;
  }
  .close:active {
    background: #b42a1c;
    color: rgb(255 255 255 / 0.8);
  }
</style>
