<script lang="ts">
  import { formatAge, formatAgeShort, sourceStale } from "../format";
  import type { SourceHealth } from "../types";

  interface Props {
    health: SourceHealth;
    now: number;
    staleMin: number;
  }

  let { health, now, staleMin }: Props = $props();

  interface Badge {
    name: string;
    full: string;
    at: number;
  }

  const badges = $derived.by(() => {
    const out: Badge[] = [];
    if (health.cli_last_capture_ms !== null) {
      out.push({ name: "CLI", full: "Claude Code statusline", at: health.cli_last_capture_ms });
    }
    const d = health.desktop;
    if (d.state === "ok" && d.last_sample_ms !== null) {
      out.push({ name: "Desktop", full: "Claude Desktop usage history", at: d.last_sample_ms });
    }
    return out;
  });
</script>

<ul class="badges" aria-label="Data sources">
  {#each badges as b (b.name)}
    {@const stale = sourceStale(b.at, now, staleMin)}
    <li class="badge" class:stale title="{b.full}: updated {formatAge(b.at, now)}{stale ? ' (stale)' : ''}">
      <span class="dot" aria-hidden="true"></span>{b.name} · {formatAgeShort(b.at, now)}
    </li>
  {/each}
</ul>

<style>
  .badges {
    display: flex;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
    min-width: 0;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 20px;
    padding: 0 7px 0 6px;
    border-radius: 10px;
    border: 1px solid var(--stroke);
    background: var(--fill-card);
    font-size: 11px;
    color: var(--fg-2);
    white-space: nowrap;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ok-fill);
  }
  .stale {
    border-style: dotted;
    border-color: var(--fg-3);
    background: transparent;
  }
  .stale .dot {
    background: transparent;
    box-shadow: inset 0 0 0 1.25px var(--fg-3);
  }
</style>
