# Privacy

Claude Usage Widget is **token-free** and **offline by default**:

- It never reads `~/.claude/.credentials.json`, `~/.claude.json`, Claude Desktop's `config.json`,
  cookies, `Local Storage`, `IndexedDB`, `Session Storage`, keychains or any other credential or
  browser-storage file. These names are hard-denied in code (`crates/core/src/saferead.rs`) even
  inside otherwise allowed folders.
- It makes **no network calls unless you enable the update check** (no Anthropic API, no
  claude.ai, no telemetry). With **Settings → System → Check for updates daily** on (off by
  default), it asks `https://api.github.com/repos/MarawanEldeib/claude-usage-widget/releases/latest`
  at most once a day, plus whenever you click **Check now** (which works even with the daily
  check off). The request is made by Windows' own `%SystemRoot%\System32\curl.exe`; the app links
  no HTTP client. It sends only the app version (in the User-Agent header) — no account, token,
  usage data or identifier — and uses only the release's version tag and page address from the
  answer. **View** opens that release page in your default browser; no other address can be
  opened.
- The UI has no file-system, shell or HTTP access; it can only call the app's own commands.
- **Fullscreen auto-hide** only asks Windows which window is in front, its size, class name and
  monitor, and whether the shell reports a fullscreen / presentation state. It never opens other
  processes, reads their memory, injects code or installs hooks; the class name is compared in
  memory and never stored.
- Nothing leaves your machine. Everything it stores stays in the folders listed below.

## Files read (read-only)

All reads go through one allowlist; anything not listed here cannot be opened.

| File | What is used |
|---|---|
| `%APPDATA%\Claude\plan-usage-history.json` (and MSIX copies under `%LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\Claude\`) | per-sample time and the 5-hour / weekly percentages. The `org` identifier in that file is only compared in memory (to keep one account's samples) and is never stored, logged or shown. |
| `%APPDATA%\Claude\claude-code-sessions\**\local_*.json` (Desktop Code tab) | only `cliSessionId`, `model`, `lastFocusedAt`, `lastActivityAt`. The account/org folder names are not stored. |
| `~/.claude/projects/**/*.jsonl` (Claude Code transcripts; `CLAUDE_CONFIG_DIR` is honoured) | the last 256 KiB (1 MiB if needed) and the first 64 KiB of recent files. Only these fields of assistant lines: `type`, `isSidechain`, `sessionId`, `entrypoint`, `timestamp`, `cwd` (folder name only, shown only if you enable "show project"), `message.model` and `message.usage` token counts; plus the 1M-context marker of the model identity line. **Message content is never parsed or kept.** Files under `subagents` folders are ignored. |
| `%APPDATA%\Claude\local-agent-mode-sessions\**\.claude\projects\**\*.jsonl` (Cowork transcripts) | same as above. Other files in that tree — e.g. a session's `.claude\history.jsonl` prompt history — are not readable. |
| `~/.claude/settings.json` | only to show the current `statusLine` and to Connect / Disconnect. It is **written** only when you click Connect or Disconnect (or run `--disconnect`). |
| `%LOCALAPPDATA%\ClaudeUsageWidget\**` | the widget's own data (below). |

## Files written

`%LOCALAPPDATA%\ClaudeUsageWidget\` (or `CUW_DATA_DIR`):

| File | Contents |
|---|---|
| `capture\<session_id>.json` | written by the capture shim after you Connect, one per Claude Code session, deleted after 7 days: session id, write/change times, a change fingerprint, `transcript_path`, model id and display name, context-window used % / size / "exceeds 200k" flag, each rate-limit window's used % and reset time, total API duration, Claude Code version. Everything else in the statusline JSON (working directory, workspace, cost, output style, …) is dropped. Note: `transcript_path` contains your Windows user name as part of the path; it stays on your machine. |
| `capture\_errors.log`, `capture\_diag.log` | at most one short line per shim failure / diagnostic run (names only, never values), size-capped. |
| `history.jsonl` | usage history for sparklines, burn rate and reset estimation, kept 14 days: time, window (`5h`, `7d`, …), %, reset time, source (`cli`/`desktop`), "estimated" flag. |
| `state.json` | newest exact reset time per window, the newest Desktop sample already copied into the history, learned model display names (e.g. `claude-opus-5-5 → Opus 5.5`), last maintenance time. |
| `alerts.json` | which alert thresholds already fired for the current window, so alerts fire once. |
| `settings.json` | your widget settings. |
| `update-check.json` | only if you use the update check: time of the last successful check and the newest version already announced. |
| `wrap.json` | after Connect: your original statusline command, so Disconnect can restore it exactly. |
| `backups\settings-<time>.json` | a copy of `~/.claude/settings.json` before each Connect/Disconnect edit (newest 10 kept). |
| `bin\cuw-capture.exe` | the capture shim your statusline command points to. |

Other locations:

- `%APPDATA%\io.github.marawaneldeib.claude-usage-widget\.window-state.json` — the widget's screen
  position.
- `%LOCALAPPDATA%\io.github.marawaneldeib.claude-usage-widget\EBWebView\` — the WebView2 profile
  used to render the widget UI (contains no Claude data; its local storage only remembers which
  update notice you dismissed).
- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` — only if you enable "Start with Windows".

## Uninstalling

The uninstaller (Windows Settings → Apps) restores your statusline first — it runs
`claude-usage-widget.exe --disconnect --quiet` — and removes the "Start with Windows" entry. Ticking
**"Delete the application data"** also removes `%APPDATA%\io.github.marawaneldeib.claude-usage-widget`,
`%LOCALAPPDATA%\io.github.marawaneldeib.claude-usage-widget` and, once the statusline no longer
points at the shim inside it, `%LOCALAPPDATA%\ClaudeUsageWidget`. Updating to a newer version
keeps all of this. If you remove the app by hand instead, Disconnect first (tray → Disconnect
Claude Code, or `claude-usage-widget.exe --disconnect --quiet`), then delete the folders above.
