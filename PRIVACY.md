# Privacy

Mikyas is **token-free** and **offline by default**:

- It never reads `~/.claude/.credentials.json`, `~/.claude.json`, Claude Desktop's `config.json`
  or `claude_desktop_config.json`, cookies, `Local Storage`, `IndexedDB`, `Session Storage`,
  keychains or any other credential or browser-storage file. These names are hard-denied in code
  (`crates/core/src/saferead.rs`) even inside otherwise allowed folders.
- It makes **no network calls unless you enable the update check** (no Anthropic API, no
  claude.ai, no telemetry). With **Settings → System → Check for updates daily** on (off by
  default), it asks `https://api.github.com/repos/MarawanEldeib/mikyas/releases?per_page=30`
  (the newest 30 releases) at most once a day, plus whenever you click **Check now** (which works
  even with the daily check off). The request is made by Windows' own
  `%SystemRoot%\System32\curl.exe`; the app links no HTTP client. It sends only the app version
  (in the User-Agent header) — no account, token, usage data or identifier — and uses only each
  release's version tag, page address, draft / pre-release flags and up to five short bullet
  lines of its release notes from the answer. **Update** opens the newest release page in your
  default browser; no address other than this repository's release pages can be opened.
- The window is drawn by **Microsoft Edge WebView2**, a component of Windows. On computers
  signed in to a Microsoft account, WebView2's own `msedgewebview2.exe` processes may contact
  Microsoft services (for example at startup) — this is Microsoft's runtime, the same as in every
  other app that uses it, not Mikyas: `mikyas.exe` itself opens no connections, and none of
  your usage data or files is ever handed to WebView2 for sending.
- The UI has no file-system, shell or HTTP access; it can only call the app's own commands.
- **Fullscreen auto-hide** only asks Windows which window is in front, its size, class name and
  monitor, and whether the shell reports a fullscreen / presentation state. It never opens other
  processes, reads their memory, injects code or installs hooks; the class name is compared in
  memory and never stored.
- Nothing leaves your machine. Everything it stores stays in the folders listed below.

## Files read (read-only)

All reads of Claude Code's and Claude Desktop's files go through one allowlist; anything not
listed here cannot be opened.

