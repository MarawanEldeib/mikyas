<script lang="ts">
  import { api } from "../ipc";
  import { app, errorText } from "../stores.svelte";
  import type { CloseAction, Settings } from "../types";
  import { APP_VERSION, updateStatus, type CheckState } from "../update";
  import HotkeyField from "./HotkeyField.svelte";
  import Icon from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";

  const CLOSE_ACTIONS: { value: CloseAction; label: string }[] = [
    { value: "hide", label: "Hide to tray" },
    { value: "quit", label: "Quit" },
  ];

  const uid = $props.id();
  const s = $derived(app.settings);
  let check = $state<CheckState>({ state: "idle" });
  const status = $derived(updateStatus(check, app.ui.update));

  function update(patch: Partial<Settings>) {
    void app.updateSettings(patch);
  }

  async function checkNow() {
    check = { state: "checking" };
    try {
      await api.checkUpdatesNow();
      check = { state: "done" };
    } catch (e) {
      check = { state: "error", message: errorText(e) };
    }
  }

  function view(url: string) {
    api.openUrl(url).catch((e: unknown) => (check = { state: "error", message: errorText(e) }));
  }
</script>

{#if s}
  <h2 class="section">System</h2>
  <div class="group">
    <div class="row">
      <HotkeyField
        label="Show / hide shortcut"
        value={s.toggle_hotkey}
        error={app.ui.toggle_hotkey_error}
        clearable
        onchange={(v) => update({ toggle_hotkey: v })}
      />
    </div>
    <div class="row">
      <span class="label" id="{uid}-close">
        Close button
        <span class="sub">The × in the corner</span>
      </span>
      <div class="seg" role="radiogroup" aria-labelledby="{uid}-close">
        {#each CLOSE_ACTIONS as c (c.value)}
          <label>
            <input type="radio" name="{uid}-close" value={c.value} checked={s.close_action === c.value} onchange={() => update({ close_action: c.value })} />
            <span>{c.label}</span>
          </label>
        {/each}
      </div>
    </div>
    <div class="row">
      <span class="label">
        Hide while a fullscreen video or app is in front
        <span class="sub">Checks only which window is in front</span>
      </span>
      <Toggle
        label="Hide while a fullscreen video or app is in front"
        checked={s.auto_hide_fullscreen}
        onchange={(v) => update({ auto_hide_fullscreen: v })}
      />
    </div>
    <div class="row">
      <span class="label">
        Check for updates daily
        <span class="sub">Only network use: api.github.com</span>
      </span>
      <Toggle label="Check for updates daily" checked={s.check_updates} onchange={(v) => update({ check_updates: v })} />
    </div>
    <div class="row">
      <span class="label">
        Version <span class="num">{APP_VERSION}</span>
        <span class="sub status {status?.tone ?? ''}" role="status">
          {#if status}
            {#if status.tone === "ok"}<Icon name="check" size={12} />{:else if status.tone === "warn"}<Icon name="warning" size={12} />{:else if status.tone === "info"}<Icon name="update" size={12} />{/if}
            <span class="status-text">{status.text}</span>
            {#if status.url}
              {@const url = status.url}
              <button type="button" class="link" onclick={() => view(url)}>View<Icon name="external" size={11} /></button>
            {/if}
          {/if}
        </span>
      </span>
      <button type="button" class="btn" disabled={check.state === "checking"} onclick={checkNow}>
        {check.state === "checking" ? "Checking…" : "Check now"}
      </button>
    </div>
  </div>
{/if}

<style>
  /* Section chrome copied from Settings.svelte (scoped styles do not cross components). */
  .section {
    margin: 16px 0 6px 2px;
    font-size: 12px;
    line-height: 16px;
    font-weight: 600;
  }
  .group {
    border-radius: 6px;
    background: var(--fill-card);
    box-shadow: inset 0 0 0 1px var(--stroke);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 40px;
    margin: 0;
    padding: 6px 12px;
  }
  .row + .row {
    border-top: 1px solid var(--divider);
  }
  .row > :global(.hotkey) {
    flex: 1;
    padding: 2px 0;
  }
  .label {
    min-width: 0;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 1px;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
  .sub:empty {
    display: none;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .status-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .status :global(.icon) {
    flex: none;
  }
  .status.ok {
    color: var(--ok);
  }
  .status.info {
    color: var(--fg);
  }
  .status.info > :global(.icon) {
    color: var(--accent);
  }
  .status.warn {
    color: var(--warn);
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    flex: none;
    padding: 0 2px;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--accent);
    font-weight: 600;
  }
  .link:hover {
    text-decoration: underline;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    font-weight: 500;
    flex: none;
  }
  .btn:hover:not(:disabled) {
    background: var(--fill-control-hover);
  }
  .btn:disabled {
    color: var(--fg-2);
  }
  /* Segmented control copied from SettingsLayout.svelte: native radios, visually hidden over
     their segment, so arrow keys move the selection. */
  .seg {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    flex: none;
  }
  .seg label {
    position: relative;
    display: block;
  }
  .seg input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
  }
  .seg span {
    display: grid;
    place-items: center;
    height: 24px;
    min-width: 52px;
    padding: 0 10px;
    border-radius: 3px;
    color: var(--fg-2);
    white-space: nowrap;
    transition:
      background-color 120ms ease-out,
      color 120ms ease-out;
  }
  .seg input:hover + span {
    background: var(--fill-hover);
    color: var(--fg);
  }
  .seg input:checked + span {
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 600;
  }
  .seg input:checked:hover + span {
    background: var(--accent-hover);
  }
  .seg input:focus-visible + span {
    outline: 2px solid var(--focus);
    outline-offset: 1px;
  }
</style>
