import { expect, test } from "@playwright/test";
import { mockLog, openWidget, VIEW_ROOT } from "./helpers";

// User flows on the browser mock: view switches, window controls, settings, connect, updates.

test.describe("pill and card", () => {
  test("double-click on the pill opens the card", async ({ page }) => {
    await openWidget(page, { view: "pill" });
    await page.locator(".pill .win").first().dblclick();
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("data-view", "card");
  });

  test("a single click on the pill does nothing", async ({ page }) => {
    await openWidget(page, { view: "pill" });
    await page.locator(".pill .win").first().click();
    // Past the double-click window, so a late switch would have happened by now.
    await page.waitForTimeout(600);
    await expect(page.locator("html")).toHaveAttribute("data-view", "pill");
  });

  test("window controls: minimize to pill, expand to card, close hides", async ({ page }) => {
    const log = mockLog(page);
    await openWidget(page, { view: "card" });
    await page.locator(".card").hover({ position: { x: 20, y: 10 } });
    await page.getByRole("button", { name: "Minimize to pill" }).click();
    await expect(page.locator(VIEW_ROOT.pill)).toBeVisible();
    await page.getByRole("button", { name: "Expand" }).click();
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
    await page.getByRole("button", { name: "Hide to tray" }).click();
    await expect.poll(() => log).toContain("[mock] hide_widget");
  });

  test("close=quit: the × quits", async ({ page }) => {
    const log = mockLog(page);
    await openWidget(page, { view: "pill", params: { close: "quit" } });
    await page.getByRole("button", { name: "Quit Mikyas" }).click();
    await expect.poll(() => log).toContain("[mock] quit_app");
  });

  test("pin toggles and reports its state", async ({ page }) => {
    await openWidget(page);
    const pin = page.getByRole("button", { name: "Unpin (stop keeping on top)" });
    await expect(pin).toHaveAttribute("aria-pressed", "true");
    await pin.click();
    await expect(page.getByRole("button", { name: "Pin on top" })).toHaveAttribute("aria-pressed", "false");
  });

  test("ghost mode hides the window controls and the toolbar", async ({ page }) => {
    await openWidget(page, { params: { ghost: "1" } });
    await expect(page.locator(".widget.ghost")).toBeVisible();
    await expect(page.getByRole("group", { name: "Window" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Settings" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "History" })).toHaveCount(0);
    // The pointer passes through everything.
    expect(await page.locator(".card").evaluate((el) => getComputedStyle(el).pointerEvents)).toBe("none");
  });
});

test.describe("sessions", () => {
  test("the session chip opens the list, newest first, and Back returns", async ({ page }) => {
    await openWidget(page);
    await page.getByRole("button", { name: /^Opus 5\.5/ }).click();
    const list = page.getByRole("list", { name: "Recent Claude Code sessions" });
    await expect(list).toBeVisible();
    const items = list.getByRole("listitem");
    await expect(items).toHaveCount(4);
    await expect(items.first()).toHaveAttribute("aria-current", "true");
    await expect(items.first()).toContainText("Active");
    await expect(items.nth(1)).toContainText("demo-app");
    await expect(items.nth(3)).toContainText("api-gateway");
    await expect(page.locator(".count")).toContainText("4");
    await page.getByRole("button", { name: "Back" }).click();
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
  });

  test("no sessions: the empty state", async ({ page }) => {
    await openWidget(page, { view: "sessions", params: { scenario: "no-session" } });
    await expect(page.getByText("No recent sessions")).toBeVisible();
  });

  test("Esc goes back", async ({ page }) => {
    await openWidget(page, { view: "sessions" });
    await page.keyboard.press("Escape");
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
  });
});

test.describe("history", () => {
  test("opens from the card with charts and day bars", async ({ page }) => {
    await openWidget(page);
    await page.getByRole("button", { name: "History" }).click();
    await expect(page.getByRole("region", { name: "5-hour history" })).toBeVisible();
    await expect(page.getByRole("region", { name: "7-day history" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Weekly budget used per day" }).getByRole("button")).toHaveCount(7);
  });

  test("range switch by click and by arrow keys", async ({ page }) => {
    await openWidget(page, { view: "history" });
    const group = page.getByRole("radiogroup", { name: "Time range" });
    const r24 = group.getByRole("radio", { name: "last 24 hours" });
    const r7 = group.getByRole("radio", { name: "last 7 days" });
    const r14 = group.getByRole("radio", { name: "last 14 days" });
    await expect(r7).toHaveAttribute("aria-checked", "true");
    const bars = page.getByRole("region", { name: "Weekly budget used per day" }).getByRole("button");
    await r14.click();
    await expect(r14).toHaveAttribute("aria-checked", "true");
    await expect(bars).toHaveCount(14);
    await expect(page.locator(".summary")).toContainText("last 14 days");
    await r14.focus();
    await page.keyboard.press("ArrowRight");
    await expect(r24).toHaveAttribute("aria-checked", "true");
    await expect(r24).toBeFocused();
    await expect(r24).toHaveAttribute("tabindex", "0");
    await expect(r14).toHaveAttribute("tabindex", "-1");
    await page.keyboard.press("ArrowLeft");
    await expect(r14).toBeFocused();
  });

  test("a day bar shows its detail on focus", async ({ page }) => {
    await openWidget(page, { view: "history" });
    const days = page.getByRole("region", { name: "Weekly budget used per day" });
    await expect(days.locator(".scale")).toContainText("full bar =");
    await days.getByRole("button").last().focus();
    await expect(days.locator(".scale.picked")).toBeVisible();
  });

  test("load error offers Try again", async ({ page }) => {
    await openWidget(page, { view: "history", params: { history: "error" } });
    await expect(page.getByRole("alert")).toContainText("Couldn't load the history");
    await expect(page.getByRole("button", { name: "Try again" })).toBeVisible();
  });

  test("onboarding: no history yet", async ({ page }) => {
    await openWidget(page, { view: "history", params: { scenario: "onboarding" } });
    await expect(page.getByText("No history yet")).toBeVisible();
  });

  test("Esc and Back return to the view it was opened from", async ({ page }) => {
    await openWidget(page, { view: "pill" });
    await page.evaluate(() => document.querySelector<HTMLElement>(".expand")?.focus());
    await page.keyboard.press("Enter");
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
    await page.getByRole("button", { name: "History" }).click();
    await expect(page.locator(VIEW_ROOT.history)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
  });
});

test.describe("settings", () => {
  test("toggles flip and stay flipped", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    const toggle = page.getByRole("switch", { name: "Start with Windows" });
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    const project = page.getByRole("switch", { name: "Show project name" });
    await project.click();
    await expect(project).toHaveAttribute("aria-checked", "false");
    // The change reaches the card: the project name is hidden.
    await page.getByRole("button", { name: "Back" }).click();
    await expect(page.locator(".card .project")).toHaveCount(0);
  });

  test("steppers: buttons, arrow keys, typing and clamping", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    const first = page.getByRole("textbox", { name: "First alert threshold" });
    await expect(first).toHaveValue("80");
    await page.getByRole("button", { name: "Increase First alert threshold" }).click();
    await expect(first).toHaveValue("81");
    await page.getByRole("button", { name: "Decrease First alert threshold" }).click();
    await page.getByRole("button", { name: "Decrease First alert threshold" }).click();
    await expect(first).toHaveValue("79");
    await first.focus();
    await page.keyboard.press("ArrowUp");
    await expect(first).toHaveValue("80");
    // Above the second threshold: clamped to one below it.
    await first.fill("99");
    await first.press("Enter");
    await first.blur();
    await expect(first).toHaveValue("94");
    const stale = page.getByRole("textbox", { name: "Minutes until data is stale" });
    await stale.fill("abc");
    await stale.blur();
    await expect(stale).toHaveValue("15");
    await stale.fill("0");
    await stale.blur();
    await expect(stale).toHaveValue("1");
    await expect(page.getByRole("button", { name: "Decrease Minutes until data is stale" })).toBeDisabled();
  });

  test("hotkey field records a shortcut, Esc cancels, Tab leaves", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    const field = page.getByRole("button", { name: /^Click-through shortcut/ });
    await expect(field).toContainText("Ctrl");
    await field.click();
    await expect(field).toContainText("Press keys…");
    await page.keyboard.press("Control+Alt+K");
    await expect(field).not.toContainText("Press keys…");
    await expect(field).toContainText("K");
    // Esc cancels recording without leaving Settings.
    await field.click();
    await page.keyboard.press("Escape");
    await expect(field).not.toContainText("Press keys…");
    await expect(page.locator(VIEW_ROOT.settings)).toBeVisible();
    // Tab moves on instead of being recorded.
    await field.click();
    await page.keyboard.press("Tab");
    await expect(field).not.toBeFocused();
  });

  test("the show/hide shortcut can be removed", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    await page.getByRole("button", { name: "Remove the show / hide shortcut" }).click();
    await expect(page.getByText("No shortcut. Click to record one.")).toBeVisible();
  });

  test("warnings: recording a new shortcut clears the registration error", async ({ page }) => {
    await openWidget(page, { view: "settings", params: { scenario: "warnings" } });
    const err = page.getByRole("alert").filter({ hasText: "already used by another app" });
    await expect(err).toBeVisible();
    await page.getByRole("button", { name: /^Click-through shortcut/ }).click();
    await page.keyboard.press("Control+Shift+Y");
    await expect(err).toHaveCount(0);
  });

  test("UI size radio scales the widget", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    await page.getByRole("radiogroup", { name: "Size" }).getByRole("radio", { name: "Extra" }).click();
    await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue("--ui-scale"))).toBe("1.3");
  });

  test("context overrides: add, validate and remove", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    const model = page.getByRole("textbox", { name: "Model id" });
    await model.fill("not a model!");
    await page.getByRole("button", { name: "Add" }).click();
    await expect(page.getByRole("alert").filter({ hasText: "Use a model id like" })).toBeVisible();
    await model.fill("claude-sonnet-5[1m]");
    await page.getByRole("button", { name: "Add" }).click();
    await expect(page.getByRole("combobox", { name: "Context size for claude-sonnet-5" })).toBeVisible();
    await page.getByRole("button", { name: "Remove override for claude-sonnet-5" }).click();
    await expect(page.getByRole("combobox", { name: "Context size for claude-sonnet-5" })).toHaveCount(0);
  });

  test("Esc goes back, except while typing in a field", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    await page.getByRole("textbox", { name: "Model id" }).focus();
    await page.keyboard.press("Escape");
    await expect(page.locator(VIEW_ROOT.settings)).toBeVisible();
    await page.getByRole("heading", { name: "Settings" }).click();
    await page.keyboard.press("Escape");
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
  });

  test("Check now finds updates and lists them", async ({ page }) => {
    await openWidget(page, { view: "settings" });
    await page.getByRole("button", { name: "Check now" }).click();
    await expect(page.getByText("v0.4.0").first()).toBeVisible();
    await expect(page.locator(".settings .releases > li")).toHaveCount(3);
  });

  test("Check now: nothing new and a failure", async ({ page }) => {
    await openWidget(page, { view: "settings", params: { update: "none" } });
    await page.getByRole("button", { name: "Check now" }).click();
    await expect(page.getByRole("button", { name: "Check now" })).toBeEnabled();
    await expect(page.locator(".settings .releases")).toHaveCount(0);
    await openWidget(page, { view: "settings", params: { update: "error" } });
    await page.getByRole("button", { name: "Check now" }).click();
    await expect(page.getByText(/Couldn't reach GitHub/)).toBeVisible();
  });

  test("about: independence line and third-party licenses", async ({ page }) => {
    const log = mockLog(page);
    await openWidget(page, { view: "settings" });
    await expect(page.getByText("Idea by Eng. Abdulrahman Alhelali · Built by Eng. Marawan Eldeib")).toBeVisible();
    await expect(
      page.getByText(
        "Independent project, not affiliated with or endorsed by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC.",
      ),
    ).toBeVisible();
    await page.getByRole("button", { name: "Third-party licenses" }).click();
    await expect.poll(() => log).toContain("[mock] open_third_party_notices");
    await expect(page.getByRole("button", { name: "Quit Mikyas" })).toBeVisible();
  });
});

