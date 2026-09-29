import { expect, test, type Page } from "@playwright/test";
import { openWidget, VIEW_ROOT, type View } from "../helpers";

// Screenshot baselines: each view in dark and light at UI scale 1 and 1.3, plus the docked strip
// and the update list. The clock is fixed (helpers.FIXED_NOW) and the mock data is seeded, so
// countdowns, sparklines and history charts are identical on every run.
// Update after an intended UI change: npm run test:visual:update (then review the new images).

const VIEWS: View[] = ["pill", "card", "settings", "sessions", "history"];

/** Waits until nothing is still loading (fonts, the history fetch). */
async function settle(page: Page, view: View | "dock"): Promise<void> {
  await page.evaluate(() => document.fonts.ready);
  if (view === "history") await expect(page.getByRole("region", { name: "Weekly budget used per day" })).toBeVisible();
}

for (const scheme of ["dark", "light"] as const) {
  for (const scale of [1, 1.3]) {
    test.describe(`${scheme} · scale ${scale}`, () => {
      test.use({ colorScheme: scheme });
      for (const view of VIEWS) {
        test(view, async ({ page }) => {
          await openWidget(page, { view, params: { bg: scheme, scale } });
          await settle(page, view);
          await expect(page.locator("#app")).toHaveScreenshot(`${view}-${scheme}-${scale}.png`);
        });
      }
    });
  }
}

test.describe("states", () => {
  test("docked strip (left)", async ({ page }) => {
    await openWidget(page, { params: { dock: "left" }, waitFor: VIEW_ROOT.dock });
    await settle(page, "dock");
    await expect(page.locator("#app")).toHaveScreenshot("dock-left-dark-1.png");
  });

  test("extra limit rows on the card", async ({ page }) => {
    await openWidget(page, { params: { scenario: "extra" } });
    await settle(page, "card");
    await page.locator(".card .body").evaluate((el) => (el.scrollTop = el.scrollHeight));
    await expect(page.locator("#app")).toHaveScreenshot("card-extra-dark-1.png");
  });

  test("update list over the card", async ({ page }) => {
    await openWidget(page, { params: { update: "1" } });
    await page.getByRole("button", { name: "3 updates available" }).click();
    await expect(page.getByRole("dialog", { name: "Updates available" })).toBeVisible();
    await settle(page, "card");
    await expect(page.locator("#app")).toHaveScreenshot("update-list-dark-1.png");
  });
});
