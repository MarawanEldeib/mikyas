// Where a Claude session runs, as the session header and the Sessions view show it.

import type { IconName } from "./components/Icon.svelte";
import type { Entrypoint } from "./types";

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
