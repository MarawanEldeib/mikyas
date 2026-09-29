import { expect, test } from "@playwright/test";
import { openWidget, VIEW_ROOT, type View } from "./helpers";

// Every view and every mock scenario renders, with no page errors and the key content shown.

const VIEWS: View[] = ["pill", "card", "settings", "sessions", "history"];
const SCENARIOS = [
  "normal",
  "high",
  "limit",
  "stale",
  "desktop-only",
  "no-session",
  "reset",
  "estimated",
  "warnings",
  "onboarding",
  "extra",
];

test.describe("every view renders in every scenario", () => {
  for (const scenario of SCENARIOS) {
    for (const view of VIEWS) {
      test(`${scenario} / ${view}`, async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        page.on("console", (m) => {
          if (m.type() === "error") errors.push(m.text());
        });
        await openWidget(page, { view, params: { scenario } });
        await expect(page.locator("html")).toHaveAttribute("data-view", view);
        await expect(page.locator(".fatal")).toHaveCount(0);
        expect(errors).toEqual([]);
      });
    }
  }
});

test.describe("card content per scenario", () => {
  test("normal: both limits with countdowns, session and data sources", async ({ page }) => {
    await openWidget(page, { params: { scenario: "normal" } });
    const card = page.locator(VIEW_ROOT.card);
    await expect(card.getByRole("region", { name: "5-hour limit" })).toBeVisible();
    await expect(card.getByRole("region", { name: "weekly limit" })).toBeVisible();
    await expect(card.getByRole("progressbar", { name: "5-hour usage" })).toHaveAttribute("aria-valuenow", "29");
    await expect(card.getByRole("progressbar", { name: "weekly usage" })).toHaveAttribute("aria-valuenow", "59");
    await expect(card.getByRole("button", { name: /Opus 5\.5/ })).toBeVisible();
    await expect(card.getByRole("list", { name: "Data sources" })).toBeVisible();
  });

  test("high: usage above the alert threshold", async ({ page }) => {
    await openWidget(page, { params: { scenario: "high" } });
    await expect(page.getByRole("progressbar", { name: "5-hour usage" })).toHaveAttribute("aria-valuenow", "83");
  });

  test("limit: the 5-hour limit shows the lock", async ({ page }) => {
    await openWidget(page, { params: { scenario: "limit" } });
    const five = page.getByRole("region", { name: "5-hour limit" });
    await expect(five.getByTitle("Limit reached", { exact: true })).toBeVisible();
    await expect(five.getByText(/Limit reached — usable again at 12:47/)).toBeVisible();
    await expect(page.getByRole("progressbar", { name: "5-hour usage" })).toHaveAttribute("aria-valuenow", "100");
  });

  test("extra: further limits get compact rows under the main two", async ({ page }) => {
    await openWidget(page, { params: { scenario: "extra" } });
    const card = page.locator(VIEW_ROOT.card);
    await expect(card.getByRole("region", { name: "5-hour limit" })).toBeVisible();
    await expect(card.getByRole("region", { name: "weekly limit" })).toBeVisible();
    const opus = card.getByRole("group", { name: "weekly Opus limit" });
    await expect(opus).toBeAttached();
    await expect(opus.getByRole("progressbar", { name: "weekly Opus usage" })).toHaveAttribute("aria-valuenow", "34");
    await expect(opus).toContainText("resets in 2d 4h");
    // A key the app has no name for still reads sensibly.
    const other = card.getByRole("group", { name: "monthly overage limit" });
    await expect(other).toContainText("12%");
    await expect(other).toContainText("reset time unknown");
    // The rows scroll into view inside the fixed-height card.
    await other.scrollIntoViewIfNeeded();
    await expect(other).toBeInViewport();
    // The compact views keep the two main windows.
    await openWidget(page, { view: "pill", params: { scenario: "extra" } });
    await expect(page.locator(VIEW_ROOT.pill)).not.toContainText("7d Opus");
    await expect(page.locator(VIEW_ROOT.pill)).not.toContainText("overage");
  });

  test("stale: readings are marked old", async ({ page }) => {
    await openWidget(page, { params: { scenario: "stale" } });
    await expect(page.getByRole("region", { name: "5-hour limit" }).getByText(/· 3h old/)).toBeVisible();
  });

  test("desktop-only and estimated: reset times are approximate", async ({ page }) => {
    for (const scenario of ["desktop-only", "estimated"]) {
      await openWidget(page, { params: { scenario } });
      await expect(page.getByRole("region", { name: "5-hour limit" }).locator(".reset")).toContainText("~");
    }
  });

  test("no-session: the header explains where the model comes from", async ({ page }) => {
    await openWidget(page, { params: { scenario: "no-session" } });
    await expect(page.locator(".head.none")).toBeVisible();
    await expect(page.getByRole("button", { name: /Opus 5\.5/ })).toHaveCount(0);
  });

  test("warnings: the footer chip opens settings, which lists every warning", async ({ page }) => {
    await openWidget(page, { params: { scenario: "warnings" } });
    const chip = page.getByRole("button", { name: /^3 warnings: Accounts may differ\. Open settings$/ });
    await expect(chip).toBeVisible();
    await chip.click();
    await expect(page.locator(VIEW_ROOT.settings)).toBeVisible();
    await expect(page.getByText("Accounts may differ", { exact: true })).toBeVisible();
    await expect(page.getByText("Claude Desktop data format changed", { exact: true })).toBeVisible();
    // The hotkey registration error is shown next to the field.
    await expect(page.getByRole("alert").filter({ hasText: "already used by another app" })).toBeVisible();
  });

  test("warnings: the pill shows a warning dot", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { scenario: "warnings" } });
    await expect(page.getByRole("img", { name: "3 warnings" })).toBeVisible();
  });

  test("onboarding: empty card leads to the welcome section", async ({ page }) => {
    await openWidget(page, { params: { scenario: "onboarding" } });
    await expect(page.getByText("Waiting for usage data")).toBeVisible();
    await page.getByRole("button", { name: "Set up data sources" }).click();
    await expect(page.getByRole("region", { name: "Welcome — connect a data source" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Connect Claude Code" })).toBeVisible();
  });

  test("onboarding: the pill says there is no data yet", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { scenario: "onboarding" } });
    await expect(page.getByText("No usage data yet")).toBeVisible();
  });

  test("reset: a window waiting for its first reading after a reset", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { scenario: "reset" } });
    // The wording after "0% used" is pinned as a known issue in a11y.spec.ts ("resets in reset").
    await expect(page.getByRole("img", { name: /^5-hour limit 0% used, / })).toBeVisible();
  });
});