| File | What is used |
|---|---|
| `%APPDATA%\Claude\plan-usage-history.json` (and MSIX copies under `%LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\Claude\`) | per-sample time and the 5-hour / weekly percentages. The `org` identifier in that file is only compared in memory (to keep one account's samples) and is never stored, logged or shown. |
| `%APPDATA%\Claude\claude-code-sessions\**\local_*.json` (Desktop Code tab) | only `cliSessionId`, `model`, `lastFocusedAt`, `lastActivityAt`. The account/org folder names are not stored. |
| `~/.claude/projects/**/*.jsonl` (Claude Code transcripts; `CLAUDE_CONFIG_DIR` is honoured) | the last 256 KiB (1 MiB if needed) and the first 64 KiB (512 KiB if needed) of recent files. Only these fields of assistant lines: `type`, `isSidechain`, `sessionId`, `entrypoint`, `timestamp`, `cwd` (folder name only, shown only if you enable "Show project name"), `message.model` and `message.usage` token counts; plus the 1M-context marker of the model identity line. **Message content is never parsed or kept.** Files under `subagents` folders are ignored. |
| `%APPDATA%\Claude\local-agent-mode-sessions\**\.claude\projects\**\*.jsonl` (Cowork transcripts) | same as above. Other files in that tree — e.g. a session's `.claude\history.jsonl` prompt history — are not readable. |
| `~/.claude/settings.json` | read only for its `statusLine` entry: to show it, to Connect / Disconnect, and — while **Warn if the connection breaks** is on (default) — to notice when something else rewrites it. Its folder is watched for changes to this file; the watchdog keeps only a fingerprint of the status-line command, never its text. The file is **written** only when you click Connect or Disconnect (or run `--disconnect`), and once on the first start after moving from SovaWatch or Claude Usage Widget, when only the old helper's path in `statusLine.command` is replaced with Mikyas' (a backup is kept first). |
| `%LOCALAPPDATA%\Mikyas\**` | the widget's own data (below). |
| `%LOCALAPPDATA%\SovaWatch\` or `%LOCALAPPDATA%\ClaudeUsageWidget\` | only once, on the first start after moving from SovaWatch or Claude Usage Widget (the app's former names), and only one of the two: the one whose helper your status line runs, else `SovaWatch` if it has settings, else `ClaudeUsageWidget`. From that folder, the settings, state, history, alerts, window positions, captures and connection record (`wrap.json`, re-pointed at the new helper) are copied into `%LOCALAPPDATA%\Mikyas\`. The old folder is not deleted; its `wrap.json` is renamed `wrap.json.migrated` once the status line has been switched to the new helper. |

## Files written

`%LOCALAPPDATA%\Mikyas\` (or `MIKYAS_DATA_DIR`):

| File | Contents |
|---|---|
| `capture\<session_id>.json` | written by the capture helper after you Connect, one per Claude Code session, deleted after 7 days: session id, write/change times, a change fingerprint, model id and display name, context-window used % / size / "exceeds 200k" flag, each rate-limit window's used % and reset time (except `spend_limit`, which is dropped), total API duration. Everything else in the status-line JSON (`transcript_path`, Claude Code version, working directory, workspace, cost, output style, …) is dropped. |
| `capture\_errors.log`, `capture\_diag.log` | at most one short line per helper failure / diagnostic run (names only, never values), size-capped. |
| `history.jsonl` | usage history for sparklines, burn rate, reset estimation, the History view and the weekly recap, kept 14 days: time, window (`5h`, `7d`, …), %, reset time, source (`cli`/`desktop`), "estimated" flag. |
| `state.json` | newest exact reset time per window, the newest Desktop sample already copied into the history, learned model display names (e.g. `claude-opus-5-5 → Opus 5.5`), last maintenance time, and which notifications were already shown so a restart does not repeat them: context-% thresholds per session (keyed by an opaque hash of the session id), pace / heads-up alerts per limit window, and the last weekly window recapped. |
| `alerts.json` | which usage thresholds (e.g. 80% / 95%) already fired for the current window, so alerts fire once. |
| `positions.json` | while **Settings → Automations → Remember position per display** is on (default): for up to 16 monitor setups, a signature of the setup (monitor positions, sizes and scale factors) and the widget's window rectangle and last-used time there. |
| `watchdog.json` | while **Settings → Automations → Warn if the connection breaks** is on (default): fingerprints of status-line changes you dismissed (newest 32) and of the one already warned about — never the command itself. |
| `settings.json` | your widget settings. |
| `update-check.json` | only if you use the update check: time of the last successful check, the newest version already announced, the newer releases that check found (version, release page and those short notes, so the notice survives a restart) and the version you chose **Later** for. |
| `wrap.json` | after Connect: your original status-line command, so Disconnect can restore it exactly. |
| `backups\settings-<time>.json` | a copy of `~/.claude/settings.json` before each Connect/Disconnect edit (newest 3 kept, none older than 30 days). |
| `bin\mikyas-capture.exe` | the capture helper your status-line command points to. |
| `migrated.json` | after moving from SovaWatch or Claude Usage Widget: which of the two it moved from, when the move happened and whether the status line was switched to the new helper (if not, the error message), so the move runs and is announced only once. |

The installer puts the app itself (`mikyas.exe`, `mikyas-capture.exe`, `THIRD_PARTY_NOTICES.md`, `uninstall.exe`) in the same `%LOCALAPPDATA%\Mikyas\` folder.

Files are written atomically through a short-lived `.tmp` file next to them. If `history.jsonl`
cannot be opened, the widget shows no history until it can (nothing is written anywhere else).
Connect's self-test writes its test capture to a temporary folder.

Other locations:

- `%APPDATA%\io.github.marawaneldeib.mikyas\.window-state.json` — the widget's screen
  position.
- `%LOCALAPPDATA%\io.github.marawaneldeib.mikyas\EBWebView\` — the WebView2 profile
  used to render the widget UI (contains no Claude data; the UI keeps nothing in its local
  storage).
- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` — only if you enable "Start with Windows".

## Uninstalling

The uninstaller (Windows Settings → Apps) restores your status line first — it runs
`mikyas.exe --disconnect --quiet` — and removes the "Start with Windows" entry. Ticking
**"Delete the application data"** also removes `%APPDATA%\io.github.marawaneldeib.mikyas`,
`%LOCALAPPDATA%\io.github.marawaneldeib.mikyas` and, once the status line no longer
points at the helper inside it, `%LOCALAPPDATA%\Mikyas`. Updating to a newer version
keeps all of this. If you remove the app by hand instead, Disconnect first (tray → Disconnect
Claude Code, or `mikyas.exe --disconnect --quiet`), then delete the folders above.
