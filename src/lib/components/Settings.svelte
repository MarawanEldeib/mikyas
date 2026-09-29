<script lang="ts">
  import { api } from "../ipc";
  import { formatAge, formatTokens } from "../format";
  import { notices } from "../notices";
  import { app, errorText } from "../stores.svelte";
  import type { EffectName } from "../types";
  import ConnectPanel from "./ConnectPanel.svelte";
  import HotkeyField from "./HotkeyField.svelte";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";
  import SettingsAutomations from "./SettingsAutomations.svelte";
  import SettingsContext from "./SettingsContext.svelte";
  import SettingsLayout from "./SettingsLayout.svelte";
  import SettingsSystem from "./SettingsSystem.svelte";
  import StatusBanner from "./StatusBanner.svelte";
  import Stepper from "./Stepper.svelte";
  import Toggle from "./Toggle.svelte";

  const s = $derived(app.settings);
  const snap = $derived(app.snapshot);
  const warn = $derived(notices(snap));
  const onboarding = $derived(app.connection?.state !== "connected" && snap?.health.desktop.state === "not_found");

  const EFFECTS: { value: EffectName; label: string }[] = [
    { value: "auto", label: "Automatic" },
    { value: "mica", label: "Mica" },
    { value: "acrylic", label: "Acrylic" },
    { value: "blur", label: "Blur" },
    { value: "none", label: "None (solid)" },
  ];
  const SIZES = [200_000, 1_000_000];

  const t = $derived([s?.thresholds[0] ?? 80, s?.thresholds[1] ?? 95] as const);

  const failed = (e: unknown) => (app.error = errorText(e));

  // ---- context-window overrides
  let newModel = $state("");
  let newSize = $state(1_000_000);
  let modelError = $state<string | null>(null);
  const overrides = $derived(Object.entries(s?.ctx_overrides ?? {}).sort(([a], [b]) => a.localeCompare(b)));

  function addOverride(e: SubmitEvent) {
    e.preventDefault();
    const id = newModel.trim().replace(/\[1m\]$/i, "");
    if (!/^[a-z0-9][a-z0-9._-]*$/i.test(id)) {
      modelError = "Use a model id like claude-opus-5-5";
      return;
    }
    modelError = null;
    app.patch({ ctx_overrides: { ...(s?.ctx_overrides ?? {}), [id]: newSize } });
    newModel = "";
  }

  function setOverride(id: string, size: number) {
    app.patch({ ctx_overrides: { ...(s?.ctx_overrides ?? {}), [id]: size } });
  }

  function removeOverride(id: string) {
    const next = { ...(s?.ctx_overrides ?? {}) };
    delete next[id];
    app.patch({ ctx_overrides: next });
  }

  const desktop = $derived.by(() => {
    const d = snap?.health.desktop;
    switch (d?.state) {
      case "ok":
        return {
          tone: "ok",
          text: d.last_sample_ms === null ? "Found · no samples yet" : `Found · sample ${formatAge(d.last_sample_ms, app.now)}`,
        };
      case "schema_changed":
        return { tone: "warn", text: `Unsupported format (v${d.version})` };
      case "unreadable":
        return { tone: "crit", text: "Found, but can't be read" };
      default:
        return { tone: "off", text: "Not found" };
    }
  });
  const cli = $derived(
    snap?.health.cli_last_capture_ms != null
      ? { tone: "ok", text: `Last capture ${formatAge(snap.health.cli_last_capture_ms, app.now)}` }
      : { tone: "off", text: "No captures yet" },
  );
  const transcripts = $derived(
    snap?.health.transcripts_last_activity_ms != null
      ? { tone: "ok", text: `Last activity ${formatAge(snap.health.transcripts_last_activity_ms, app.now)}` }
      : { tone: "off", text: "None found" },
  );
</script>

<svelte:window
  onkeydown={(e) => {
    // Esc leaves settings, except while typing in a field (the shortcut recorder handles its
    // own Esc and stops propagation).
    const typing = e.target instanceof Element && e.target.closest("input, select, textarea");
    if (e.key === "Escape" && !e.defaultPrevented && !typing) void app.back();
  }}
/>

