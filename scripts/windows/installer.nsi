; Catbot-Setup.exe (stage 29.5): installs Catbot for this Windows user, with no administrator
; needed. Built by scripts/build-windows.sh with NSIS (makensis), from the same files as the zip:
;
;   makensis -DVERSION=0.2.0 -DSRC=dist/catbot-windows -DOUT=dist/Catbot-Setup-0.2.0.exe scripts/windows/installer.nsi
;
; Into %LOCALAPPDATA%\Programs\Catbot:
;   Catbot.exe          the launcher (starts the current version; asks before going back)
;   versions\<version>\ this version of the app; versions already there are kept, so Settings
;                       can go back to them
;   versions.json       written by "Catbot.exe --register <version>"
; Start menu and desktop shortcuts, and an entry in Settings > Apps (Add or remove programs) with
; an uninstaller. Catbot's chats and settings live in %LOCALAPPDATA%\Catbot; uninstalling asks
; before removing those.
;
; Moo's choice, 2026-11-25: "Setup.exe, unsigned for now". Until it's signed, Windows shows
; "Windows protected your PC" the first time: More info, then Run anyway.

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !error "Give the version: -DVERSION=0.2.0"
!endif
!ifndef SRC
  !define SRC "../../dist/catbot-windows"
!endif
!ifndef OUT
  !define OUT "../../dist/Catbot-Setup-${VERSION}.exe"
!endif

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Catbot"
!define WEBVIEW2_KEY "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

Name "Catbot"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\Catbot"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "Catbot ${VERSION}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Catbot"
VIAddVersionKey "FileDescription" "Catbot setup"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Apache-2.0"

!define MUI_ICON "${SRC}/app/catbot.ico"
!define MUI_UNICON "${SRC}/app/catbot.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Catbot ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "Catbot is a team of AI bots you chat with.$\r$\n$\r$\nThis puts Catbot in your Start menu and on your desktop. It installs for you only, so it doesn't need an administrator.$\r$\n$\r$\nIf Catbot is open, it will be closed first."
!define MUI_FINISHPAGE_RUN "$INSTDIR\Catbot.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Catbot"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Catbot's window draws with Microsoft Edge WebView2, which Windows 11 and most Windows 10
; computers already have.
Function CheckWebView2
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${If} $0 == ""
    ReadRegStr $0 HKLM "${WEBVIEW2_KEY}" "pv"
  ${EndIf}
  ${If} $0 == ""
    ReadRegStr $0 HKCU "${WEBVIEW2_KEY}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    MessageBox MB_YESNO|MB_ICONINFORMATION "Catbot needs Microsoft Edge WebView2, which this PC doesn't have yet. It's free from Microsoft.$\r$\n$\r$\nOpen its download page now? (Install it, then open Catbot.)" IDNO done
    ExecShell "open" "https://developer.microsoft.com/microsoft-edge/webview2/#download"
    done:
  ${EndIf}
FunctionEnd

; Closes a running Catbot (its window, its own Catbot, any desktop pets) so its files can change.
!macro CloseCatbot
  nsExec::Exec 'taskkill /F /IM catbot-server.exe'
  nsExec::Exec 'taskkill /F /IM Catbot.exe'
  Sleep 500
!macroend

Section "Catbot"
  !insertmacro CloseCatbot
  ; A reinstall of the same version starts clean.
  RMDir /r "$INSTDIR\versions\${VERSION}"
  SetOutPath "$INSTDIR\versions\${VERSION}"
  File /r "${SRC}/app/*"
  SetOutPath "$INSTDIR"
  File "${SRC}/Catbot.exe"
  File "${SRC}/app/catbot.ico"
  ; The zip's own installer from before stage 29.5 is no longer needed here.
  Delete "$INSTDIR\install.ps1"
  Delete "$INSTDIR\Install.cmd"

  nsExec::ExecToLog '"$INSTDIR\Catbot.exe" --register ${VERSION}'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION "Catbot was copied, but couldn't note which version to start. Run this setup again."
  ${EndIf}

  CreateShortcut "$SMPROGRAMS\Catbot.lnk" "$INSTDIR\Catbot.exe" "" "$INSTDIR\catbot.ico"
  CreateShortcut "$DESKTOP\Catbot.lnk" "$INSTDIR\Catbot.exe" "" "$INSTDIR\catbot.ico"

  WriteUninstaller "$INSTDIR\Uninstall Catbot.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Catbot"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\catbot.ico"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Catbot"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall Catbot.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall Catbot.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  Call CheckWebView2
SectionEnd

Section "Uninstall"
  !insertmacro CloseCatbot
  Delete "$SMPROGRAMS\Catbot.lnk"
  Delete "$DESKTOP\Catbot.lnk"
  ; Starting when you log in (the tray menu's choice).
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Catbot"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  ; Chats, bots, keys and settings: kept unless the person says to remove them (never when
  ; uninstalling silently).
  IfSilent keep
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Also delete your Catbot chats, bots, keys and settings?$\r$\n$\r$\nChoose No to keep them for if you install Catbot again." IDNO keep
  RMDir /r "$LOCALAPPDATA\Catbot"
  keep:
SectionEnd
