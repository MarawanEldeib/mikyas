<script lang="ts">
  import { clampPct, fillColor } from "../color";
  import { ctxLabel, formatAge, formatTokens, modelLabel } from "../format";
  import type { Entrypoint, SessionView } from "../types";
  import Icon, { type IconName } from "./Icon.svelte";

  interface Props {
    session: SessionView | null;
    now: number;
    showProject: boolean;
  }

  let { session, now, showProject }: Props = $props();

  const SURFACE: Record<Entrypoint, { icon: IconName; name: string } | null> = {
    cli: { icon: "terminal", name: "Claude Code (terminal)" },
    desktop: { icon: "desktop", name: "Claude Desktop — Code" },
    cowork: { icon: "cowork", name: "Claude Desktop — Cowork" },
    unknown: null,
  };

  const BASIS: Record<SessionView["ctx_basis"], string> = {
    statusline: "reported by Claude Code",
    identity: "1M model detected from the transcript",
    desktop_model: "model from Claude Desktop",
    override: "your override",
    heuristic: "inferred: a turn exceeded 200K",
    default: "default 200K",
  };

  const surface = $derived(session ? SURFACE[session.entrypoint] : null);
  const ctx = $derived(session?.ctx_pct == null ? null : clampPct(session.ctx_pct));
  const ctxTip = $derived.by(() => {
    if (!session) return "";
    const used = session.ctx_tokens === null ? "" : `${formatTokens(session.ctx_tokens)} of `;
    const est = session.ctx_is_estimate ? " (estimated from the transcript)" : "";
    return `Context: ${used}${formatTokens(session.ctx_size)} tokens${est}\nWindow size: ${BASIS[session.ctx_basis]}`;
  });
  const chipTip = $derived(
    session
      ? [surface?.name, session.model_id, session.project ? `Project: ${session.project}` : null, session.concurrent > 1 ? `${session.concurrent} sessions active` : null]
          .filter(Boolean)
          .join("\n")
      : "",
  );
</script>

{#if session}
  <div class="head">
    <span class="chip" title={chipTip}>
      {#if surface}<Icon name={surface.icon} size={12} />{/if}
      <span class="model">{modelLabel(session)}</span>
      {#if session.concurrent > 1}<span class="more" aria-label="{session.concurrent} sessions">+{session.concurrent - 1}</span>{/if}
    </span>
    <!-- Reversed wrapping row: the project name is shown only when it fits in full. -->
    <span class="meta">
      <span class="age">{formatAge(session.last_active_ms, now)}</span>
      {#if showProject && session.project}<span class="project" title={session.project}>{session.project}</span>{/if}
    </span>
    <span class="ctx" title={ctxTip}>
      <span class="ctx-label">{ctxLabel(session)}</span>
      <span class="ctx-bar" aria-hidden="true">
        {#if ctx !== null}<span class="ctx-fill" style:width="{ctx}%" style:background={fillColor(ctx)}></span>{/if}
      </span>
    </span>
  </div>
{:else}
  <div class="head none" title="The model and context are read from Claude Code (terminal, Desktop Code tab or Cowork) sessions. Claude Desktop's chat doesn't expose which model it uses.">
    <span class="none-title">No Claude Code session —</span>
    <span class="none-sub">Desktop chat model isn't exposed</span>
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
    border-radius: 11px;
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--fg);
    font-weight: 600;
    white-space: nowrap;
    flex: none;
  }
  .chip :global(.icon) {
    color: var(--fg-2);
  }
  .more {
    font-size: 10px;
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
    flex: 1 1 auto;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 16px;
    white-space: nowrap;
  }
  .age,
  .project {
    flex: none;
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
  /* Two tight lines: the sentence does not fit the 296px header on one line. */
  .none {
    flex-direction: column;
    align-items: flex-start;
    justify-content: center;
    gap: 0;
    height: 24px;
    font-size: 11px;
    line-height: 12px;
    white-space: nowrap;
  }
  .none-title {
    font-weight: 600;
  }
  .none-sub {
    color: var(--fg-2);
  }
</style>
