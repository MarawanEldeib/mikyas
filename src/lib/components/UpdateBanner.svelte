<script lang="ts" module>
  // Module state, so the list stays open across the pill → card switch that opens it.
  let listOpen = $state(false);
</script>

<script lang="ts">
  import { tick } from "svelte";
  import { backend } from "../ipc";
  import { notices } from "../notices";
  import { app } from "../stores.svelte";
  import { bannerText, bannerVisible } from "../update";
  import Icon from "./Icon.svelte";
  import UpdateList from "./UpdateList.svelte";

  const update = $derived(app.ui.update);
  // The status-line banner takes the same footer slot on the card.
  const warnings = $derived(notices(app.snapshot).length > 0 || app.ui.connection_lost);
  const visible = $derived(bannerVisible(app.ui, app.settings?.dock ?? "off", warnings));
  const pill = $derived(app.ui.view === "pill");
  // The list floats over the card (the pill is too small): it closes with the card, in ghost
  // mode and once the updates are gone or put off.
  const listShown = $derived(
    listOpen && update !== null && !update.dismissed && app.ui.view === "card" && !app.ui.click_through,
  );

  async function openList() {
    listOpen = !listOpen || pill;
    if (pill) await app.setView("card");
    if (!listOpen) return;
    await tick();
    document.querySelector<HTMLElement>(".update-list button")?.focus();
  }

  async function focusHome() {
    // The banner is gone; keep keyboard focus in the widget (the card's first toolbar button,
    // the pill's "Show details") rather than dropping it on <body>.
    await tick();
    const home = document.querySelector<HTMLElement>("[data-focus-home]");
    (home?.matches("button") ? home : home?.querySelector<HTMLElement>("button"))?.focus();
  }

  // The × is "Later": hidden until a newer version appears (Rust remembers it).
  async function later(e: MouseEvent, version: string) {
    const keyboard = (e.currentTarget as HTMLElement).matches(":focus-visible");
    listOpen = false;
    try {
      await (await backend()).invoke<void>("dismiss_update", { version });
    } catch (err) {
      console.warn("dismiss_update failed", err);
    }
    if (keyboard) await focusHome();
  }

  function closeList() {
    listOpen = false;
    void focusHome();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && listShown) {
      e.stopPropagation();
      closeList();
    }
  }
</script>

<svelte:window {onkeydown} />

<!-- Floats over the card footer's status area (its buttons stay usable) or the pill's lower edge.
     Clicking it opens the list of missed versions over the card. -->
{#if visible && update}
  <div class="update" class:pill role="status">
    <Icon name="update" size={pill ? 11 : 13} />
    <button
      type="button"
      class="text"
      aria-expanded={pill ? undefined : listShown}
      title="Show what's new (nothing installs by itself)"
      onclick={openList}
    >
      {bannerText(update, pill)}
    </button>
    <button type="button" class="close" aria-label="Later: hide until a newer version" title="Later" onclick={(e) => later(e, update.latest)}>
      <Icon name="close" size={pill ? 9 : 10} />
    </button>
  </div>
{/if}

{#if listShown && update}
  <div class="update-list" role="dialog" aria-label="Updates available" data-no-drag>
    <div class="list-head">
      <strong>{update.count > 1 ? `${update.count} updates available` : "Update available"}</strong>
      <button type="button" class="close" aria-label="Close the update list" title="Close" onclick={closeList}>
        <Icon name="close" size={10} />
      </button>
    </div>
    <UpdateList {update} compact ondone={closeList} />
  </div>
{/if}

<style>
  .update {
    position: absolute;
    z-index: 2;
    /* The card's status area: between the side padding and the three footer buttons (3 × 24px,
       2px apart, 8px in from the edge, then the footer's 8px gap). */
    left: 12px;
    right: 92px;
    bottom: 8px;
    height: 24px;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 3px 0 8px;
    border-radius: 12px;
    /* Opaque, so the source badges underneath never show through. */
    background: color-mix(in srgb, var(--accent) 16%, rgb(var(--surface-rgb)));
    box-shadow:
      inset 0 0 0 1px color-mix(in srgb, var(--accent) 42%, transparent),
      0 2px 8px rgb(0 0 0 / 0.18);
    color: var(--accent);
    font-size: 11px;
    line-height: 14px;
    white-space: nowrap;
  }
  .text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    padding: 0 2px;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-weight: 600;
    white-space: nowrap;
  }
  .text:hover {
    color: var(--accent);
    text-decoration: underline;
  }
  /* The list of missed versions: over the card's body, above the footer. */
  .update-list {
    position: absolute;
    z-index: 3;
    left: 8px;
    right: 8px;
    top: 8px;
    bottom: 38px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px 10px;
    overflow: hidden;
    border-radius: 8px;
    background: rgb(var(--surface-rgb));
    box-shadow:
      inset 0 0 0 1px color-mix(in srgb, var(--accent) 42%, transparent),
      0 4px 16px rgb(0 0 0 / 0.28);
    color: var(--fg);
    font-size: 11px;
  }
  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 12px;
    line-height: 16px;
  }
  .list-head strong {
    font-weight: 600;
  }
  .close {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
    width: 18px;
    height: 18px;
    margin-left: auto;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--fg-2);
    transition: background-color 120ms ease-out;
  }
  /* A larger hit area than the drawn circle: 24px square here, 20px on the pill. */
  .close::before {
    content: "";
    position: absolute;
    inset: -3px;
  }
  .close:hover {
    background: var(--fill-hover);
    color: var(--fg);
  }
  /* Pill: a slim chip in the 18px band below the rings (which end 18px above the bottom). */
  .pill {
    left: 50%;
    right: auto;
    bottom: 2px;
    max-width: calc(100% - 20px);
    height: 14px;
    gap: 3px;
    padding: 0 1px 0 5px;
    border-radius: 7px;
    font-size: 10.5px;
    line-height: 12px;
    transform: translateX(-50%);
  }
  .pill .close {
    width: 12px;
    height: 12px;
    margin-left: 1px;
  }
  /* Only 3px of the widget lie below the circle, so the hit area grows upwards. */
  .pill .close::before {
    inset: -5px -4px -3px;
  }
</style>
