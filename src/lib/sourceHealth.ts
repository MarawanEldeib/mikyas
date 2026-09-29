import { formatAge } from "./format";
import type { DesktopHealth } from "./types";

export type HealthTone = "ok" | "warn" | "crit" | "off";

export interface HealthLine {
  tone: HealthTone;
  text: string;
}

/** The Settings "Data sources" line for Claude Desktop's usage history. */
export function desktopHealthLine(d: DesktopHealth | undefined, now: number): HealthLine {
  switch (d?.state) {
    case "ok": {
      const sample = d.last_sample_ms === null ? "no samples yet" : `sample ${formatAge(d.last_sample_ms, now)}`;
      // A newer file format still read best-effort: say so instead of looking fully supported.
      if (d.newer_version != null) return { tone: "warn", text: `Newer format (v${d.newer_version}), read best-effort · ${sample}` };
      return { tone: "ok", text: `Found · ${sample}` };
    }
    case "schema_changed":
      return { tone: "warn", text: `Unsupported format (v${d.version})` };
    case "unreadable":
      return { tone: "crit", text: "Found, but can't be read" };
    default:
      return { tone: "off", text: "Not found" };
  }
}
