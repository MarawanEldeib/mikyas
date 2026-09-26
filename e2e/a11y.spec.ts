import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { openWidget, VIEW_ROOT, type View } from "./helpers";

// Keyboard-only use, visible focus, reduced motion and an axe-core scan of every view.

const VIEWS: View[] = ["pill", "card", "settings", "sessions", "history"];

/** Tabs through the widget and returns the accessible names reached, in order. */
async function tabOrder(page: Page, max = 60): Promise<string[]> {
  const names: string[] = [];
  for (let i = 0; i < max; i++) {
    await page.keyboard.press("Tab");
    const name = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      if (!el || el === document.body) return null;
      return el.getAttribute("aria-label") ?? el.textContent?.trim() ?? el.tagName;
    });
    if (name === null || names.includes(name)) break;
    names.push(name);
  }
  return names;
}

/**
 * Whether the focused element draws a visible focus indicator: an outline or box-shadow ring on
 * it, its ::before/::after, or a wrapper (the steppers' `.field:focus-within`) that is there while
 * focused and gone once blurred. A resting shadow (a card's elevation) does not count. Focus is
 * put back afterwards, so the next Tab continues from the same element.
 */
async function focusVisible(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const el = document.activeElement as HTMLElement | null;
    if (!el || el === document.body || !el.matches(":focus-visible")) return false;
    const targets: [Element, string | undefined][] = [
      [el, undefined],
      [el, "::after"],
      [el, "::before"],
    ];
    for (let p = el.parentElement, i = 0; p && i < 2; p = p.parentElement, i++) targets.push([p, undefined]);
    const ring = () =>
      targets.map(([e, pseudo]) => {
        const s = getComputedStyle(e, pseudo);
        const outline =
          s.outlineStyle !== "none" && parseFloat(s.outlineWidth) > 0 ? `${s.outlineStyle} ${s.outlineWidth} ${s.outlineColor}` : "";
        const shadow = s.boxShadow !== "none" ? s.boxShadow : "";
        return outline || shadow ? `${outline}|${shadow}` : "";
      });
    const focused = ring();
    el.blur();
    const blurred = ring();
    el.focus();
    return focused.some((r, i) => r !== "" && r !== blurred[i]);
  });
}

test.describe("keyboard only", () => {
  test("card: window controls come first, then the content, then the toolbar", async ({ page }) => {
    await openWidget(page);
    const order = await tabOrder(page);
    expect(order[0]).toBe("Minimize to pill");
    expect(order[1]).toBe("Hide to tray");
    expect(order).toContain("History");
    expect(order).toContain("Settings");
    expect(order.indexOf("Settings")).toBeGreaterThan(order.indexOf("History"));
  });

  test("every focus stop on each view shows a focus ring", async ({ page }) => {
    // At least this many stops per view, so a view that stops rendering its controls fails here.
    const MIN_STOPS: Record<View, number> = { pill: 5, card: 8, settings: 50, sessions: 2, history: 10 };
    for (const view of VIEWS) {
      await openWidget(page, { view, params: { update: "1" } });
      if (view === "history") await expect(page.getByRole("region", { name: "Weekly budget used per day" })).toBeVisible();
      let stops = 0;
      // Walk the whole tab order once: until focus falls off the end or comes back round.
      for (let i = 0; i < 200; i++) {
        await page.keyboard.press("Tab");
        const label = await page.evaluate(() => {
          const el = document.activeElement as HTMLElement | null;
          if (!el || el === document.body || el.dataset.e2eSeen) return null;
          el.dataset.e2eSeen = "1";
          return el.outerHTML.slice(0, 120);
        });
        if (label === null) break;
        stops++;
        expect(await focusVisible(page), `${view}: ${label}`).toBe(true);
      }
      expect(stops, `${view}: focus stops`).toBeGreaterThanOrEqual(MIN_STOPS[view]);
    }
  });

  test("pill → card → settings → back → history → back without a mouse", async ({ page }) => {
    await openWidget(page, { view: "pill" });
    // Tab to "Show details" (after the window controls) and open the card.
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press("Tab");
      if (await page.getByRole("button", { name: "Show details" }).evaluate((el) => el === document.activeElement)) break;
    }
    await expect(page.getByRole("button", { name: "Show details" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();

    await page.getByRole("button", { name: "Settings" }).focus();
    await page.keyboard.press("Enter");
    await expect(page.locator(VIEW_ROOT.settings)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();

    await page.getByRole("button", { name: "History" }).focus();
    await page.keyboard.press("Space");
    await expect(page.locator(VIEW_ROOT.history)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
  });

  test("settings toggles and radios work with Space and arrows", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    const toggle = page.getByRole("switch", { name: "Weekly recap" });
    await toggle.focus();
    await page.keyboard.press("Space");
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    const bars = page.getByRole("radio", { name: "Bars" });
    await page.getByRole("radio", { name: "Rings" }).focus();
    await page.keyboard.press("ArrowRight");
    await expect(bars).toBeChecked();
  });

  test("the update list keeps focus inside the widget when closed", async ({ page }) => {
    await openWidget(page, { params: { update: "1" } });
    await page.getByRole("button", { name: "3 updates available" }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Close the update list" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Unpin (stop keeping on top)" })).toBeFocused();
  });
});

test.describe("reduced motion", () => {
  test("transitions and animations are switched off", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openWidget(page, { params: { update: "1" } });
    const moving = await page.evaluate(() =>
      [...document.querySelectorAll("*")]
        .map((el) => ({ el, s: getComputedStyle(el) }))
        .filter(({ s }) => {
          const t = s.transitionDuration.split(",").some((d) => parseFloat(d) > 0);
          const a = s.animationName !== "none" && s.animationDuration.split(",").some((d) => parseFloat(d) > 0);
          return t || a;
        })
        .map(({ el }) => el.className.toString()),
    );
    expect(moving).toEqual([]);
  });

  test("without the preference the widget does animate", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await openWidget(page);
    const animated = await page.evaluate(() =>
      [...document.querySelectorAll("*")].some((el) =>
        getComputedStyle(el)
          .transitionDuration.split(",")
          .some((d) => parseFloat(d) > 0),
      ),
    );
    expect(animated).toBe(true);
  });
});

