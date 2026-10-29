; NSIS hooks for the Windows bundle (bundle.windows.nsis.installerHooks).
;
; One job: wait for a previous FmS to let go of the executable before Section Install
; asks Restart Manager to shut it down.
;
; tauri-plugin-updater hands the downloaded installer to Windows and quits the app in the
; same breath (`ShellExecuteW`, then `std::process::exit`), while a real session takes one
; to two seconds to tear down (WebView2 unload, logger flush). When the `RmShutdown` in
; Section Install's `CheckIfAppIsRunning` lands inside that window, Restart Manager answers
; `ERROR_FAIL_SHUTDOWN` and the section aborts with "Failed to kill FmS. Please close it
; first then try again" — although the app is already gone, which is exactly why the advice
; cannot be followed. The race is lost or won by a few hundred milliseconds, so it shows up
; on some machines and not others.
;
; So poll until the executable can be opened for writing before that check runs: the loader
; keeps a running image open without write sharing, so a successful append-open means the
; old process has really exited. Bounded at 10 s — a process that hangs rather than exits is
; left to Restart Manager, which force-shuts it down. Only in `/UPDATE` mode: a plain
; Setup.exe over a live app is killed by Restart Manager without trouble.

!macro NSIS_HOOK_PREINSTALL
  ${If} $UpdateMode = 1
    ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
      StrCpy $R8 0
      ${Do}
        ClearErrors
        FileOpen $R9 "$INSTDIR\${MAINBINARYNAME}.exe" a
        ${If} ${Errors}
          Sleep 200
          IntOp $R8 $R8 + 1
        ${Else}
          FileClose $R9
          ${ExitDo}
        ${EndIf}
      ${LoopUntil} $R8 >= 50
    ${EndIf}
  ${EndIf}
!macroend
