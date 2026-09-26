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
    ; Exits 0 when restored or when it was never connected; the uninstall goes on either way.
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --disconnect --quiet' $0
    ${If} ${Errors}
      DetailPrint "Could not run the statusline restore."
    ${ElseIf} $0 = 0
      StrCpy $CuwDisconnected 1
    ${Else}
      DetailPrint "The statusline could not be restored automatically (exit code $0); see ~/.claude/settings.json."
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

  ; "Delete the application data" also removes the widget's own folder — but only once the
  ; statusline no longer points at the shim inside it.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $CuwReinstall = 0
  ${AndIf} $CuwDisconnected = 1
    SetShellVarContext current
    RMDir /r "${CUW_DATA_DIR}"
  ${EndIf}
!macroend