async function seriousViolations(page: Page): Promise<string[]> {
  const results = await new AxeBuilder({ page }).include("#app").analyze();
  return results.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .flatMap((v) => v.nodes.map((n) => ({ id: v.id, impact: v.impact, target: n.target.join(" ") })))
    .map((v) => `${v.id} (${v.impact}): ${v.target}`);
}

/** Light mode, card with the update list open. */
async function openUpdateList(page: Page): Promise<void> {
  await page.emulateMedia({ colorScheme: "light" });
  await openWidget(page, { params: { update: "1", bg: "light" } });
  await page.getByRole("button", { name: "3 updates available" }).click();
  await expect(page.getByRole("dialog", { name: "Updates available" })).toBeVisible();
}

// Once-known findings, kept as regression tests now that they are fixed.
test.describe("fixed a11y issues stay fixed", () => {
  for (const scheme of ["light", "dark"] as const) {
    test(`update list (${scheme}): no scrollable-region-focusable or color-contrast finding`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await openWidget(page, { params: { update: "1", bg: scheme } });
      await page.getByRole("button", { name: "3 updates available" }).click();
      await expect(page.getByRole("dialog", { name: "Updates available" })).toBeVisible();
      const found = await seriousViolations(page);
      expect(found.filter((f) => /^(scrollable-region-focusable|color-contrast) /.test(f))).toEqual([]);
    });
  }

  test("update list: the scrolling release list takes keyboard focus and shows a ring", async ({ page }) => {
    await openUpdateList(page);
    const list = page.getByRole("list", { name: "Release notes" });
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press("Tab");
      if (await list.evaluate((el) => el === document.activeElement)) break;
    }
    await expect(list).toBeFocused();
    expect(await focusVisible(page)).toBe(true);
    // It really scrolls (why it needs the focus stop), and the keyboard scrolls it.
    expect(await list.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
    await page.keyboard.press("End");
    await expect.poll(() => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  });

  test("update list: the latest tag has at least 4.5:1 contrast in both themes", async ({ page }) => {
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await openWidget(page, { params: { update: "1", bg: scheme } });
      await page.getByRole("button", { name: "3 updates available" }).click();
      const results = await new AxeBuilder({ page }).include(".tag").withRules(["color-contrast"]).analyze();
      expect(results.violations, scheme).toEqual([]);
      expect(results.passes.length, scheme).toBeGreaterThan(0);
    }
  });

  test("sessions: the count reads as '4 sessions'", async ({ page }) => {
    await openWidget(page, { view: "sessions" });
    await expect(page.locator(".count")).toHaveText("4 sessions");
  });

  test("sessions: the context label reads as 'context 34%'", async ({ page }) => {
    await openWidget(page, { view: "sessions" });
    await expect(page.locator(".ctx-pct").first()).toHaveText("context 34%");
  });

  test("pill: a window waiting after its reset says so instead of 'resets in reset'", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { scenario: "reset" } });
    const win = page.getByRole("img", { name: /^5-hour limit 0% used/ });
    await expect(win).toHaveAccessibleName(/, reset now, waiting for new data(,|$)/);
    await expect(win).not.toHaveAccessibleName(/resets in (reset|—)/);
  });
});

test.describe("axe-core", () => {
  const cases: { name: string; view: View; params?: Record<string, string> }[] = [
    ...VIEWS.map((view) => ({ name: view, view })),
    { name: "card with update list", view: "card", params: { update: "1" } },
    { name: "card warnings", view: "card", params: { scenario: "warnings" } },
    { name: "card lost connection", view: "card", params: { lost: "1" } },
    { name: "card onboarding", view: "card", params: { scenario: "onboarding" } },
    { name: "settings onboarding", view: "settings", params: { scenario: "onboarding" } },
    { name: "card limit", view: "card", params: { scenario: "limit" } },
    { name: "docked strip", view: "card", params: { dock: "left" } },
  ];
  for (const scheme of ["dark", "light"] as const) {
    for (const c of cases) {
      test(`${c.name} (${scheme}): no serious or critical violations`, async ({ page }) => {
        await page.emulateMedia({ colorScheme: scheme });
        await openWidget(page, {
          view: c.view,
          params: { bg: scheme, ...c.params },
          waitFor: c.params?.dock ? VIEW_ROOT.dock : undefined,
        });
        if (c.params?.update) await page.getByRole("button", { name: "3 updates available" }).click();
        expect(await seriousViolations(page)).toEqual([]);
      });
    }
  }
});
