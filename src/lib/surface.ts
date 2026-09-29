// Where a Claude session runs, as the session header and the Sessions view show it.

import type { IconName } from "./components/Icon.svelte";
import type { Entrypoint, SessionView } from "./types";

export interface Surface {
  icon: IconName;
  /** Short label for a list row. */
  short: string;
  /** Full name for tooltips and screen readers. */
  name: string;
}

export const SURFACE: Record<Entrypoint, Surface> = {
  cli: { icon: "terminal", short: "Terminal", name: "Claude Code (terminal)" },
  desktop: { icon: "desktop", short: "Desktop · Code", name: "Claude Desktop — Code" },
  cowork: { icon: "cowork", short: "Cowork", name: "Claude Desktop — Cowork" },
  unknown: { icon: "info", short: "Claude Code", name: "Claude Code" },
};

/** The surface of a session: a known one, else one named after the transcript's own
 *  `entrypoint` value ("claude-vscode" -> "Claude vscode"), so a surface added later is still
 *  told apart; else the generic "Claude Code". */
export function surfaceOf(s: Pick<SessionView, "entrypoint" | "entrypoint_raw">): Surface {
  const raw = s.entrypoint_raw?.trim();
  if (s.entrypoint !== "unknown" || !raw) return SURFACE[s.entrypoint];
  const words = raw
    .split(/[-_\s]+/)
    .filter(Boolean)
    .join(" ");
  const short = words.charAt(0).toUpperCase() + words.slice(1);
  return { ...SURFACE.unknown, short, name: `Claude Code (${raw})` };
}
