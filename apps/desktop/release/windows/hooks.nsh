; Hooks into the bundler's NSIS installer (`installerHooks` in ../tauri.windows.json).
;
; The installer is per user (`installMode: currentUser`, ADR-0021 and ADR-0030): it installs
; into %LOCALAPPDATA%\Iamb with no administrator rights. Besides the UI host it carries the
; service, iamb-service.exe, next to iamb-desktop.exe, which is where the UI host looks for it
; when it registers the login item (iamb-platform-windows, login_item.rs).
;
; - Before files are replaced or removed, a running service from this user's installation is
;   stopped, or its executable could not be overwritten (Windows locks a running .exe). The
;   bundler already handles a running iamb-desktop.exe itself.
; - After an install or update, if the user had turned on "start at login" (the Run value
;   `Iamb`), the supervisor is started again, so expansion resumes without a new login.
; - On uninstall the Run value is removed, so nothing points at a deleted program. Because an
;   upgrade from the installer's pages runs the old uninstaller first (without /UPDATE), the
;   value is not simply deleted: it is moved to `HKCU\Software\Iamb Setup`, and the next install
;   puts it back pointing at the new iamb-service.exe. After a real uninstall that one value is
;   all that stays behind (docs/release.md).

!define IAMB_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define IAMB_SETUP_KEY "Software\Iamb Setup"

!macro IAMB_STOP_SERVICE
  ; Without administrator rights only this user's processes can be stopped; /T takes the
  ; worker the supervisor started with it. Nothing running is not an error.
  Push $0
  nsExec::Exec 'taskkill /F /T /IM iamb-service.exe'
  Pop $0
  Pop $0
  Sleep 500
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro IAMB_STOP_SERVICE
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  Push $1
  ClearErrors
  ReadRegStr $0 HKCU "${IAMB_RUN_KEY}" "Iamb"
  ${If} $0 == ""
    ; Turned on before an upgrade that uninstalled the old version first: put it back.
    ReadRegStr $1 HKCU "${IAMB_SETUP_KEY}" "StartAtLogin"
    ${If} $1 == "1"
      WriteRegStr HKCU "${IAMB_RUN_KEY}" "Iamb" '"$INSTDIR\iamb-service.exe" --supervise'
      StrCpy $0 "restored"
    ${EndIf}
  ${EndIf}
  DeleteRegKey HKCU "${IAMB_SETUP_KEY}"
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$INSTDIR\iamb-service.exe"
    Exec '"$INSTDIR\iamb-service.exe" --supervise'
  ${EndIf}
  Pop $1
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro IAMB_STOP_SERVICE
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; During an update through the updater (/UPDATE) the login item stays as it is.
  Push $0
  ${IfNot} $UpdateMode = 1
    ClearErrors
    ReadRegStr $0 HKCU "${IAMB_RUN_KEY}" "Iamb"
    ${If} $0 != ""
      WriteRegStr HKCU "${IAMB_SETUP_KEY}" "StartAtLogin" "1"
      DeleteRegValue HKCU "${IAMB_RUN_KEY}" "Iamb"
    ${EndIf}
  ${EndIf}
  Pop $0
!macroend
