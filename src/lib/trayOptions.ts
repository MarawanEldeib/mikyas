// The tray-number choices, built from the data: "worst", every limit the snapshot has (main ones
// first, named like the card), the saved choice even when that limit is absent right now (the
// tray then shows the worst value), and "off".

import { windowLabel } from "./format";
import type { TrayNumber, WindowKind } from "./types";
import { allWindows } from "./windows";

export interface TrayOption {
  value: TrayNumber;
  label: string;
}

/** Window names as the card and toasts use them ("5-hour", "weekly Opus"), capitalised. */
function optionLabel(w: WindowKind | { kind: WindowKind; label?: string }): string {
  const name = windowLabel(w);
  return name.charAt(0).toUpperCase() + name.slice(1);
}

export function trayOptions(windows: readonly { kind: WindowKind; label?: string; is_main?: boolean }[], saved: TrayNumber): TrayOption[] {
  const limits: TrayOption[] = allWindows(windows).map((w) => ({ value: w.kind, label: optionLabel(w) }));
  if (saved !== "worst" && saved !== "off" && !limits.some((o) => o.value === saved)) {
    limits.push({ value: saved, label: `${optionLabel(saved)} (not reported now)` });
  }
  return [{ value: "worst", label: "Highest limit" }, ...limits, { value: "off", label: "Off (dot)" }];
}
