# Claude Usage Widget

A small floating, always-on-top Windows widget that shows what the Claude Code statusline shows,
even when no Claude Code terminal is open:

- **5-hour limit** usage with a reset countdown
- **Weekly limit** usage with a reset countdown (plus per-model weekly limits when reported)
- **Current model** and **context-window %** of your most recently active Claude Code / Cowork session
- Extras: 80 % / 95 % / reset notifications, a burn-rate forecast ("at this pace you hit 100 % at
  15:40"), 24 h / 7 d sparklines, a compact pill view, a click-through "ghost" mode toggled with
  a global hotkey (default **Ctrl+Alt+U**), a show / hide hotkey (default **Ctrl+Alt+H**), and
  automatic hiding while a fullscreen video, course or app is in front

**Install:** download the installer from the
[Releases page](https://github.com/MarawanEldeib/claude-usage-widget/releases) — see
[INSTALL.md](INSTALL.md) for the SmartScreen prompt, first run and uninstalling.

It runs beside Claude Desktop, sits in the tray, and uses little memory (WebView2 is asked to keep
its memory use low; see *Performance*).

## Privacy: token-free by design

The widget **never** reads `~/.claude/.credentials.json`, cookies, browser storage or keychains,
**never** calls `api.anthropic.com`, claude.ai or `claude -p /usage`, and makes **no network calls
unless you enable the update check**; then it asks only `api.github.com` (once a day, or when you
click "Check now") whether a newer release exists, through Windows' own `curl.exe` — the app
itself contains no HTTP client. Every file it reads goes through an allowlist (`crates/core/src/saferead.rs`) that
hard-denies credential, cookie and browser-storage files. Each person only ever sees their own
numbers, computed from files already on their own machine:

| Source | What it gives |
|---|---|
| Claude Code statusline JSON (via the optional capture shim, see *Connect*) | exact 5 h / 7 d %, exact reset times, model, context % |
| Claude Desktop's `%APPDATA%\Claude\plan-usage-history.json` | 5 h / 7 d % (whole numbers, every ~15 min while Desktop runs); reset times are estimated and shown with "~" |
| Claude Code / Cowork transcripts (`~/.claude/projects/**/*.jsonl`) | model and context size of the latest session (message text is never parsed or kept) |

[PRIVACY.md](PRIVACY.md) lists every file read and written.

Model and context % of plain Claude Desktop *chat* conversations are not stored anywhere locally,
so the widget shows the most recent Code / CLI / Cowork session instead (with its age).

## Build

Requirements: Windows 10/11, Rust (see `rust-toolchain.toml`), Node 20+, VS Build Tools (C++),
WebView2 runtime.

```powershell
npm ci
.\scripts\build-sidecar.ps1          # builds cuw-capture.exe → src-tauri\binaries\ (required once, and after shim changes)
npx tauri build --no-bundle          # → <target>\release\claude-usage-widget.exe (+ cuw-capture.exe next to it)
npx tauri build                      # per-user NSIS installer → <target>\release\bundle\nsis\*-setup.exe
```

### Installer

`npx tauri build` makes a per-user NSIS installer (no admin rights; installs to
`%LOCALAPPDATA%\Claude Usage Widget`) that ships `cuw-capture.exe` as a sidecar. Its hooks
(`src-tauri/windows/hooks.nsh`) make a real uninstall run `claude-usage-widget.exe --disconnect
--quiet`, which restores the user's Claude Code statusline, and remove the "Start with Windows"
entry; with "Delete the application data" ticked, the widget's data folder goes too (only once the
statusline no longer points at the shim in it). When the uninstaller only runs as part of an update
or reinstall — started with `/UPDATE`, or in place by a newer installer's "uninstall before
installing" step — the connection and autostart are kept.

Pushing a `v*` tag runs `.github/workflows/release.yml`: it builds the installer with
`tauri-apps/tauri-action` and creates a **draft** GitHub release with the installer and
`SHA256SUMS`. The versions in `package.json`, `Cargo.toml` and `src-tauri/tauri.conf.json` must
equal the tag. `.github/workflows/ci.yml` runs gitleaks, clippy, the Rust and UI tests,
svelte-check, the UI build and a check that no HTTP client crate entered the dependency tree.

Tests: `cargo test -p cuw-core -p cuw-capture -p claude-usage-widget`,
`cargo clippy --workspace --all-targets -- -D warnings`, `npm test`, `npm run check`.
The UI can be developed in a plain browser with a mock backend: `npm run dev`, then open
`http://localhost:1420/?scenario=normal&view=card`.

Development overrides: `CUW_DATA_DIR` (widget data dir), `CLAUDE_CONFIG_DIR` (Claude Code dir, as
Claude Code itself honours it), `CUW_X` / `CUW_Y` (initial window position in physical pixels), `CUW_MEMORY_NORMAL` (skip the
WebView2 low memory target), `CUW_BROWSER_ARGS` (replace the WebView2 browser arguments; repeat
wry's defaults `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`).

## Connect Claude Code (optional, but gives exact numbers)

Without connecting, the widget already works from Claude Desktop's usage file and the transcripts.
**Connect** (Settings → Claude Code) makes the numbers exact and live:

1. The widget shows a preview of the change first.
2. It copies the tiny capture shim `cuw-capture.exe` to `%LOCALAPPDATA%\ClaudeUsageWidget\bin\`.
3. It changes only `statusLine.command` in `~/.claude/settings.json` into
   `"<shim>" --tee | <your original command>` (the exact form depends on the shell Claude Code uses
   on your machine). Every other byte of the file stays as it was; a backup goes to
   `%LOCALAPPDATA%\ClaudeUsageWidget\backups\`.
4. The shim passes Claude Code's statusline JSON through to your own statusline **unchanged**, so it
   looks exactly as before, and saves a few whitelisted numbers (no prompts, no paths other than the
   transcript path, no credentials) to `%LOCALAPPDATA%\ClaudeUsageWidget\capture\`.
5. A self-test runs your original and the wrapped command on the same sample input and compares
   their output.

With no statusline configured, the shim prints a compact default line instead. **Disconnect**
(tray menu or Settings) restores the original command byte-for-byte;
`claude-usage-widget.exe --disconnect --quiet` does the same from the command line (for the
uninstaller hook).

## Notes

- **Fullscreen auto-hide** (Settings → System, on by default): the widget hides while a fullscreen
  video, course or app is in front. Every 1.5 s it asks Windows which window is in front
  (`GetForegroundWindow`, `GetWindowRect`, `MonitorFromWindow` / `GetMonitorInfoW`,
  `GetClassNameW`) and whether the shell reports a fullscreen / presentation state
  (`SHQueryUserNotificationState`). When a fullscreen app covers the monitor the widget is
  on, the widget hides, and it comes back — without taking the focus — once that app leaves the
  foreground. Window queries only: the widget never reads, injects into or hooks other programs. A
  widget you hid yourself stays hidden, and one you brought back with the show / hide hotkey while
  something is fullscreen stays visible.
- **Update check** (Settings → System, off by default): see *Privacy*. A newer release shows a
  dismissible "Update vX.Y.Z available · View" line on the widget and one notification; "View"
  opens the release page in your browser (only this repository's release pages can be opened).
- **SmartScreen:** the installer is not code-signed yet, so Windows may show "Windows protected your
  PC" on first run (More info → Run anyway).
- **Backdrop effects:** Mica/Acrylic/Blur are selectable, but because the widget never takes focus,
  Windows renders Mica and Acrylic flat; the default is a near-opaque surface ("none").
- Data goes stale when neither Claude Code nor Claude Desktop is running; stale values are greyed
  out with their age.

## Performance

Measured on the development laptop (Ryzen 7 5800H, 16 threads, 125 % scaling) with real data,
**idle with the display locked** (WebView2 was not compositing, so expect somewhat more while the
widget is visible): idle CPU for the app plus its WebView2 processes ≈ 0.03–0.06 % of all cores;
private working set ≈ 20–45 MB with WebView2's low memory target (83–99 MB without it; commit
≈ 130 MB either way). Set `CUW_MEMORY_NORMAL=1` to keep WebView2's normal memory target. Measure
with `scripts\measure-ram.ps1`.

## Layout

- `crates/core` — token-free parsers and the engine (merge, reset estimation, burn rate, context,
  history, alerts, Connect/Disconnect file edits). No Tauri; most tests live here.
- `crates/capture` — `cuw-capture.exe`, the statusline tee shim.
- `src-tauri` — the app shell: pipeline thread, file watchers, window, tray, hotkeys, fullscreen
  auto-hide, the opt-in update check, notifications, and the NSIS installer hooks
  (`src-tauri/windows/hooks.nsh`).
- `src` — Svelte 5 UI.
