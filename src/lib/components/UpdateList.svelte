<script lang="ts">
  // Every missed version with its short notes, and the two choices: "Update" opens the latest
  // release page (the user downloads the installer there) and "Later" hides the banner until a
  // newer version appears. Notify-only: nothing is downloaded or installed from here. Notes are
  // plain text and only ever rendered with text interpolation.
  import { api, backend } from "../ipc";
  import { errorText } from "../stores.svelte";
  import type { UpdateInfo } from "../types";
  import { latestUrl, updateRows } from "../update";

  interface Props {
    update: UpdateInfo;
    /** Called after Update or Later (the card's popover closes). */
    ondone?: () => void;
    /** Limits the list height and scrolls (the card popover). */
    compact?: boolean;
  }

  let { update, ondone, compact = false }: Props = $props();

  const rows = $derived(updateRows(update));
  const url = $derived(latestUrl(update));
  let error = $state<string | null>(null);

  function open() {
    if (!url) return;
    error = null;
    api
      .openUrl(url)
      .then(() => ondone?.())
      .catch((e: unknown) => (error = errorText(e)));
  }

  async function later() {
    error = null;
    try {
      await (await backend()).invoke<void>("dismiss_update", { version: update.latest });
      ondone?.();
    } catch (e) {
      error = errorText(e);
    }
  }
</script>

<div class="updates" class:compact data-no-drag>
  <ul class="releases">
    {#each rows as r (r.version)}
      <li>
        <span class="version"
          >v{r.version}{#if r.latest}<span class="tag">latest</span>{/if}</span
        >
        {#if r.notes.length}
          <ul class="notes">
            {#each r.notes as note, i (i)}
              <li>{note}</li>
            {/each}
          </ul>
        {/if}
      </li>
    {/each}
  </ul>
  <div class="actions">
    <button type="button" class="btn primary" disabled={!url} onclick={open} title="Open the v{update.latest} release page to download it"
      >Update</button
    >
    <button type="button" class="btn" disabled={update.dismissed} onclick={later} title="Hide until a newer version appears">Later</button>
    <span class="hint">Opens the release page; nothing installs by itself.</span>
  </div>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>

<style>
  .updates {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  .releases {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  /* The card popover: the list scrolls, the buttons stay in view. */
  .compact {
    flex: 1 1 auto;
    min-height: 0;
  }
  .compact .releases {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
  }
  .compact .actions {
    flex: none;
  }
  .version {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    line-height: 16px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .tag {
    padding: 0 5px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
    font-size: 10px;
    line-height: 14px;
    font-weight: 600;
  }
  .notes {
    margin: 1px 0 0;
    padding-left: 14px;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
  .notes li {
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }
  .btn {
    height: 24px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    font-size: 11.5px;
    font-weight: 500;
    flex: none;
  }
  .btn:hover:not(:disabled) {
    background: var(--fill-control-hover);
  }
  .btn:disabled {
    color: var(--fg-2);
  }
  .primary {
    background: var(--accent);
    box-shadow: none;
    color: var(--on-accent);
    font-weight: 600;
  }
  .primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .hint {
    min-width: 0;
    color: var(--fg-2);
    font-size: 10.5px;
    line-height: 14px;
  }
  .error {
    margin: 0;
    color: var(--warn);
    font-size: 11px;
    line-height: 15px;
  }
</style>
