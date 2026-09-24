<script lang="ts">
  import { api } from "../ipc";
  import { app, errorText } from "../stores.svelte";
  import type { ConnectPreview, ShellKind, WrapMode } from "../types";
  import Icon from "./Icon.svelte";

  type Phase = "idle" | "loading" | "preview" | "connecting" | "done" | "disconnecting";

  let phase = $state<Phase>("idle");
  let preview = $state.raw<ConnectPreview | null>(null);
  let result = $state.raw<ConnectPreview | null>(null);
  let error = $state<string | null>(null);

  const conn = $derived(app.connection);

  const SHELL: Record<ShellKind, string> = {
    bash: "Bash",
    cmd: "Command Prompt",
    pwsh: "PowerShell 7",
    legacy_power_shell: "Windows PowerShell",
  };
  const MODE: Record<WrapMode, string> = {
    pipe: "stdin pipe",
    pipe_grouped: "grouped pipe",
    argv: "argument wrapper",
    default: "default",
  };

  async function startConnect() {
    error = null;
    result = null;
    phase = "loading";
    try {
      preview = await api.connectClaudeCode(true);
      phase = "preview";
    } catch (e) {
      error = errorText(e);
      phase = "idle";
    }
  }

  async function confirm() {
    phase = "connecting";
    try {
      result = await api.connectClaudeCode(false);
      phase = "done";
    } catch (e) {
      error = errorText(e);
      phase = "preview";
    }
    await app.refreshConnection();
  }

  async function disconnect() {
    error = null;
    result = null;
    phase = "disconnecting";
    try {
      app.connection = await api.disconnectClaudeCode();
    } catch (e) {
      error = errorText(e);
    }
    phase = "idle";
  }

  function cancel() {
    preview = null;
    phase = "idle";
  }
</script>

<div class="panel">
  <div class="status-row">
    {#if conn?.state === "connected"}
      <span class="state ok"><span class="dot"></span>Connected</span>
      <span class="sub" title="Capture mode: {MODE[conn.mode]}">{conn.original ? "wraps your statusline" : "statusline capture on"}</span>
    {:else if conn?.state === "foreign"}
      <span class="state"><span class="dot hollow"></span>Not connected</span>
      <span class="sub">another statusline is set</span>
    {:else if conn?.state === "error"}
      <span class="state crit"><Icon name="warning" size={12} />Can't read settings</span>
    {:else}
      <span class="state"><span class="dot hollow"></span>Not connected</span>
    {/if}
  </div>

  {#if conn?.state === "foreign" && conn.command && phase === "idle"}
    <code class="cmd">{conn.command}</code>
    <p class="note">Connecting wraps it: your statusline keeps working and the widget reads the same data.</p>
  {:else if conn?.state === "error"}
    <p class="note crit-text">{conn.message}</p>
  {:else if conn?.state === "not_configured" && phase === "idle"}
    <p class="note">Adds a tiny capture step to Claude Code's statusLine so the widget gets exact limits and reset times.</p>
  {/if}

  {#if phase === "preview" || phase === "connecting"}
    {#if preview}
      <div class="preview" aria-live="polite">
        <p class="preview-title">Change to <code>~/.claude/settings.json</code> · {SHELL[preview.shell]}</p>
        <span class="tag">Before</span>
        <pre>{preview.before ?? "(no statusLine)"}</pre>
        <span class="tag">After</span>
        <pre class="after">{preview.after}</pre>
        {#each preview.warnings as w, i (i)}
          <p class="warn-line"><Icon name="warning" size={12} /><span>{w}</span></p>
        {/each}
      </div>
    {/if}
    <div class="buttons">
      <button type="button" class="btn" onclick={cancel} disabled={phase === "connecting"}>Cancel</button>
      <button type="button" class="btn primary" onclick={confirm} disabled={phase === "connecting"}>
        {phase === "connecting" ? "Connecting…" : "Confirm"}
      </button>
    </div>
  {:else}
    {#if phase === "done" && result}
      <p class="result" class:bad={result.selftest_ok === false} aria-live="polite">
        <Icon name={result.selftest_ok === false ? "warning" : "check"} size={13} />
        <span>
          {#if result.selftest_ok === true}
            Self-test passed — your statusline output is unchanged.
          {:else if result.selftest_ok === false}
            Connected, but the self-test output differed. Check your statusline or disconnect.
          {:else}
            Connected.
          {/if}
        </span>
      </p>
    {/if}
    <div class="buttons">
      {#if conn?.state === "connected"}
        <button type="button" class="btn" onclick={disconnect} disabled={phase === "disconnecting"}>
          {phase === "disconnecting" ? "Disconnecting…" : "Disconnect"}
        </button>
      {:else}
        <button type="button" class="btn primary" onclick={startConnect} disabled={phase === "loading"}>
          <Icon name="link" size={13} />{phase === "loading" ? "Preparing…" : conn?.state === "error" ? "Try again" : "Connect Claude Code"}
        </button>
      {/if}
    </div>
  {/if}

  {#if error}
    <p class="note crit-text" role="alert">{error}</p>
  {/if}
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px 12px;
  }
  .status-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
  }
  .state {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .state.ok {
    color: var(--fg);
  }
  .state.crit {
    color: var(--crit);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ok-fill);
  }
  .dot.hollow {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--fg-2);
  }
  .sub {
    color: var(--fg-2);
    font-size: 11px;
  }
  .note {
    margin: 0;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
  .crit-text {
    color: var(--crit);
  }
  code,
  pre {
    font-family: var(--font-mono);
    /* Cascadia's regular weight reads heavy at 11px next to Segoe UI. */
    font-weight: 350;
    font-size: 11px;
    line-height: 15px;
  }
  .cmd {
    display: block;
    padding: 6px 8px;
    border-radius: var(--radius-s);
    background: var(--code-bg);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .preview {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .preview-title {
    margin: 0 0 2px;
    font-size: 11px;
    color: var(--fg-2);
  }
  .preview-title code {
    font-size: 10.5px;
  }
  .tag {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-2);
    margin-top: 2px;
  }
  pre {
    margin: 0;
    padding: 6px 8px;
    border-radius: var(--radius-s);
    background: var(--code-bg);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  pre.after {
    box-shadow: inset 2px 0 0 var(--ok-fill);
  }
  .warn-line {
    display: flex;
    gap: 6px;
    margin: 2px 0 0;
    color: var(--warn);
    font-size: 11px;
    line-height: 15px;
  }
  .warn-line :global(.icon) {
    margin-top: 1px;
  }
  .warn-line span {
    overflow-wrap: anywhere;
  }
  .result {
    display: flex;
    gap: 6px;
    margin: 0;
    color: var(--ok);
    font-size: 11px;
    line-height: 15px;
  }
  .result :global(.icon) {
    margin-top: 1px;
  }
  .result.bad {
    color: var(--warn);
  }
  .buttons {
    display: flex;
    gap: 8px;
    justify-content: flex-start;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    border-radius: var(--radius-s);
    border: 0;
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    font-weight: 500;
  }
  .btn:hover:not(:disabled) {
    background: var(--fill-control-hover);
  }
  .btn.primary {
    background: var(--accent);
    color: var(--on-accent);
    box-shadow: none;
    font-weight: 600;
  }
  .btn.primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .btn:disabled {
    opacity: 0.6;
  }
</style>
