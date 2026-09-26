; Claude Usage Widget: NSIS installer hooks (tauri.conf.json → bundle.windows.nsis.installerHooks).
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
; When the statusline can't be restored in a real uninstall, a message names the file and the
; statusLine entry to remove, and offers Abort (keep the widget installed and connected), Retry
; and Ignore (uninstall anyway; a silent uninstall ignores). A real uninstall always removes the
; widget's copies of Claude Code's settings (backups\) and wrap.json, which hold nothing else.

Var CuwReinstall      ; 1 = update/reinstall: keep the statusline connection and autostart
Var CuwAutostart      ; the "Start with Windows" command line, restored after a reinstall
Var CuwDisconnected   ; 1 = the statusline was restored (or there was nothing to restore)

!define CUW_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define CUW_STARTUP_APPROVED_KEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
; The widget's own data folder (captures, history, settings, the statusline shim).
!define CUW_DATA_DIR "$LOCALAPPDATA\ClaudeUsageWidget"

!macro NSIS_HOOK_PREUNINSTALL
  StrCpy $CuwReinstall 0
  StrCpy $CuwDisconnected 0
  ${If} $UpdateMode = 1
  ${OrIf} ${FileExists} "$EXEDIR\${MAINBINARYNAME}.exe"
    StrCpy $CuwReinstall 1
  ${EndIf}

  ${If} $CuwReinstall = 1
    ReadRegStr $CuwAutostart HKCU "${CUW_RUN_KEY}" "${PRODUCTNAME}"
  ${Else}
    ; The template only asks to close a running widget after this hook. Ask first: cancelling
    ; that question aborts the uninstall, which must leave the widget connected and autostarting.
    !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
    DetailPrint "Restoring the Claude Code statusline..."
    ; Exits 0 when restored or when it was never connected.
    cuw_disconnect:
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --disconnect --quiet' $0
    ${If} ${Errors}
      StrCpy $0 "not run"
    ${EndIf}
    ${If} $0 == 0
      StrCpy $CuwDisconnected 1
    ${Else}
      DetailPrint "The statusline could not be restored automatically (exit code $0)."
      MessageBox MB_ABORTRETRYIGNORE|MB_ICONEXCLAMATION \
        "The Claude Code statusline could not be restored automatically (exit code $0).$\r$\n$\r$\nIn $PROFILE\.claude\settings.json, remove the $\"statusLine$\" entry whose command runs$\r$\n${CUW_DATA_DIR}\bin\cuw-capture.exe$\r$\n(or put your own statusline command back).$\r$\n$\r$\nAbort keeps the widget installed, Retry tries again, Ignore uninstalls anyway." \
        /SD IDIGNORE IDRETRY cuw_disconnect IDIGNORE cuw_disconnect_ignored
      Abort "Uninstall cancelled: the Claude Code statusline still uses the widget."
      cuw_disconnect_ignored:
      DetailPrint "Left the statusLine in $PROFILE\.claude\settings.json; remove it by hand."
    ${EndIf}
    DeleteRegValue HKCU "${CUW_RUN_KEY}" "${PRODUCTNAME}"
    DeleteRegValue HKCU "${CUW_STARTUP_APPROVED_KEY}" "${PRODUCTNAME}"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $CuwReinstall = 1
  ${AndIf} $CuwAutostart != ""
    WriteRegStr HKCU "${CUW_RUN_KEY}" "${PRODUCTNAME}" $CuwAutostart
  ${EndIf}

  ; A real uninstall always removes the copies of Claude Code's settings and the connection
  ; record; the shim (still in use if the restore was ignored) and the history stay.
  ${If} $CuwReinstall = 0
    SetShellVarContext current
    Delete "${CUW_DATA_DIR}\wrap.json"
    RMDir /r "${CUW_DATA_DIR}\backups"
  ${EndIf}

  ; "Delete the application data" also removes the widget's own folder — but only once the
  ; statusline no longer points at the shim inside it.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $CuwReinstall = 0
  ${AndIf} $CuwDisconnected = 1
    SetShellVarContext current
    RMDir /r "${CUW_DATA_DIR}"
  ${EndIf}
!macroend