test.describe("connect Claude Code", () => {
  test("preview, cancel, then confirm with a passing self-test, then disconnect", async ({ page }) => {
    await openWidget(page, { view: "settings", params: { conn: "not_configured" } });
    const connect = page.getByRole("button", { name: "Connect Claude Code" });
    await connect.click();
    await expect(page.getByText("Before", { exact: true })).toBeVisible();
    await expect(page.locator(".preview pre").first()).toHaveText("(no statusLine)");
    await expect(page.locator(".preview pre.after")).toContainText("mikyas-capture.exe");
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.locator(".preview")).toHaveCount(0);
    await connect.click();
    await page.getByRole("button", { name: "Confirm" }).click();
    await expect(page.getByText("Self-test passed — your statusline output is unchanged.")).toBeVisible();
    await expect(page.locator(".state.ok")).toHaveText("Connected");
    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(page.getByRole("button", { name: "Connect Claude Code" })).toBeVisible();
  });

  test("a foreign statusline is wrapped, and disconnect restores it", async ({ page }) => {
    await openWidget(page, { view: "settings", params: { conn: "foreign" } });
    await expect(page.locator(".cmd")).toHaveText("npx -y ccstatusline@latest");
    await page.getByRole("button", { name: "Connect Claude Code" }).click();
    await expect(page.locator(".preview pre.after")).toContainText("--wrap -- npx -y ccstatusline@latest");
    await page.getByRole("button", { name: "Confirm" }).click();
    await expect(page.getByText("wraps your statusline")).toBeVisible();
    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(page.getByText("another statusline is set")).toBeVisible();
  });

  test("warnings scenario: the preview lists project overrides", async ({ page }) => {
    await openWidget(page, { view: "settings", params: { scenario: "warnings", conn: "not_configured" } });
    await page.getByRole("button", { name: "Connect Claude Code" }).click();
    await expect(page.locator(".preview .warn-line")).toContainText("sets its own statusLine");
  });

  test("unreadable settings offer Try again", async ({ page }) => {
    await openWidget(page, { view: "settings", params: { conn: "error" } });
    await expect(page.getByText("Can't read settings")).toBeVisible();
    await expect(page.getByRole("button", { name: "Try again" })).toBeVisible();
  });
});

