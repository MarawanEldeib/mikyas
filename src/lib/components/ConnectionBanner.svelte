<script lang="ts">
  import { reconnect, type ReconnectState } from "../connection";
  import { api } from "../ipc";
  import Icon from "./Icon.svelte";

  let status = $state<ReconnectState>({ state: "idle" });
  const busy = $derived(status.state === "busy");
  const title = $derived(
    status.state === "error"
      ? `Reconnect failed: ${status.message}`
      : "Claude Code's status line was changed, so the widget no longer gets exact limits. Reconnect wraps it again.",
  );

  async function onreconnect() {
    status = { state: "busy" };
    status = await reconnect(api);
  }

  function ondismiss() {
    api.dismissConnectionWarning().catch((e: unknown) => console.warn("dismiss_connection_warning failed", e));
  }
</script>

<!-- The card footer's status slot: the loss matters more than the health chips it replaces. -->
<div class="lost" class:error={status.state === "error"} role="status" {title}>
  <Icon name="warning" size={12} />
  <span class="text">{status.state === "error" ? "Reconnect failed" : "Status line changed"}</span>
  <button type="button" class="reconnect" disabled={busy} onclick={onreconnect}>{busy ? "Reconnecting…" : "Reconnect"}</button>
  <button type="button" class="close" aria-label="Dismiss the status line warning" title="Dismiss" disabled={busy} onclick={ondismiss}>
    <Icon name="close" size={10} />
  </button>
</div>

<style>
  .lost {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 22px;
    max-width: 100%;
    min-width: 0;
    padding: 0 2px 0 6px;
    border: 1px solid color-mix(in srgb, var(--warn-fill) 45%, transparent);
    border-radius: 11px;
    background: var(--warn-bg);
    color: var(--warn);
    font-size: 11px;
    line-height: 14px;
    white-space: nowrap;
  }
  .error {
    border-color: color-mix(in srgb, var(--crit-fill) 45%, transparent);
    background: var(--crit-bg);
    color: var(--crit);
  }
  .text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg);
    font-weight: 600;
  }
  .reconnect {
    flex: none;
    padding: 0 2px;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--accent);
    font-weight: 600;
  }
  .reconnect:hover:not(:disabled) {
    text-decoration: underline;
  }
  .reconnect:disabled {
    color: var(--fg-2);
  }
  .close {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
    width: 18px;
    height: 18px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--fg-2);
    transition: background-color 120ms ease-out;
  }
  /* A 24px hit area around the drawn circle. */
  .close::before {
    content: "";
    position: absolute;
    inset: -3px;
  }
  .close:hover:not(:disabled) {
    background: var(--fill-hover);
    color: var(--fg);
  }
</style>
