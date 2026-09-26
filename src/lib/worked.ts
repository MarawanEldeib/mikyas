import type { WindowView } from "./types";

/** Tooltip of the "▲" after a percentage (card, pill and dock strip). */
export const WORKED_SINCE_TIP = "Claude has worked since this reading, so the real value is higher";

/** Show the "▲" marker: Claude worked after the value was read. Not on a reached limit (it can't
 *  go higher) or a window waiting for its first reading after a reset (that 0% is no reading). */
export function showWorkedSince(w: Pick<WindowView, "worked_since" | "limit_reached" | "phase">): boolean {
  return w.worked_since && !w.limit_reached && w.phase !== "reset_awaiting_data";
}