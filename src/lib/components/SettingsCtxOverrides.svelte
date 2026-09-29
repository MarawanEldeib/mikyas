<script lang="ts">
  import { CTX_PRESETS, CTX_SIZE_HINT, formatCtxSize, normalizeModelId, parseCtxSize } from "../ctxSize";
  import { app } from "../stores.svelte";
  import IconButton from "./IconButton.svelte";

  const s = $derived(app.settings);
  const overrides = $derived(Object.entries(s?.ctx_overrides ?? {}).sort(([a], [b]) => a.localeCompare(b)));

  let newModel = $state("");
  let newSize = $state(formatCtxSize(1_000_000));
  let error = $state<string | null>(null);

  function setOverride(id: string, size: number) {
    app.patch({ ctx_overrides: { ...(s?.ctx_overrides ?? {}), [id]: size } });
  }

  function addOverride(e: SubmitEvent) {
    e.preventDefault();
    const id = normalizeModelId(newModel);
    if (id === null) {
      error = "Use a model id like claude-opus-5-5";
      return;
    }
    const size = parseCtxSize(newSize);
    if (size === null) {
      error = CTX_SIZE_HINT;
      return;
    }
    error = null;
    setOverride(id, size);
    newModel = "";
  }

  /** Commits an edited row; an invalid entry is reported and the field shows the saved size again. */
  function editOverride(id: string, input: HTMLInputElement, saved: number) {
    const size = parseCtxSize(input.value);
    if (size === null) {
      error = `${id}: ${CTX_SIZE_HINT}`;
      input.value = formatCtxSize(saved);
      return;
    }
    error = null;
    input.value = formatCtxSize(size);
    if (size !== saved) setOverride(id, size);
  }

  function removeOverride(id: string) {
    const next = { ...(s?.ctx_overrides ?? {}) };
    delete next[id];
    app.patch({ ctx_overrides: next });
  }
</script>

{#if s}
  <h2 class="section">Context window sizes</h2>
  <div class="group">
    <p class="row hint">
      Mikyas learns each model's window from Claude Code's status line. Override it here if “ctx %” looks wrong: type any size, like 400K or
      1.5M.
    </p>
    {#each overrides as [id, size] (id)}
      <div class="row">
        <code class="model" title={id}>{id}</code>
        <div class="inline">
          <input
            class="text size"
            type="text"
            inputmode="decimal"
            list="ctx-size-presets"
            aria-label="Context size for {id}"
            spellcheck="false"
            autocomplete="off"
            value={formatCtxSize(size)}
            onchange={(e) => editOverride(id, e.currentTarget, size)}
          />
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
        aria-invalid={error ? "true" : undefined}
      />
      <input
        class="text size"
        type="text"
        inputmode="decimal"
        list="ctx-size-presets"
        placeholder="Size"
        aria-label="Context size"
        spellcheck="false"
        autocomplete="off"
        bind:value={newSize}
      />
      <button type="submit" class="btn">Add</button>
    </form>
    {#if error}<p class="row hint crit-text" role="alert">{error}</p>{/if}
  </div>
  <!-- Quick picks for the size fields; any other size in range can be typed. -->
  <datalist id="ctx-size-presets">
    {#each CTX_PRESETS as n (n)}<option value={formatCtxSize(n)}></option>{/each}
  </datalist>
{/if}

<style>
  /* Same rules as Settings.svelte (section heading, grouped rows). */
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
  .group > :global(* + *) {
    border-top: 1px solid var(--divider);
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
  .text.size {
    flex: none;
    width: 76px;
    font-variant-numeric: tabular-nums;
  }
  .text:focus {
    box-shadow:
      inset 0 0 0 1px var(--stroke-control),
      inset 0 -2px 0 var(--accent);
  }
  .text::placeholder {
    color: var(--fg-2);
  }
  .btn {
    display: inline-flex;
    align-items: center;
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
  .text::-webkit-calendar-picker-indicator {
    opacity: 0.6;
  }
</style>