test.describe("lost connection banner", () => {
  test("lost=1: reconnect wraps the status line again and the banner goes", async ({ page }) => {
    await openWidget(page, { params: { lost: "1" } });
    const banner = page.locator(".lost");
    await expect(banner).toContainText("Status line changed");
    await banner.getByRole("button", { name: "Reconnect" }).click();
    await expect(banner.getByRole("button", { name: "Reconnecting…" })).toBeDisabled();
    await expect(banner).toHaveCount(0);
    await expect(page.getByRole("list", { name: "Data sources" })).toBeVisible();
  });

  test("lost=1: dismiss brings the data sources back", async ({ page }) => {
    await openWidget(page, { params: { lost: "1" } });
    await page.getByRole("button", { name: "Dismiss the status line warning" }).click();
    await expect(page.locator(".lost")).toHaveCount(0);
    await expect(page.getByRole("list", { name: "Data sources" })).toBeVisible();
  });

  test("lost=1 on the pill: a warning dot", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { lost: "1" } });
    await expect(page.getByRole("img", { name: "1 warning" })).toBeVisible();
  });
});

test.describe("updates", () => {
  test("update=1: the banner lists every skipped version; Update opens the latest release", async ({ page }) => {
    const log = mockLog(page);
    await openWidget(page, { params: { update: "1" } });
    const banner = page.getByRole("button", { name: "3 updates available" });
    await expect(banner).toHaveAttribute("aria-expanded", "false");
    await banner.click();
    await expect(banner).toHaveAttribute("aria-expanded", "true");
    const dialog = page.getByRole("dialog", { name: "Updates available" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("3 updates available")).toBeVisible();
    await expect(dialog.locator(".releases > li")).toHaveCount(3);
    await expect(dialog.locator(".version").first()).toContainText("v0.4.0");
    await expect(dialog.locator(".version").first()).toContainText("latest");
    await expect(dialog.getByText("fix weekly reset detection")).toBeVisible();
    // Focus moves into the list.
    await expect(dialog.getByRole("button", { name: "Close the update list" })).toBeFocused();
    await dialog.getByRole("button", { name: "Update", exact: true }).click();
    await expect.poll(() => log.join("\n")).toContain("open_url https://github.com/MarawanEldeib/mikyas/releases/tag/v0.4.0");
    await expect(dialog).toHaveCount(0);
  });

  test("update=1: Later hides the banner", async ({ page }) => {
    await openWidget(page, { params: { update: "1" } });
    await page.getByRole("button", { name: "3 updates available" }).click();
    await page.getByRole("dialog", { name: "Updates available" }).getByRole("button", { name: "Later" }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".update")).toHaveCount(0);
  });

  test("update=1: Esc closes the list before anything else", async ({ page }) => {
    await openWidget(page, { params: { update: "1" } });
    await page.getByRole("button", { name: "3 updates available" }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
    await expect(page.getByRole("button", { name: "Unpin (stop keeping on top)" })).toBeFocused();
  });

  test("update=1: the × on the banner is Later", async ({ page }) => {
    await openWidget(page, { params: { update: "1" } });
    await page.getByRole("button", { name: "Later: hide until a newer version" }).click();
    await expect(page.locator(".update")).toHaveCount(0);
  });

  test("update=1 on the pill: the short banner opens the card with the list", async ({ page }) => {
    await openWidget(page, { view: "pill", params: { update: "1" } });
    await page.getByRole("button", { name: "3 updates" }).click();
    await expect(page.locator(VIEW_ROOT.card)).toBeVisible();
    await expect(page.getByRole("dialog", { name: "Updates available" })).toBeVisible();
  });

  test("update=one: a single version", async ({ page }) => {
    await openWidget(page, { params: { update: "one" } });
    await page.getByRole("button", { name: "Update available: v0.2.0" }).click();
    await expect(page.getByRole("dialog").getByText("Update available", { exact: true })).toBeVisible();
  });
});
