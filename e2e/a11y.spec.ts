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

/**
 * Serious axe findings in components this suite does not own, reported to their owners. They are
 * left out of the scans below and pinned by the `known issue` tests, which are expected to fail:
 * once a fix lands they pass, Playwright reports that, and the entry here must be removed.
 */
const KNOWN_AXE: { id: string; target: string; why: string }[] = [
  {
    id: "scrollable-region-focusable",
    target: ".releases",
    why: "UpdateList.svelte: the compact list scrolls but has no focusable element",
  },
  { id: "color-contrast", target: ".tag", why: 'UpdateList.svelte: the light-mode "latest" tag is below 4.5:1' },
];

async function seriousViolations(page: Page): Promise<string[]> {
  const results = await new AxeBuilder({ page }).include("#app").analyze();
  return results.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .flatMap((v) => v.nodes.map((n) => ({ id: v.id, impact: v.impact, target: n.target.join(" ") })))
    .map((v) => `${v.id} (${v.impact}): ${v.target}`);
}

/** Only the open update list has the known findings; every other scan reports everything. */
const isKnown = (finding: string) => KNOWN_AXE.some((k) => finding === `${k.id} (serious): ${k.target}`);

/** Light mode, card with the update list open (where the known findings are). */
async function openUpdateList(page: Page): Promise<void> {
  await page.emulateMedia({ colorScheme: "light" });
  await openWidget(page, { params: { update: "1", bg: "light" } });
  await page.getByRole("button", { name: "3 updates available" }).click();
  await expect(page.getByRole("dialog", { name: "Updates available" })).toBeVisible();
}

// Each test asserts that its defect is still there (not test.fail, which would also pass when the
// setup itself breaks). Once the owner fixes one, its test fails with FIXED: delete the test and,
// for an axe finding, its KNOWN_AXE entry.
const FIXED = (why: string) => `FIXED? ${why}. Remove this known-issue test (and its KNOWN_AXE entry).`;

test.describe("known issues (pinned until fixed)", () => {
  // One test per finding, so each one flips on its own when it is fixed.
  for (const k of KNOWN_AXE) {
    test(`update list: ${k.id} on ${k.target}`, async ({ page }) => {
      await openUpdateList(page);
      expect(await seriousViolations(page), FIXED(k.why)).toContain(`${k.id} (serious): ${k.target}`);
    });
  }

  test("sessions: the count loses its screen-reader space", async ({ page }) => {
    await openWidget(page, { view: "sessions" });
    const why = 'SessionsView.svelte: Svelte 5 trims the leading space of <span class="sr"> sessions</span> ("4sessions")';
    await expect(page.locator(".count"), FIXED(why)).toHaveText("4sessions");
  });

  test("sessions: the context label loses its screen-reader space", async ({ page }) => {
    await openWidget(page, { view: "sessions" });
    const why = 'SessionsView.svelte: Svelte 5 trims the trailing space of <span class="sr">context </span> ("context34%")';
    await expect(page.locator(".ctx-pct").first(), FIXED(why)).toHaveText("context34%");
  });

  test("pill: a window waiting after its reset reads 'resets in reset'", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { scenario: "reset" } });
    const why = "Pill.svelte describe(): pushes `resets in ${pillCountdown()}` even when the countdown is 'reset' or '—'";
    await expect(page.getByRole("img", { name: /^5-hour limit 0% used/ }), FIXED(why)).toHaveAccessibleName(/resets in reset$/);
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
        const all = await seriousViolations(page);
        // Known findings are only skipped where they are known to be: the open update list.
        const serious = c.params?.update ? all.filter((f) => !isKnown(f)) : all;
        expect(serious).toEqual([]);
      });
    }
  }
});
