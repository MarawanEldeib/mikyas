# Installing Mikyas

Step-by-step install, update and uninstall guide for Windows 10/11. For what the widget does, see
the [README](README.md); for every file it reads and writes, see [PRIVACY.md](PRIVACY.md).

## 1. Download

Open the [Releases page](https://github.com/MarawanEldeib/mikyas/releases) and
download `Mikyas_<version>_x64-setup.exe`.

Optional: check the download against `SHA256SUMS` from the same release. In PowerShell:

```powershell
Get-FileHash "$env:USERPROFILE\Downloads\Mikyas_*_x64-setup.exe" -Algorithm SHA256
```

The hash must match the line in `SHA256SUMS`.

## 2. Run the installer

The installer is not code-signed yet, so Windows SmartScreen may show **"Windows protected your
PC"**. Click **More info → Run anyway**.

It installs for your Windows user only (no administrator rights) into
`%LOCALAPPDATA%\Mikyas`, and adds a Start menu entry. The widget needs the WebView2
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
  **Connect**. You see a preview of the one change first; **Disconnect** undoes it exactly. See
  [Connect Claude Code](README.md#connect-claude-code) for what it changes.

Handy defaults:

- **Ctrl+Alt+H** shows / hides the widget; **Ctrl+Alt+U** makes it click-through ("ghost" mode).
  Both can be changed or removed in Settings.
- While a fullscreen video, course or app is in front on the widget's screen, the widget hides
  itself and comes back when you leave it (not while ghost mode is on). It only checks which
  window is in front, and never reads, injects into or hooks other programs.
- **Start with Windows** is off until you switch it on (Settings or the tray menu).
- No network access unless you switch on **Settings → System → Check for updates daily**.

## Updating

Download the new installer and run it. Keep the default **"Uninstall before installing"**: your
settings, history, Claude Code connection and "Start with Windows" are all kept.

## Uninstalling

**Windows Settings → Apps → Installed apps → Mikyas → Uninstall.**

The uninstaller first restores your original Claude Code status line (the same as Disconnect) and
removes the "Start with Windows" entry. Tick **"Delete the application data"** to also remove the
widget's data folders (history, settings, backups; the full list is in
[PRIVACY.md](PRIVACY.md#uninstalling)).

If the status line cannot be restored — for example because comments were added to
`~/.claude/settings.json` after you connected — the uninstaller leaves that file alone and keeps
the data folder `%LOCALAPPDATA%\Mikyas`, because your status line still uses the helper
inside it. Remove the `mikyas-capture.exe … |` part from `statusLine.command` by hand, or click
**Disconnect** in the widget before uninstalling.

## Moving from SovaWatch or Claude Usage Widget

Mikyas is the new name of SovaWatch, which was called Claude Usage Widget before that. To move
over:

1. Install Mikyas as above (it installs next to the old app; nothing is removed).
2. Open Mikyas once. On its first start it copies your settings, history, alerts, window positions
   and captures from the old app's folder (`%LOCALAPPDATA%\SovaWatch` or
   `%LOCALAPPDATA%\ClaudeUsageWidget`), and if Claude Code's status line was connected, it
   switches the status line to Mikyas' own helper — only the helper's path changes, and
   **Disconnect** still restores your original status line exactly. The old folder is left in
   place. A notification confirms the move. Do not click **Reconnect** in the old app after this.
3. Uninstall the old app: **Windows Settings → Apps → Installed apps → SovaWatch** (or **Claude
   Usage Widget**) **→ Uninstall.** Its uninstaller leaves Mikyas' status line alone. Ticking
   "Delete the application data" there removes only the old app's folders.

If **Start with Windows** was on in the old app, Mikyas switches it on for itself.

## Troubleshooting

- **A shortcut shows "could not be registered":** another app uses it. Click the field in
  Settings and press a different combination.
- **No numbers yet:** open Claude Desktop once, or Connect Claude Code and send a message.
- **"Accounts may differ":** Claude Desktop and Claude Code are signed in to different accounts.
- **Connect says settings.json is not strict JSON:** your `~/.claude/settings.json` has comments
  or trailing commas. Remove them, then Connect again.