test.describe("pill", () => {
  test("describes both limits for screen readers", async ({ page }) => {
    await openWidget(page, { view: "pill" });
    await expect(page.getByRole("img", { name: /^5-hour limit 29% used, resets in 3h 12m/ })).toBeVisible();
    await expect(page.getByRole("img", { name: /^weekly limit 59% used, resets in 2d 4h/ })).toBeVisible();
  });

  test("bar gauges", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { gauge: "bar" } });
    await expect(page.locator(".pill .bars .brow")).toHaveCount(2);
  });
});

test.describe("edge dock", () => {
  for (const dock of ["left", "right", "top"] as const) {
    test(`${dock}: pointing at the strip slides the card out, leaving slides it back`, async ({ page }) => {
      await openWidget(page, { view: "card", params: { dock }, waitFor: VIEW_ROOT.dock });
      const strip = page.locator(VIEW_ROOT.dock);
      await expect(strip).toHaveAccessibleName(/^5-hour 29% used\. weekly 59% used\. Show details$/);
      await expect(page.locator("html")).toHaveAttribute("data-dock-collapsed", "");
      await expect(strip).toHaveClass(dock === "top" ? /\bh\b/ : /\bv\b/);
      await strip.hover();
      await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
      await expect(page.locator("html")).not.toHaveAttribute("data-dock-collapsed", "");
      // The pointer moves over the card, then leaves the widget: it slides back in after a short delay.
      const box = (await page.locator("#app").boundingBox())!;
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 3 });
      await page.mouse.move(2, 2, { steps: 3 });
      await expect(page.locator(VIEW_ROOT.dock)).toBeVisible();
    });

    test(`${dock}: keyboard opens the strip and the minimize button tucks it in`, async ({ page }) => {
      await openWidget(page, { view: "card", params: { dock }, waitFor: VIEW_ROOT.dock });
      await page.mouse.move(2, 2);
      await page.keyboard.press("Tab");
      await expect(page.locator(VIEW_ROOT.dock)).toBeFocused();
      await page.keyboard.press("Enter");
      await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
      await page.getByRole("button", { name: "Minimize to the screen edge" }).click();
      await expect(page.locator(VIEW_ROOT.dock)).toBeVisible();
    });
  }

  test("settings opens expanded while docked", async ({ page }) => {
    await openWidget(page, { view: "settings", params: { dock: "right" } });
    await expect(page.locator(VIEW_ROOT.dock)).toHaveCount(0);
  });
});
