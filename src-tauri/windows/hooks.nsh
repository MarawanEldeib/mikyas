; Mikyas: NSIS installer hooks (tauri.conf.json → bundle.windows.nsis.installerHooks).
;
; A real uninstall restores the user's Claude Code statusline (the app's own `--disconnect`
; command) and removes the "Start with Windows" entry. Neither may happen when the uninstaller
; only runs as part of an update or a reinstall, which the widget recognises in two ways:
;
; - $UpdateMode = 1: the uninstaller was started with /UPDATE (Tauri's updater path).
; - The uninstaller runs in place, next to the app. Tauri's installer, when the user keeps its
;   default "uninstall before installing" choice over an older (or the same) version, starts
;   "$INSTDIR\uninstall.exe _?=$INSTDIR" without /UPDATE, and `_?=` makes NSIS run it in place.
;   An uninstall started from Windows Settings (or by running uninstall.exe) has no `_?=`, so
;   NSIS first copies the uninstaller to %TEMP% and runs it from there.
;
; The template's own Section Uninstall deletes the Run value whenever $UpdateMode <> 1; for a
; reinstall it is read here first and written back after the template is done.
;
; When the statusline can't be restored in a real uninstall, a message names the file (the same
; settings.json that `mikyas.exe --disconnect` prints) and the statusLine entry to remove, and
; offers Abort (keep the widget installed and connected), Retry and Ignore (uninstall anyway; a
; silent uninstall ignores). A real uninstall always removes the widget's copies of Claude
; Code's settings (backups\) and wrap.json, which hold nothing else.

Var MikyasReinstall    ; 1 = update/reinstall: keep the statusline connection and autostart
Var MikyasAutostart    ; the "Start with Windows" command line, restored after a reinstall
Var MikyasDisconnected ; 1 = the statusline was restored (or there was nothing to restore)
Var MikyasDataDir      ; the widget's own data folder, see MIKYAS_PATHS
Var MikyasSettings     ; Claude Code's settings.json, see MIKYAS_PATHS

!define MIKYAS_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define MIKYAS_STARTUP_APPROVED_KEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
; $MikyasDataDir: the widget's own data folder (captures, history, settings, the statusline
; shim), named after the product like the app's (mikyas_core::paths::APP_DIR_NAME, which a test
; keeps equal to productName). It is also the default install folder: the template's uninstall
; deletes only the files it installed plus a non-recursive RMDir, so the data stays unless it is
; deleted below.
; $MikyasSettings: Claude Code's settings.json, found the way Claude Code and the app find it:
; in %CLAUDE_CONFIG_DIR% when that is set, else in %USERPROFILE%\.claude.
!macro MIKYAS_PATHS
  StrCpy $MikyasDataDir "$LOCALAPPDATA\${PRODUCTNAME}"
  ReadEnvStr $MikyasSettings CLAUDE_CONFIG_DIR
  ${If} $MikyasSettings == ""
    StrCpy $MikyasSettings "$PROFILE\.claude"
  ${EndIf}
  StrCpy $MikyasSettings "$MikyasSettings\settings.json"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro MIKYAS_PATHS
  StrCpy $MikyasReinstall 0
  StrCpy $MikyasDisconnected 0
  ${If} $UpdateMode = 1
  ${OrIf} ${FileExists} "$EXEDIR\${MAINBINARYNAME}.exe"
    StrCpy $MikyasReinstall 1
  ${EndIf}

  ${If} $MikyasReinstall = 1
    ReadRegStr $MikyasAutostart HKCU "${MIKYAS_RUN_KEY}" "${PRODUCTNAME}"
  ${Else}
    ; The template only asks to close a running widget after this hook. Ask first: cancelling
    ; that question aborts the uninstall, which must leave the widget connected and autostarting.
    !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
    DetailPrint "Restoring the Claude Code statusline..."
    ; Exits 0 when restored or when it was never connected.
    mikyas_disconnect:
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --disconnect --quiet' $0
    ${If} ${Errors}
      StrCpy $0 "not run"
    ${EndIf}
    ${If} $0 == 0
      StrCpy $MikyasDisconnected 1
    ${Else}
      DetailPrint "The statusline could not be restored automatically (exit code $0)."
      MessageBox MB_ABORTRETRYIGNORE|MB_ICONEXCLAMATION \
        "The Claude Code statusline could not be restored automatically (exit code $0).$\r$\n$\r$\nIn $MikyasSettings, remove the $\"statusLine$\" entry whose command runs$\r$\n$MikyasDataDir\bin\mikyas-capture.exe$\r$\n(or put your own statusline command back).$\r$\n$\r$\nAbort keeps the widget installed, Retry tries again, Ignore uninstalls anyway and also deletes the widget's saved copies of settings.json in $MikyasDataDir\backups." \
        /SD IDIGNORE IDRETRY mikyas_disconnect IDIGNORE mikyas_disconnect_ignored
      Abort "Uninstall cancelled: the Claude Code statusline still uses the widget."
      mikyas_disconnect_ignored:
      DetailPrint "Left the statusLine in $MikyasSettings; remove it by hand."
    ${EndIf}
    DeleteRegValue HKCU "${MIKYAS_RUN_KEY}" "${PRODUCTNAME}"
    DeleteRegValue HKCU "${MIKYAS_STARTUP_APPROVED_KEY}" "${PRODUCTNAME}"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  SetShellVarContext current
  !insertmacro MIKYAS_PATHS
  ${If} $MikyasReinstall = 1
  ${AndIf} $MikyasAutostart != ""
    WriteRegStr HKCU "${MIKYAS_RUN_KEY}" "${PRODUCTNAME}" $MikyasAutostart
  ${EndIf}

  ; A real uninstall always removes the copies of Claude Code's settings and the connection
  ; record; the shim (still in use if the restore was ignored) and the history stay.
  ${If} $MikyasReinstall = 0
    Delete "$MikyasDataDir\wrap.json"
    RMDir /r "$MikyasDataDir\backups"
  ${EndIf}

  ; "Delete the application data" also removes the widget's own folder — but only once the
  ; statusline no longer points at the shim inside it.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $MikyasReinstall = 0
  ${AndIf} $MikyasDisconnected = 1
    RMDir /r "$MikyasDataDir"
  ${EndIf}
!macroend