<div class="settings">
  <header class="bar">
    <IconButton icon="back" label="Back" size="m" onclick={() => app.back()} />
    <h1>Settings</h1>
  </header>

  <div class="scroll" data-no-drag>
    {#if app.error}
      <p class="error" role="alert"><Icon name="warning" size={13} /><span>{app.error}</span></p>
    {/if}
    {#if warn.length}
      <div class="block"><StatusBanner notices={warn} /></div>
    {/if}

    {#if onboarding}
      <section class="welcome" aria-labelledby="welcome-h">
        <h2 id="welcome-h">Welcome — connect a data source</h2>
        <ol>
          <li><strong>Claude Code:</strong> connect below for exact limits and reset times.</li>
          <li><strong>Claude Desktop:</strong> open it once; its usage history is picked up automatically.</li>
        </ol>
      </section>
    {/if}

    <h2 class="section">Claude Code</h2>
    <div class="group"><ConnectPanel /></div>

    {#if s}
      <h2 class="section">Alerts</h2>
      <div class="group">
        <div class="row">
          <span class="label">First alert at</span>
          <Stepper
            label="First alert threshold"
            value={t[0]}
            min={10}
            max={t[1] - 1}
            suffix="%"
            onchange={(v) => app.patch({ thresholds: [v, t[1]] })}
          />
        </div>
        <div class="row">
          <span class="label">Second alert at</span>
          <Stepper
            label="Second alert threshold"
            value={t[1]}
            min={t[0] + 1}
            max={100}
            suffix="%"
            onchange={(v) => app.patch({ thresholds: [t[0], v] })}
          />
        </div>
        <div class="row">
          <span class="label">Notify when a limit resets</span>
          <Toggle label="Notify when a limit resets" checked={s.notify_reset} onchange={(v) => app.patch({ notify_reset: v })} />
        </div>
      </div>

      <SettingsContext />

      <SettingsAutomations />

      <h2 class="section">Appearance</h2>
      <div class="group">
        <div class="row col">
          <label class="label spread" for="opacity">Opacity <span class="val">{Math.round(s.opacity * 100)}%</span></label>
          <input
            id="opacity"
            type="range"
            min="30"
            max="100"
            step="5"
            value={Math.round(s.opacity * 100)}
            style:--p="{((s.opacity * 100 - 30) / 70) * 100}%"
            oninput={(e) => app.previewSettings({ opacity: Number(e.currentTarget.value) / 100 })}
            onchange={(e) => app.patch({ opacity: Number(e.currentTarget.value) / 100 })}
          />
        </div>
        <div class="row col">
          <label class="label spread" for="ghost">Click-through opacity <span class="val">{Math.round(s.ghost_opacity * 100)}%</span></label
          >
          <input
            id="ghost"
            type="range"
            min="15"
            max="100"
            step="5"
            value={Math.round(s.ghost_opacity * 100)}
            style:--p="{((s.ghost_opacity * 100 - 15) / 85) * 100}%"
            oninput={(e) => app.previewSettings({ ghost_opacity: Number(e.currentTarget.value) / 100 })}
            onchange={(e) => app.patch({ ghost_opacity: Number(e.currentTarget.value) / 100 })}
          />
        </div>
        <div class="row">
          <label class="label" for="effect">Backdrop</label>
          <select id="effect" value={s.effect} onchange={(e) => app.patch({ effect: e.currentTarget.value as EffectName })}>
            {#each EFFECTS as e (e.value)}
              <option value={e.value}>{e.label}</option>
            {/each}
          </select>
        </div>
        <div class="row">
          <span class="label">Show project name</span>
          <Toggle label="Show project name" checked={s.show_project} onchange={(v) => app.patch({ show_project: v })} />
        </div>
      </div>

      <SettingsLayout />

      <h2 class="section">Behaviour</h2>
      <div class="group">
        <div class="row">
          <HotkeyField value={s.hotkey} error={app.ui.hotkey_error} onchange={(v) => app.patch({ hotkey: v })} />
        </div>
        <div class="row">
          <span class="label">Start with Windows</span>
          <Toggle label="Start with Windows" checked={s.start_with_windows} onchange={(v) => app.patch({ start_with_windows: v })} />
        </div>
        <div class="row">
          <span class="label">Mark data stale after</span>
          <Stepper
            label="Minutes until data is stale"
            value={s.stale_min}
            min={1}
            max={240}
            suffix="m"
            onchange={(v) => app.patch({ stale_min: v })}
          />
        </div>
      </div>

      <SettingsSystem />

      <h2 class="section">Context window sizes</h2>
      <div class="group">
        <p class="row hint">Override the context size used for “ctx %” when a model's window can't be detected.</p>
        {#each overrides as [id, size] (id)}
          <div class="row">
            <code class="model" title={id}>{id}</code>
            <div class="inline">
              <select aria-label="Context size for {id}" value={size} onchange={(e) => setOverride(id, Number(e.currentTarget.value))}>
                {#each SIZES as n (n)}<option value={n}>{formatTokens(n)}</option>{/each}
                {#if !SIZES.includes(size)}<option value={size}>{formatTokens(size)}</option>{/if}
              </select>
              <IconButton icon="close" label="Remove override for {id}" onclick={() => removeOverride(id)} />
            </div>
          </div>
        {/each}
        <form class="row add" onsubmit={addOverride}>
          <input
            class="text"
            type="text"
            placeholder="Model id"
            aria-label="Model id"
            spellcheck="false"
            autocomplete="off"
            bind:value={newModel}
            aria-invalid={modelError ? "true" : undefined}
          />
          <select aria-label="Context size" bind:value={newSize}>
            {#each SIZES as n (n)}<option value={n}>{formatTokens(n)}</option>{/each}
          </select>
          <button type="submit" class="btn">Add</button>
        </form>
        {#if modelError}<p class="row hint crit-text" role="alert">{modelError}</p>{/if}
      </div>
    {/if}

    <h2 class="section">Data sources</h2>
    <div class="group">
      <div class="row">
        <span class="label">Claude Desktop</span>
        <span class="health {desktop.tone}"><span class="dot"></span>{desktop.text}</span>
      </div>
      <div class="row">
        <span class="label">Claude Code</span>
        <span class="health {cli.tone}"><span class="dot"></span>{cli.text}</span>
      </div>
      <div class="row">
        <span class="label">Transcripts</span>
        <span class="health {transcripts.tone}"><span class="dot"></span>{transcripts.text}</span>
      </div>
    </div>

    <div class="privacy">
      <Icon name="shield" size={16} />
      <div>
        <p>
          <strong>Reads only</strong> Claude Code's statusline output, local session transcripts (model and context size) and Claude
          Desktop's usage history. <strong>Never reads your Claude login.</strong> No network access except update checks you turn on or run (api.github.com).
        </p>
        <button type="button" class="btn" onclick={() => api.openDataFolder().catch(failed)}
          ><Icon name="folder" size={13} />Open data folder</button
        >
      </div>
    </div>

    <button type="button" class="btn quit" onclick={() => api.quitApp().catch(failed)}><Icon name="power" size={13} />Quit Mikyas</button>
    <p class="credits">Built by Eng. Marawan Eldeib</p>
    <p class="legal">
      Independent project, not affiliated with or endorsed by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC.
    </p>
    <button type="button" class="btn notices" onclick={() => api.openThirdPartyNotices().catch(failed)}
      ><Icon name="external" size={12} />Third-party licenses</button
    >
  </div>
</div>

<style>
  .settings {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 44px;
    padding: 0 12px 0 6px;
    flex: none;
    border-bottom: 1px solid var(--divider);
  }
  h1 {
    margin: 0;
    font-size: 14px;
    line-height: 20px;
    font-weight: 600;
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    /* The 10px scrollbar gutter completes the 12px right inset. */
    padding: 4px 2px 16px 12px;
    scrollbar-gutter: stable;
  }
  .block {
    margin-top: 8px;
  }
  .error {
    display: flex;
    gap: 6px;
    margin: 8px 0 0;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--crit-bg);
    color: var(--crit);
    font-size: 11px;
    line-height: 15px;
  }
  .error :global(.icon) {
    flex: none;
    margin-top: 1px;
  }
  .error span {
    overflow-wrap: anywhere;
  }
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
  .row + .row,
  .group > :global(* + *) {
    border-top: 1px solid var(--divider);
  }
  .row.col {
    flex-direction: column;
    align-items: stretch;
    gap: 6px;
    padding: 8px 12px 10px;
  }
  .row > :global(.hotkey) {
    flex: 1;
    padding: 2px 0;
  }
  .label {
    min-width: 0;
  }
  .spread {
    display: flex;
    justify-content: space-between;
  }
  .val {
    color: var(--fg-2);
    font-variant-numeric: tabular-nums;
  }
  .hint {
    display: block;
    min-height: 0;
    padding-top: 8px;
    padding-bottom: 8px;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
  .crit-text {
    color: var(--crit);
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }
  .model {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
    font-weight: 350;
    font-size: 11px;
  }
  .add {
    justify-content: flex-start;
  }
  .text {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow:
      inset 0 0 0 1px var(--stroke-control),
      inset 0 -1px 0 var(--fg-3);
    font-family: var(--font-mono);
    font-weight: 350;
    font-size: 11px;
    outline: none;
  }
  .text:focus {
    box-shadow:
      inset 0 0 0 1px var(--stroke-control),
      inset 0 -2px 0 var(--accent);
  }
  .text::placeholder {
    color: var(--fg-2);
  }
  select {
    height: 28px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke-control);
    flex: none;
  }
  select:hover {
    background: var(--fill-control-hover);
  }
  option {
    background: Canvas;
    color: CanvasText;
  }
  input[type="range"] {
    width: 100%;
    height: 20px;
    margin: 0;
    background: transparent;
    appearance: none;
  }
  input[type="range"]::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: 2px;
    background: linear-gradient(to right, var(--accent) var(--p), var(--fg-3) var(--p));
  }
  input[type="range"]::-webkit-slider-thumb {
    appearance: none;
    width: 18px;
    height: 18px;
    margin-top: -7px;
    border-radius: 50%;
    background: var(--accent);
    border: 4px solid rgb(var(--surface-rgb));
    box-shadow: 0 0 0 1px var(--stroke-control);
    transition: transform 120ms ease-out;
  }
  input[type="range"]:hover::-webkit-slider-thumb {
    transform: scale(1.1);
  }
  .health {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-2);
    font-size: 11px;
    text-align: right;
  }
  .health .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
    box-shadow: inset 0 0 0 1.25px var(--fg-3);
  }
  .health.ok .dot {
    background: var(--ok-fill);
    box-shadow: none;
  }
  .health.warn {
    color: var(--warn);
  }
  .health.warn .dot {
    background: var(--warn-fill);
    box-shadow: none;
  }
  .health.crit {
    color: var(--crit);
  }
  .health.crit .dot {
    background: var(--crit-fill);
    box-shadow: none;
  }
  .welcome {
    margin-top: 8px;
    padding: 10px 12px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 30%, transparent);
  }
  .welcome h2 {
    margin: 0 0 4px;
    font-size: 12px;
    font-weight: 600;
  }
  .welcome ol {
    margin: 0;
    padding-left: 16px;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
  .welcome li + li {
    margin-top: 2px;
  }
  .welcome strong {
    color: var(--fg);
    font-weight: 600;
  }
  .privacy {
    display: flex;
    gap: 10px;
    margin-top: 16px;
    padding: 10px 12px 12px;
    border-radius: 6px;
    background: var(--fill-card);
    box-shadow: inset 0 0 0 1px var(--stroke);
  }
  .privacy > :global(.icon) {
    color: var(--ok);
    margin-top: 1px;
  }
  .privacy p {
    margin: 0 0 8px;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
  .privacy strong {
    color: var(--fg);
    font-weight: 600;
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
  .btn:hover {
    background: var(--fill-control-hover);
  }
  .quit {
    margin-top: 12px;
    width: 100%;
    justify-content: center;
  }
  .credits {
    margin: 10px 0 2px;
    text-align: center;
    font-size: 11px;
    color: var(--fg-3);
  }
  .legal {
    margin: 4px 0 0;
    text-align: center;
    font-size: 10.5px;
    line-height: 1.4;
    color: var(--fg-3);
  }
  .notices {
    display: flex;
    width: fit-content;
    margin: 8px auto 0;
  }
  .quit:hover {
    color: var(--crit);
  }
</style>
