<script lang="ts">
  import { burnText, formatClock, resetAt, type BurnText } from "../format";
  import type { WindowView } from "../types";

  interface Props {
    window: WindowView;
    now: number;
  }

  let { window: w, now }: Props = $props();

  const line = $derived.by((): BurnText | null => {
    if (w.phase === "reset_awaiting_data") return null;
    if (w.limit_reached) {
      const at = resetAt(w.reset);
      const t = w.reset.type === "estimated" ? "~" : "";
      return { text: at !== null && at > now ? `Limit reached — usable again at ${t}${formatClock(at, now)}` : "Limit reached", tone: "crit" };
    }
    return burnText(w.burn, w.reset, now);
  });
  const slope = $derived(w.burn ? `Recent pace: ${Math.round(w.burn.slope_pct_per_h * 10) / 10}% per hour` : undefined);
</script>

{#if line}
  <p class="burn {line.tone}" title={slope}>
    {#if line.tone !== "muted" && !w.limit_reached}<span class="arrow" aria-hidden="true">↗</span>{/if}{line.text}
  </p>
{/if}

<style>
  .burn {
    margin: 0;
    font-size: 11px;
    line-height: 14px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg-2);
  }
  .warn {
    color: var(--warn);
  }
  .crit {
    color: var(--crit);
  }
  .arrow {
    display: inline-block;
    margin-right: 3px;
    font-weight: 600;
  }
</style>
