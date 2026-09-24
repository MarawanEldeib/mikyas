<script lang="ts">
  import type { Notice } from "../notices";
  import Icon from "./Icon.svelte";

  interface Props {
    notices: Notice[];
    /** One-line chip for the card footer; the full list otherwise. */
    compact?: boolean;
    /** Makes the compact chip a button (e.g. to open settings). */
    onopen?: () => void;
  }

  let { notices, compact = false, onopen }: Props = $props();

  const first = $derived(notices[0]);
  const summary = $derived(notices.map((n) => `${n.title}: ${n.detail}`).join("\n\n"));
</script>

{#if notices.length && compact}
  {#if onopen}
    <button type="button" class="chip" title={summary} aria-label="{notices.length} warning{notices.length > 1 ? 's' : ''}: {first.title}. Open settings" onclick={onopen}>
      <Icon name="warning" size={12} /><span class="chip-text">{first.title}</span>{#if notices.length > 1}<span class="more">+{notices.length - 1}</span>{/if}
    </button>
  {:else}
    <span class="chip" title={summary} role="status">
      <Icon name="warning" size={12} /><span class="chip-text">{first.title}</span>{#if notices.length > 1}<span class="more">+{notices.length - 1}</span>{/if}
    </span>
  {/if}
{:else if notices.length}
  <div class="list" role="status">
    {#each notices as n (n.id)}
      <div class="banner">
        <span class="icon"><Icon name="warning" size={14} /></span>
        <div class="body">
          <strong>{n.title}</strong>
          <p>{n.detail}</p>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 20px;
    max-width: 100%;
    min-width: 0;
    padding: 0 8px 0 6px;
    border: 1px solid color-mix(in srgb, var(--warn-fill) 45%, transparent);
    border-radius: 10px;
    background: var(--warn-bg);
    color: var(--warn);
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;
  }
  button.chip:hover {
    background: color-mix(in srgb, var(--warn-fill) 22%, transparent);
  }
  .chip-text {
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .more {
    flex: none;
    font-weight: 400;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .banner {
    display: flex;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--warn-bg);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--warn-fill) 35%, transparent);
  }
  .icon {
    color: var(--warn);
    padding-top: 1px;
  }
  .body {
    min-width: 0;
  }
  strong {
    display: block;
    font-weight: 600;
  }
  p {
    margin: 2px 0 0;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
</style>
