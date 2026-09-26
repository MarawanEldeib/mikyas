<script lang="ts" module>
  import { loadDismissed } from "../update";

  // Module state, so a view switch (card ↔ pill) does not bring a dismissed banner back.
  let dismissed = $state<string | null>(loadDismissed());
</script>

<script lang="ts">
  import { api } from "../ipc";
  import { app } from "../stores.svelte";
  import { bannerVisible, saveDismissed } from "../update";
  import Icon from "./Icon.svelte";

  const update = $derived(app.ui.update);
  const visible = $derived(bannerVisible(app.ui, app.settings?.dock ?? "off", dismissed));
  const pill = $derived(app.ui.view === "pill");

  function view(url: string) {
    api.openUrl(url).catch((e: unknown) => console.warn("open_url failed", e));
  }

  function dismiss(version: string) {
    dismissed = version;
    saveDismissed(version);
  }
</script>

<!-- Floats over the card footer's status area (its buttons stay usable) or the pill's lower edge. -->
{#if visible && update}
  <div class="update" class:pill role="status">
    <Icon name="update" size={pill ? 11 : 13} />
    <span class="text" title="Claude Usage Widget {update.version} is available">
      Update <strong>v{update.version}</strong> available
    </span>
    <span class="dot" aria-hidden="true">·</span>
    <button type="button" class="view" onclick={() => view(update.url)}>View</button>
    <button type="button" class="close" aria-label="Dismiss the update notice" title="Dismiss" onclick={() => dismiss(update.version)}>
      <Icon name="close" size={pill ? 9 : 10} />
    </button>
  </div>
{/if}

<style>
  .update {
    position: absolute;
    z-index: 2;
    /* The card's status area: between the side padding and the three footer buttons. */
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
    color: var(--fg);
  }
  strong {
    font-weight: 600;
  }
  .dot {
    flex: none;
    color: var(--fg-2);
  }
  .view {
    flex: none;
    padding: 0 2px;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--accent);
    font-weight: 600;
  }
  .view:hover {
    text-decoration: underline;
  }
  .close {
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
</style>
