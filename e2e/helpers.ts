import { expect, type Page } from "@playwright/test";

/** A fixed instant (Monday 12:00 UTC) so countdowns, ages and mock data never drift. */
export const FIXED_NOW = Date.UTC(2026, 8, 21, 12, 0, 0);

export type View = "pill" | "card" | "settings" | "sessions" | "history";

/** The root element each view renders (docked, the collapsed strip replaces pill and card). */
export const VIEW_ROOT: Record<View | "dock", string> = {
  pill: ".pill",
  card: ".card",
  settings: ".settings",
  sessions: ".view:has(h1:text-is('Sessions'))",
  history: ".view:has(h1:text-is('History'))",
  dock: "button.dock",
};

export interface OpenOptions {
  view?: View;
  /** Any other mock URL param: scenario, dock, update, lost, conn, scale, bg, ... */
  params?: Record<string, string | number>;
  /** Freeze Date at FIXED_NOW (timers keep running). Default true. */
  fixedClock?: boolean;
  /** Root selector to wait for (defaults to the view's). */
  waitFor?: string;
}

/** Opens the widget on the browser mock and waits until the view has rendered. */
export async function openWidget(page: Page, opts: OpenOptions = {}): Promise<void> {
  const view = opts.view ?? "card";
  if (opts.fixedClock ?? true) await page.clock.setFixedTime(FIXED_NOW);
  const q = new URLSearchParams({ view, bg: "dark" });
  for (const [k, v] of Object.entries(opts.params ?? {})) q.set(k, String(v));
  await page.goto(`/?${q}`);
  await expect(page.locator(opts.waitFor ?? VIEW_ROOT[view]).first()).toBeVisible();
}

/** The `view` the root element reports (App.svelte sets html[data-view]). */
export async function currentView(page: Page): Promise<string | undefined> {
  return page.evaluate(() => document.documentElement.dataset.view);
}

/** Collects the mock backend's console.info lines ("[mock] open_url …"). */
export function mockLog(page: Page): string[] {
  const lines: string[] = [];
  page.on("console", (m) => {
    if (m.text().startsWith("[mock]")) lines.push(m.text());
  });
  return lines;
}
