// Warning banners derived from a snapshot (explicit warnings plus Desktop source health).

import type { Snapshot } from "./types";

export interface Notice {
  id: "account_mismatch" | "no_plan_limits" | "schema_changed" | "unreadable";
  title: string;
  detail: string;
}

/** Banners to show for a snapshot: explicit warnings in snapshot order, then Desktop health. */
export function notices(s: Snapshot | null): Notice[] {
  if (!s) return [];
  const out: Notice[] = [];
  for (const w of s.warnings) {
    if (w.type === "account_mismatch") {
      out.push({
        id: "account_mismatch",
        title: "Accounts may differ",
        detail: "Claude Desktop and Claude Code keep reporting different usage — they may be signed in to different accounts.",
      });
    } else if (w.type === "no_plan_limits") {
      out.push({
        id: "no_plan_limits",
        title: "No plan limits reported",
        detail: "Claude Code reports no usage limits — it is likely signed in with an API key or a plan without limits.",
      });
    }
  }
  const d = s.health.desktop;
  if (d.state === "schema_changed") {
    out.push({
      id: "schema_changed",
      title: "Claude Desktop data format changed",
      detail: `Its usage file now uses an unsupported format (v${d.version}), so Desktop data is ignored until the widget is updated.`,
    });
  } else if (d.state === "unreadable") {
    out.push({
      id: "unreadable",
      title: "Can't read Claude Desktop data",
      detail: "The usage history file exists but could not be read.",
    });
  }
  return out;
}
