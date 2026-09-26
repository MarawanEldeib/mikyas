// App end-to-end test: starts a DEBUG build of the real app (Rust + WebView2) against synthetic
// fixtures in a temp dir and drives it over the Chrome DevTools Protocol.
//
// Usage (Windows):
//   npm run build                               # the frontend the exe embeds (dist/)
//   cargo build -p claude-usage-widget --features tauri/custom-protocol
//                                               # needs src-tauri/binaries/cuw-capture-*.exe
//   npm install --no-save playwright-core       # unless it is already a devDependency
//   node scripts/app-e2e.mjs
//
// The feature matters: a plain `cargo build` compiles Tauri with cfg(dev), and that exe loads
// the Vite dev server (devUrl) instead of the embedded frontend. The test fails on such a build.
//
// Env:
//   CUW_APP_EXE        the debug exe (default <CARGO_TARGET_DIR or ./target>/debug/claude-usage-widget.exe)
//   CUW_E2E_KEEP=1     keep the temp dir afterwards (its path is printed)
//
// What it checks:
//   - the window appears and renders the fixture's 5-hour % and model
//   - every view can be switched to through the app's own `set_view` command
//   - the private working set of the app + WebView2 process tree stays under 150 MB
//   - the process tree holds no established TCP connection except the test's own DevTools socket
//   - `quit_app` ends the app and all of its WebView2 processes with exit code 0
//   - the real %LOCALAPPDATA%\ClaudeUsageWidget listing (names only) is unchanged
//
// Safety:
//   - The app is single-instance. If any claude-usage-widget process is running (e.g. the user's
//     installed widget) the test SKIPS (exit 0) and never stops it. The check runs right before
//     the launch; a widget started during the test would hand over to the test instance.
//   - Widget data (CUW_DATA_DIR) and Claude Code data (CLAUDE_CONFIG_DIR) point at the temp dir;
//     WEBVIEW2_USER_DATA_FOLDER keeps the WebView2 profile there too (verified below).
//   - There is no override for Claude Desktop's folders, so the app still READS (never writes)
//     the machine's Claude Desktop data if there is any. The fixture capture is stamped "now" so
//     it is the newest observation; the % assertion reports the source if Desktop won anyway.
//   - The window-state plugin saves the window position under the app identifier on quit
//     (%APPDATA%\<identifier from tauri.conf.json>\.window-state.json, shared with the installed
//     widget). The file is saved before the launch and restored byte for byte after.
//   - Debug builds only honour CUW_BROWSER_ARGS (the remote debugging port); release builds
//     ignore it.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmdirSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { CTX_SIZE, FIVE_HOUR_PCT, MODEL_NAME, writeFixtures } from "./app-e2e/fixtures.mjs";
import { pageKind } from "./app-e2e/page.mjs";
import { anyAlive, inspectTree, killTree, listNames, pidsNamed, unexpectedConnections } from "./app-e2e/win.mjs";

const APP_NAME = "claude-usage-widget";
const MAX_PRIVATE_MB = 150;
const VIEWS = ["pill", "card", "sessions", "history", "settings", "card"];
// wry's own default WebView2 arguments: CUW_BROWSER_ARGS replaces them, so they are repeated.
const WRY_DEFAULT_ARGS = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const IDENTIFIER = JSON.parse(readFileSync(join(repo, "src-tauri", "tauri.conf.json"), "utf8")).identifier;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mb = (bytes) => Math.round((bytes / 1024 / 1024) * 10) / 10;

function skip(message) {
  console.log(`app-e2e: SKIPPED - ${message}`);
  process.exit(0);
}

class Failure extends Error {}
const failures = [];
function check(ok, message) {
  console.log(`  ${ok ? "ok  " : "FAIL"} ${message}`);
  if (!ok) failures.push(message);
}

async function waitFor(what, fn, timeoutMs = 30_000, everyMs = 250) {
  const end = Date.now() + timeoutMs;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(everyMs);
  }
  throw new Failure(`timed out waiting for ${what}${last ? ` (${last.message})` : ""}`);
}

function freePort() {
  return new Promise((ok, fail) => {
    const server = createServer();
    server.unref();
    server.on("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => ok(port));
    });
  });
}

function appExe() {
  if (process.env.CUW_APP_EXE) return resolve(process.env.CUW_APP_EXE);
  const target = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repo, "target");
  return join(target, "debug", `${APP_NAME}.exe`);
}

