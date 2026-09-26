<script lang="ts">
  import { startDock } from "../dock";
  import { liveWindow } from "../format";
  import { api } from "../ipc";
  import { notices } from "../notices";
  import { app } from "../stores.svelte";
  import type { CardRows, WindowView } from "../types";
  import IconButton from "./IconButton.svelte";
  import SessionHeader from "./SessionHeader.svelte";
  import SourceBadges from "./SourceBadges.svelte";
  import StatusBanner from "./StatusBanner.svelte";
  import WindowSection from "./WindowSection.svelte";

  const snap = $derived(app.snapshot);
  const ghost = $derived(app.ui.click_through);
  const warn = $derived(notices(snap));
  const shown = $derived.by(() => {
    const ws = snap?.windows ?? [];
    const main = ["five_hour", "seven_day"]
      .map((k) => ws.find((w) => w.kind === k))
      .filter((w): w is WindowView => w !== undefined);
    return (main.length ? main : ws.slice(0, 2)).map((w) => liveWindow(w, app.now));
  });
  const noLimits = $derived(snap?.warnings.some((w) => w.type === "no_plan_limits") ?? false);
  const ALL_ROWS: CardRows = { sparklines: true, burn: true, session: true, sources: true };
  // Hidden rows shrink the window (window.rs card_height); the data attributes size the mock stage.
  const rows = $derived(app.settings?.card_rows ?? ALL_ROWS);

  // Slides the widget back into its strip when the pointer leaves (edge dock).
  startDock(app, api.setDockExpanded);
</script>

<div class="card" data-no-burn={rows.burn ? undefined : ""} data-no-session={rows.session ? undefined : ""}>
  {#if rows.session && (shown.length || snap?.session)}
    <SessionHeader session={snap?.session ?? null} now={app.now} showProject={app.settings?.show_project ?? true} />
  {/if}

  <div class="body">
    {#if shown.length}
      {#each shown as w, i (w.kind)}
        {#if i > 0}<div class="rule" aria-hidden="true"></div>{/if}
        <WindowSection window={w} now={app.now} sparkline={rows.sparklines} burn={rows.burn} />
      {/each}
    {:else}
      <div class="empty">
        {#if noLimits}
          <strong>No plan limits reported</strong>
          <p>Claude Code is signed in without usage limits (for example with an API key), so there is nothing to track.</p>
        {:else}
          <strong>Waiting for usage data</strong>
          <p>Connect Claude Code, or open Claude Desktop once — usage appears here automatically.</p>
          {#if !ghost}
            <button type="button" class="setup" onclick={() => app.setView("settings")}>Set up data sources</button>
          {/if}
        {/if}
      </div>
    {/if}
  </div>

  <footer>
    <div class="status">
      {#if warn.length}
        <StatusBanner notices={warn} compact onopen={ghost ? undefined : () => app.setView("settings")} />
      {:else if snap && rows.sources}
        <SourceBadges health={snap.health} now={app.now} staleMin={app.settings?.stale_min ?? 15} />
      {/if}
    </div>
    {#if !ghost}
      <div class="actions">
        <IconButton icon="pin" label={app.ui.pinned ? "Unpin (stop keeping on top)" : "Pin on top"} pressed={app.ui.pinned} onclick={() => app.togglePinned()} />
        <IconButton icon="collapse" label="Collapse to pill" onclick={() => app.setView("pill")} />
        <IconButton icon="history" label="History" onclick={() => app.setView("history")} />
        <IconButton icon="tune" label="Settings" onclick={() => app.setView("settings")} />
      </div>
    {/if}
  </footer>
</div>

<style>
  .card {
    height: 100%;
    display: flex;
    flex-direction: column;
    padding: 10px 12px 8px;
    gap: 8px;
  }
  .body {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .rule {
    height: 1px;
    flex: none;
    background: var(--divider);
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    height: 24px;
    flex: none;
  }
  .status {
    min-width: 0;
    display: flex;
  }
  .actions {
    display: flex;
    gap: 2px;
    margin-right: -4px;
    flex: none;
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: center;
    align-items: flex-start;
    gap: 4px;
    padding: 0 2px;
  }
  .empty strong {
    font-size: 14px;
    line-height: 20px;
    font-weight: 600;
  }
  .empty p {
    margin: 0;
    color: var(--fg-2);
    max-width: 272px;
  }
  .setup {
    margin-top: 8px;
    height: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 600;
  }
  .setup:hover {
    background: var(--accent-hover);
  }
</style>
