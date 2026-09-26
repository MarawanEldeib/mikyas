<script lang="ts">
  import { displayHotkey, readHotkey } from "../hotkey";
  import IconButton from "./IconButton.svelte";

  interface Props {
    value: string;
    /** Registration error reported by Rust (UiState.hotkey_error / toggle_hotkey_error). */
    error: string | null;
    onchange: (accelerator: string) => void;
    label?: string;
    /** Offers a button that removes the shortcut ("" = none); Backspace works either way. */
    clearable?: boolean;
  }

  let { value, error, onchange, label = "Click-through shortcut", clearable = false }: Props = $props();

  const uid = $props.id();
  let recording = $state(false);
  let hint = $state<string | null>(null);
  let held = $state("");
  let field = $state<HTMLButtonElement>();
  /** Focus returned to the field after "Remove" must not start recording. */
  let quietFocus = false;

  function record() {
    recording = true;
    hint = null;
    held = "";
  }

  function clear() {
    onchange("");
    quietFocus = true;
    field?.focus();
  }

  function onkeydown(e: KeyboardEvent) {
    if (!recording) return;
    // Let Tab move focus normally so the field never traps keyboard users.
    if (e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey) return;
    e.preventDefault();
    e.stopPropagation();
    const r = readHotkey(e);
    const el = e.currentTarget as HTMLElement;
    switch (r.type) {
      case "wait":
        held = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Win"].filter(Boolean).join(" + ");
        hint = null;
        break;
      case "cancel":
        el.blur();
        break;
      case "clear":
        onchange("");
        el.blur();
        break;
      case "invalid":
        hint = r.reason;
        held = "";
        break;
      case "set":
        onchange(r.accelerator);
        el.blur();
        break;
    }
  }
</script>

<div class="hotkey">
  <div class="line">
    <span class="label" id="{uid}-l">{label}</span>
    <div class="controls">
      {#if clearable && value && !recording}
        <IconButton icon="close" label="Remove the {label.toLowerCase()}" onclick={clear} />
      {/if}
      <button
        bind:this={field}
        type="button"
        class="field"
        class:recording
        class:none={!value && !recording}
        aria-labelledby="{uid}-l {uid}-v"
        aria-describedby="{uid}-m"
        onfocus={() => {
          if (quietFocus) quietFocus = false;
          else record();
        }}
        onclick={() => {
          if (!recording) record();
        }}
        onblur={() => (recording = false)}
        {onkeydown}
      >
        <span id="{uid}-v">
          {#if recording}
            <span class="rec">{held ? `${held} + …` : "Press keys…"}</span>
          {:else}
            <kbd>{displayHotkey(value)}</kbd>
          {/if}
        </span>
      </button>
    </div>
  </div>
  <p class="msg" id="{uid}-m" class:err={!recording && error} role={!recording && error ? "alert" : undefined}>
    {#if recording}
      {hint ?? "Esc cancels · Backspace removes the shortcut"}
    {:else if error}
      {error}
    {:else if !value}
      No shortcut. Click to record one.
    {:else}
      Click to record a new shortcut.
    {/if}
  </p>
</div>

<style>
  .hotkey {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .line {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .label {
    min-width: 0;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }
  .field {
    min-width: 112px;
    height: 28px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    text-align: center;
  }
  .field:hover {
    background: var(--fill-control-hover);
  }
  .field.recording {
    box-shadow:
      inset 0 0 0 1px var(--stroke-control),
      inset 0 -2px 0 var(--accent);
  }
  kbd {
    font-family: var(--font);
    font-weight: 600;
    font-size: 12px;
  }
  .none kbd {
    font-weight: 400;
    color: var(--fg-2);
  }
  .rec {
    color: var(--fg-2);
  }
  .msg {
    margin: 0;
    font-size: 11px;
    line-height: 14px;
    color: var(--fg-2);
  }
  .msg.err {
    color: var(--crit);
  }
</style>
