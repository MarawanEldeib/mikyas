<script lang="ts">
  import { clampPct, fillColor } from "../color";
  import { ctxLabel, formatAge, formatTokens, modelLabel } from "../format";
  import { app } from "../stores.svelte";
  import { surfaceOf } from "../surface";
  import type { SessionView } from "../types";
  import Icon from "./Icon.svelte";

  interface Props {
    session: SessionView | null;
    now: number;
    showProject: boolean;
  }

  let { session, now, showProject }: Props = $props();

  // Where the window size came from; the size itself comes from the session, never from this text.
  const basisText = (s: SessionView): string => {
    const size = formatTokens(s.ctx_size);
    switch (s.ctx_basis) {
      case "statusline":
        return "reported by Claude Code";
      case "identity":
        return `long-context model detected from the transcript (${size})`;
      case "desktop_model":
        return "model from Claude Desktop";
      case "override":
        return "your override";
      case "learned":
        return "reported by Claude Code for this model";
      case "heuristic":
        return `inferred: a turn went past the default window (${size} assumed)`;
      case "default":
        return `default ${size} (not reported yet)`;
    }
  };

  // An unknown surface gets no icon here (the Sessions view still labels it).
  const surface = $derived(session && session.entrypoint !== "unknown" ? surfaceOf(session) : null);
  const ctx = $derived(session?.ctx_pct == null ? null : clampPct(session.ctx_pct));
  const ctxTip = $derived.by(() => {
    if (!session) return "";
    const used = session.ctx_tokens === null ? "" : `${formatTokens(session.ctx_tokens)} of `;
    const est = session.ctx_is_estimate ? " (estimated from the transcript)" : "";
    return `Context: ${used}${formatTokens(session.ctx_size)} tokens${est}\nWindow size: ${basisText(session)}`;
  });
  // Other sessions of the last 12 hours (the list includes this one).
  const others = $derived(Math.max(0, (app.snapshot?.sessions.length ?? 1) - 1));
  const listText = $derived(others > 0 ? `Show all ${others + 1} recent sessions` : "Show recent sessions");
  const chipTip = $derived(
    session
      ? [
          surface?.name,
          modelLabel(session),
          session.model_id,
          session.project ? `Project: ${session.project}` : null,
          session.concurrent > 1 ? `${session.concurrent} sessions active` : null,
          listText,
        ]
          .filter(Boolean)
          .join("\n")
      : "",
  );
</script>

{#if session}
  <div class="head">
    <button
      type="button"
      class="chip"
      title={chipTip}
      aria-label="{modelLabel(session)}. {listText}"
      onclick={() => app.setView("sessions")}
    >
      {#if surface}<Icon name={surface.icon} size={12} />{/if}
      <span class="model">{modelLabel(session)}</span>
      {#if others > 0}<span class="more" aria-hidden="true">+{others}</span>{/if}
    </button>
    <!-- Reversed wrapping row: the age and then the project name are shown only when they fit
         in full; the empty lead item lets even the age wrap onto the hidden second line. -->
    <span class="meta">
      <span class="lead"></span>
      <span class="age">{formatAge(session.last_active_ms, now)}</span>
      {#if showProject && session.project}<span class="project" title={session.project}>{session.project}</span>{/if}
    </span>
    <!-- Fades out while the window controls show over it (WindowControls.svelte). -->
    <span class="ctx" title={ctxTip} data-under-controls>
      <span class="ctx-label">{ctxLabel(session)}</span>
      <span class="ctx-bar" aria-hidden="true">
        {#if ctx !== null}<span class="ctx-fill" style:width="{ctx}%" style:background={fillColor(ctx)}></span>{/if}
      </span>
    </span>
  </div>
{:else}
  <div
    class="head none"
    title="The model and context are read from Claude Code (terminal, Desktop Code tab or Cowork) sessions. Claude Desktop's chat doesn't expose which model it uses."
  >
    <Icon name="info" size={14} />
    <p>
      <span class="none-title">No Claude Code session</span><span class="sr"> — </span>
      <span class="none-sub">Desktop chat model isn't exposed</span>
    </p>
  </div>
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 22px;
    min-width: 0;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 22px;
    padding: 0 8px 0 7px;
    border: 0;
    border-radius: 11px;
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--fg);
    font-weight: 600;
    white-space: nowrap;
    /* Shrinks (model name ellipsized) before the context bar is pushed out of the card. */
    flex: 0 1 auto;
    min-width: 0;
    transition: background-color 120ms ease-out;
  }
  .chip:hover {
    background: var(--fill-control-hover);
  }
  .chip:active {
    background: var(--fill-press);
  }
  .model {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .chip :global(.icon) {
    color: var(--fg-2);
  }
  .more {
    flex: none;
    margin-right: -2px;
    padding: 0 4px;
    border-radius: 6px;
    /* Not a control fill: in light theme that equals the chip's hover fill and the badge vanished. */
    background: var(--track);
    font-size: 10px;
    line-height: 13px;
    font-weight: 600;
    color: var(--fg-2);
  }
  .meta {
    display: flex;
    flex-direction: row-reverse;
    flex-wrap: wrap;
    justify-content: flex-end;
    column-gap: 4px;
    height: 16px;
    overflow: hidden;
    min-width: 0;
    /* Only takes space the model chip and the context bar leave over. */
    flex: 1 1 0;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 16px;
    white-space: nowrap;
  }
  .age,
  .project {
    flex: none;
  }
  .lead {
    width: 0;
    margin-left: -4px;
  }
  .project::after {
    content: " ·";
  }
  .ctx {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .ctx-label {
    font-size: 11px;
    color: var(--fg-2);
    white-space: nowrap;
  }
  .ctx-bar {
    position: relative;
    width: 36px;
    height: 4px;
    border-radius: 2px;
    background: var(--track);
    overflow: hidden;
  }
  .ctx-fill {
    position: absolute;
    inset: 0 auto 0 0;
    border-radius: 2px;
    transition: width var(--dur) var(--ease);
  }
  /* A quiet two-line note (the sentence does not fit the 296px header on one line), kept to the
     22px of the session row so the card height (window.rs) still fits both windows. */
  .none {
    gap: 6px;
    color: var(--fg-2);
  }
  .none p {
    display: flex;
    flex-direction: column;
    margin: 0;
    font-size: 11px;
    line-height: 11px;
    white-space: nowrap;
  }
  .none-title {
    font-weight: 600;
  }
  /* Keeps the sentence's dash for screen readers without a dangling dash on screen. */
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
