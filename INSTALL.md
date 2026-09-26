# Installing Claude Usage Widget

A small always-on-top widget for Windows 10/11 that shows your Claude 5-hour and weekly usage,
when each limit resets, and the model and context % of your latest Claude Code session. It reads
files that Claude Code and Claude Desktop already keep on your PC. **It never asks for your
Claude account, password or token, and never reads your Claude login.**

## 1. Download

Open the [Releases page](https://github.com/MarawanEldeib/claude-usage-widget/releases) and
download `Claude.Usage.Widget_<version>_x64-setup.exe`.

Optional: check the download against `SHA256SUMS` from the same release. In PowerShell:

```powershell
Get-FileHash "$env:USERPROFILE\Downloads\Claude.Usage.Widget_*_x64-setup.exe" -Algorithm SHA256
```

The hash must match the line in `SHA256SUMS`.

## 2. Run the installer

The installer is not code-signed yet, so Windows SmartScreen may show **"Windows protected your
PC"**. Click **More info → Run anyway**.

It installs for your Windows user only (no administrator rights) into
`%LOCALAPPDATA%\Claude Usage Widget`, and adds a Start menu entry. The widget needs the WebView2
runtime, which Windows 10/11 already include; if it is missing, the installer downloads it from
Microsoft.

## 3. First run

The widget appears in the bottom-right corner, with an icon in the system tray (click it to show
or hide the widget; right-click for the menu). Point at the widget for its minimize and close
buttons in the top-right corner (close hides it to the tray; Settings → System → **Close button**
can make it quit instead), or right-click it for a menu much like the tray's.

It fills in by itself from two sources — use either or both:

- **Claude Desktop:** open it once. Its usage history is picked up automatically (updated about
  every 15 minutes while Desktop runs; reset times are estimated and shown with "~").
- **Claude Code (exact numbers):** open **Settings** (the sliders icon) → **Claude Code** →
  **Connect**. You first see a preview of the one change it makes: your `statusLine` command in
  `~/.claude/settings.json` gets a tiny pass-through in front of it, so your statusline looks
  exactly as before. A backup of the file is saved first, and **Disconnect** undoes the change
  exactly.

Handy defaults:

- **Ctrl+Alt+H** shows / hides the widget; **Ctrl+Alt+U** makes it click-through ("ghost" mode).
  Both can be changed or removed in Settings.
- While a fullscreen game or app is in front on the widget's screen, the widget hides itself and
  comes back when you leave it. It only checks which window is in front — no process access or
  hooks — so it is safe next to anti-cheat such as Riot Vanguard.
- **Start with Windows** is off until you switch it on (Settings or the tray menu).

## What it reads (and what it never touches)

- Claude Code's statusline data (after Connect), Claude Desktop's usage history file and the
  token counts of your recent Claude Code / Cowork sessions. Message text is never read.
- It never opens `~/.claude/.credentials.json`, cookies, browser storage or keychains.
- No network access at all — unless you switch on **Settings → System → Check for updates
  daily**. Then, once a day, it asks GitHub (api.github.com) whether a newer release exists, using
  Windows' own `curl.exe`. Nothing about you or your usage is sent.

Everything is listed in [PRIVACY.md](PRIVACY.md).

## Updating

Download the new installer and run it. Keep the default **"Uninstall before installing"**: your
settings, history, Claude Code connection and "Start with Windows" are all kept.

## Uninstalling

**Windows Settings → Apps → Installed apps → Claude Usage Widget → Uninstall.**

The uninstaller first restores your original Claude Code statusline (the same as Disconnect) and
removes the "Start with Windows" entry. Tick **"Delete the application data"** to also remove the
widget's data folder `%LOCALAPPDATA%\ClaudeUsageWidget` (history, settings, backups).

If your `~/.claude/settings.json` contains comments, the uninstaller cannot edit it safely and
leaves it alone (the data folder is then kept too, because your statusline still uses the helper
inside it). Remove the `cuw-capture.exe … |` part from `statusLine.command` by hand, or click
**Disconnect** in the widget before uninstalling.

## Troubleshooting

- **A shortcut shows "could not be registered":** another app uses it. Click the field in
  Settings and press a different combination.
- **No numbers yet:** open Claude Desktop once, or Connect Claude Code and send a message.
- **"Accounts may differ":** Claude Desktop and Claude Code are signed in to different accounts.