/** The user's real widget data dir; capture files churn there whenever Claude Code runs. */
function realDataListing() {
  const local = process.env.LOCALAPPDATA;
  if (!local) return null;
  const dir = join(local, "ClaudeUsageWidget");
  // The statusline helper writes capture\*.json (and *.tmp) for any running Claude Code session,
  // independently of the app, so those entries are not compared.
  return listNames(dir, (rel) => /^capture\\.+/i.test(rel) || /\.tmp$/i.test(rel));
}

function rmdirIfEmpty(dir) {
  try {
    if (existsSync(dir) && readdirSync(dir).length === 0) rmdirSync(dir);
  } catch {
    // left as it is
  }
}

function readIfExists(path) {
  return existsSync(path) ? readFileSync(path) : null;
}

async function main() {
  if (process.platform !== "win32") skip("Windows only (WebView2 + Win32 process checks)");

  const exe = appExe();
  if (!existsSync(exe)) {
    console.error(`app-e2e: debug exe not found: ${exe}\n  build it with: cargo build -p ${APP_NAME}`);
    process.exit(1);
  }
  let chromium;
  try {
    ({ chromium } = await import("playwright-core"));
  } catch {
    console.error("app-e2e: playwright-core is missing\n  install it with: npm install --no-save playwright-core");
    process.exit(1);
  }

  const running = pidsNamed(APP_NAME);
  if (running.length) {
    skip(`${APP_NAME} is already running (pid ${running.join(", ")}); the app is single-instance and the test never stops it`);
  }

  const root = mkdtempSync(join(tmpdir(), "cuw-app-e2e-"));
  const { dataDir, claudeDir, webviewDir } = writeFixtures(root);
  const windowState = process.env.APPDATA ? join(process.env.APPDATA, IDENTIFIER, ".window-state.json") : null;
  const windowStateBefore = windowState ? readIfExists(windowState) : null;
  const windowStateDirBefore = windowState ? existsSync(dirname(windowState)) : true;
  const realBefore = realDataListing();
  const port = await freePort();

  console.log(`app-e2e: ${exe}\n  temp dir ${root}\n  devtools port ${port}`);
  const child = spawn(exe, [], {
    env: {
      ...process.env,
      CUW_DATA_DIR: dataDir,
      CLAUDE_CONFIG_DIR: claudeDir,
      WEBVIEW2_USER_DATA_FOLDER: webviewDir,
      CUW_BROWSER_ARGS: `${WRY_DEFAULT_ARGS} --remote-debugging-port=${port}`,
      RUST_BACKTRACE: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: false,
  });
  let output = "";
  child.stdout.on("data", (d) => (output += d));
  child.stderr.on("data", (d) => (output += d));
  const exited = new Promise((ok) => child.on("exit", (code, signal) => ok({ code, signal })));
  let exitInfo = null;
  exited.then((e) => (exitInfo = e));

  let browser;
  let treePids = [child.pid];
  try {
    // ---- window and first render ----
    browser = await waitFor(
      "the DevTools endpoint",
      async () => {
        if (exitInfo) throw new Failure(`the app exited early (${JSON.stringify(exitInfo)})\n${output}`);
        return chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 2_000 });
      },
      60_000,
      500,
    );
    const page = await waitFor("the widget page", () => {
      const pages = browser.contexts().flatMap((c) => c.pages());
      const dev = pages.find((p) => pageKind(p.url()) === "dev");
      if (dev) {
        throw new Failure(
          `the exe loads the dev server (${dev.url()}): build it with --features tauri/custom-protocol`,
        );
      }
      return pages.find((p) => pageKind(p.url()) === "app");
    });
    const invoke = (cmd, args) =>
      page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args ?? {}]);

    await page.waitForFunction((name) => document.body?.innerText.includes(name), MODEL_NAME, { timeout: 30_000 });
    const tree = await waitFor("a visible window", () => {
      const t = inspectTree(child.pid);
      return t.mainWindow ? t : null;
    });
    treePids = tree.pids;
    check(tree.mainWindow !== 0, "the widget window is visible");

    const snap = await invoke("get_snapshot");
    const five = snap.windows.find((w) => w.state.kind === "five_hour") ?? snap.windows[0];
    check(
      five && Math.round(five.state.pct) === FIVE_HOUR_PCT,
      `5-hour window is ${FIVE_HOUR_PCT}% (got ${five?.state.pct} from ${JSON.stringify(five?.state.source)})`,
    );
    check(snap.session?.display_name === MODEL_NAME, `session model is ${MODEL_NAME} (got ${snap.session?.display_name})`);
    check(snap.session?.ctx_size === CTX_SIZE, `context size is ${CTX_SIZE} (got ${snap.session?.ctx_size})`);
    const text = await page.evaluate(() => document.body.innerText);
    check(new RegExp(`(^|\\D)${FIVE_HOUR_PCT}(\\D|$)`).test(text), `the card shows ${FIVE_HOUR_PCT}`);
    check(text.includes(`${MODEL_NAME} · 200K`), `the card shows "${MODEL_NAME} · 200K"`);

    // ---- views ----
    for (const view of VIEWS) {
      await invoke("set_view", { view });
      const ui = await waitFor(`view ${view}`, async () => {
        const u = await invoke("get_ui_state");
        return u.view === view ? u : null;
      }, 10_000);
      check(ui.view === view, `set_view switches to ${view}`);
    }
    await page.waitForFunction((name) => document.body?.innerText.includes(name), MODEL_NAME, { timeout: 10_000 });
    check(true, "the card renders again after the round trip");

    // ---- memory and network, once idle ----
    await sleep(8_000);
    const idle = inspectTree(child.pid);
    treePids = idle.pids;
    const privateMb = mb(idle.privateBytes);
    const perProcess = idle.processes.map((p) => `${p.name}(${p.pid})=${mb(p.private)}`).join(", ");
    check(
      privateMb > 0 && privateMb < MAX_PRIVATE_MB,
      `private working set ${privateMb} MB < ${MAX_PRIVATE_MB} MB over ${idle.processes.length} processes [${perProcess}]`,
    );
    const foreign = unexpectedConnections([...tree.tcp, ...idle.tcp], port);
    check(foreign.length === 0, `no TCP connections besides the DevTools port${foreign.length ? `: ${JSON.stringify(foreign)}` : ""}`);

    // ---- isolation ----
    const profile = readdirSync(webviewDir);
    check(profile.length > 0, `the WebView2 profile lives in the temp dir (${profile.length} entries)`);

    // ---- quit ----
    // The app exits inside the command, so the call itself may never answer.
    page.evaluate(() => window.__TAURI_INTERNALS__.invoke("quit_app")).catch(() => {});
    const exit = await Promise.race([exited, sleep(20_000).then(() => null)]);
    check(exit !== null, "quit_app ends the app");
    check(exit?.code === 0, `exit code 0 (got ${JSON.stringify(exit)})`);
    await waitFor("the WebView2 processes to end", () => !anyAlive(treePids), 20_000, 500).catch(() => {});
    check(!anyAlive(treePids), "no process of the tree is left");
  } catch (e) {
    // Any error (a timeout, a failed evaluate) is a failed check: cleanup and the checks
    // below still run.
    check(false, e instanceof Failure ? e.message : String(e?.stack ?? e));
  } finally {
    await browser?.close().catch(() => {});
    if (!exitInfo) killTree(child.pid);
    // Restore the window position the installed widget saved (the test run overwrote it).
    if (windowState) {
      if (windowStateBefore) writeFileSync(windowState, windowStateBefore);
      else if (existsSync(windowState)) unlinkSync(windowState);
      // The plugin created the identifier dir if it was missing; remove it only while empty.
      if (!windowStateDirBefore) rmdirIfEmpty(dirname(windowState));
    }
  }

  const realAfter = realDataListing();
  const same = JSON.stringify(realBefore) === JSON.stringify(realAfter);
  check(same, `%LOCALAPPDATA%\\ClaudeUsageWidget listing unchanged (${realBefore ? realBefore.length : "absent"})`);
  if (!same) {
    const before = new Set(realBefore ?? []);
    const after = new Set(realAfter ?? []);
    console.log(`    added: ${[...after].filter((n) => !before.has(n)).join(", ") || "-"}`);
    console.log(`    removed: ${[...before].filter((n) => !after.has(n)).join(", ") || "-"}`);
  }
  if (windowState) {
    const now = readIfExists(windowState);
    check(
      (now === null && windowStateBefore === null) || (now && windowStateBefore && now.equals(windowStateBefore)),
      "the saved window position is restored",
    );
  }

  if (process.env.CUW_E2E_KEEP === "1") console.log(`  kept ${root}`);
  else rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });

  if (failures.length) {
    console.error(`app-e2e: ${failures.length} check(s) failed`);
    if (output.trim()) console.error(`--- app output ---\n${output.trim()}`);
    process.exit(1);
  }
  console.log("app-e2e: all checks passed");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
