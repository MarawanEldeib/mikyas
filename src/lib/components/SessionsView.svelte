<script lang="ts">
  import { clampPct, fillColor } from "../color";
  import { formatAge, formatPct, formatTime, formatTokens, modelLabel } from "../format";
  import { app } from "../stores.svelte";
  import { SURFACE } from "../surface";
  import type { SessionView } from "../types";
  import Icon from "./Icon.svelte";
  import IconButton from "./IconButton.svelte";

  const sessions = $derived(app.snapshot?.sessions ?? []);
  const activeKey = $derived(app.snapshot?.session?.key ?? null);
  const showProject = $derived(app.settings?.show_project ?? true);

  function ctxText(s: SessionView): string {
    if (s.ctx_pct === null || !Number.isFinite(s.ctx_pct)) return "—";
    return `${s.ctx_is_estimate ? "≈" : ""}${formatPct(s.ctx_pct)}%`;
  }

  function describe(s: SessionView, active: boolean): string {
    const surface = SURFACE[s.entrypoint];
    const used = s.ctx_tokens === null ? "" : `${formatTokens(s.ctx_tokens)} of `;
    const ctx =
      s.ctx_pct === null
        ? "Context unknown"
        : `Context ${ctxText(s)}: ${used}${formatTokens(s.ctx_size)} tokens${s.ctx_is_estimate ? " (estimated from the transcript)" : ""}`;
    return [
      active ? "Shown in the widget" : null,
      surface.name,
      s.model_id,
      showProject && s.project ? `Project: ${s.project}` : null,
      ctx,
      `Last reply ${formatTime(s.last_active_ms)} (${formatAge(s.last_active_ms, app.now)})`,
    ]
      .filter(Boolean)
      .join("\n");
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape" && !e.defaultPrevented) void app.back();
  }}
/>

<div class="view">
  <header class="bar">
    <IconButton icon="back" label="Back" size="m" onclick={() => app.back()} />
    <h1>Sessions</h1>
    <!-- aria-label is ignored on a plain span; the hidden word gives the number its meaning.
         {" "}: Svelte 5 trims a literal space at the edge of an element. -->
    {#if sessions.length}<span class="count">{sessions.length}<span class="sr">{" "}sessions</span></span>{/if}
    <span class="span">Last 12 hours</span>
  </header>

  {#if sessions.length}
    <ul class="list" data-no-drag aria-label="Recent Claude Code sessions">
      {#each sessions as s (s.key)}
        {@const active = s.key === activeKey}
        {@const surface = SURFACE[s.entrypoint]}
        {@const ctx = s.ctx_pct === null ? null : clampPct(s.ctx_pct)}
        {@const where = showProject && s.project ? s.project : surface.short}
        <li class="item" class:active aria-current={active ? "true" : undefined} title={describe(s, active)}>
          <span class="surface" aria-hidden="true"><Icon name={surface.icon} size={14} /></span>
          <div class="main">
            <div class="line">
              <span class="model">{modelLabel(s)}</span>
              {#if active}<span class="tag">Active</span>{/if}
              <span class="age">{formatAge(s.last_active_ms, app.now)}</span>
            </div>
            <div class="line sub">
              <span class="where" class:project={showProject && s.project}>{where}</span>
              <span class="ctx">
                <span class="ctx-bar" aria-hidden="true">
                  {#if ctx !== null}<span class="ctx-fill" style:width="{ctx}%" style:background={fillColor(ctx)}></span>{/if}
                </span>
                <span class="ctx-pct"><span class="sr">context{" "}</span>{ctxText(s)}</span>
              </span>
            </div>
          </div>
        </li>
      {/each}
    </ul>
  {:else}
    <div class="empty">
      <span class="empty-icon" aria-hidden="true"><Icon name="terminal" size={20} /></span>
      <strong>No recent sessions</strong>
      <p>Claude Code sessions in the terminal, the Desktop Code tab or Cowork appear here for 12 hours after their last reply.</p>
    </div>
  {/if}
</div>

<style>
  /* Mock stage size (browser preview only); zero specificity so app.css can override it. */
  :global(:where(html.mock[data-view="sessions"])) {
    --mock-w: 320px;
    --mock-h: 300px;
  }
  .view {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 44px;
    /* The right 42px are the window controls' × (24px, 10px in) and an 8px gap. */
    padding: 0 42px 0 6px;
    flex: none;
    border-bottom: 1px solid var(--divider);
  }
  h1 {
    margin: 0;
    font-size: 14px;
    line-height: 20px;
    font-weight: 600;
  }
  .count {
    min-width: 18px;
    height: 18px;
    margin-left: 4px;
    padding: 0 5px;
    border-radius: 9px;
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--fg-2);
    font-size: 11px;
    line-height: 18px;
    font-weight: 600;
    text-align: center;
  }
  .span {
    margin-left: auto;
    color: var(--fg-2);
    font-size: 11px;
  }
  .list {
    flex: 1;
    min-height: 0;
    margin: 0;
    /* The 10px scrollbar gutter completes the 12px right inset (as in Settings). */
    padding: 8px 2px 10px 12px;
    list-style: none;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 10px 7px 8px;
    border-radius: 6px;
  }
  .item + .item {
    margin-top: 2px;
  }
  .item:hover {
    background: var(--fill-hover);
  }
  .active,
  .active:hover {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 30%, transparent);
  }
  .surface {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    flex: none;
    border-radius: 6px;
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--fg-2);
  }
  .active .surface {
    color: var(--fg);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    height: 16px;
    white-space: nowrap;
  }
  .model {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 600;
  }
  .tag {
    flex: none;
    padding: 0 5px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 22%, transparent);
    color: var(--fg);
    font-size: 10px;
    line-height: 14px;
    font-weight: 600;
  }
  .age {
    flex: none;
    margin-left: auto;
    color: var(--fg-2);
    font-size: 11px;
  }
  .sub {
    font-size: 11px;
    color: var(--fg-2);
  }
  .where {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .where.project {
    color: var(--fg);
  }
  .ctx {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
    margin-left: auto;
  }
  .ctx-bar {
    position: relative;
    width: 44px;
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
  .ctx-pct {
    min-width: 30px;
    text-align: right;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 0 28px 16px;
    text-align: center;
  }
  .empty-icon {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    margin-bottom: 4px;
    border-radius: 10px;
    background: var(--fill-control);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--fg-2);
  }
  .empty strong {
    font-size: 13px;
    font-weight: 600;
  }
  .empty p {
    margin: 0;
    color: var(--fg-2);
    font-size: 11px;
    line-height: 15px;
  }
</style>
