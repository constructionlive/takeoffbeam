; Takeoff Desk — Windows installer (NSIS). Built by scripts/package-windows.sh.
Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.1"
!endif
!ifndef STAGE
  !define STAGE "..\..\dist\windows\stage"
!endif
!ifndef OUTFILE
  !define OUTFILE "..\..\dist\Takeoff-Desk-${VERSION}-x64-setup.exe"
!endif

Name "Takeoff Desk"
OutFile "${OUTFILE}"
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Takeoff Desk"
VIAddVersionKey "CompanyName" "Neuratwin Inc. (construction.live)"
VIAddVersionKey "LegalCopyright" "© 2026 Neuratwin Inc. · Apache-2.0"
VIAddVersionKey "FileDescription" "Takeoff Desk installer"
VIAddVersionKey "FileVersion" "${VERSION}"

; Authenticode: SIGN is a command that signs the file it is given (see scripts/package-windows.sh).
!ifdef SIGN
  !finalize '${SIGN} "%1"'
  !uninstfinalize '${SIGN} "%1"'
!endif

; For everyone (Program Files, needs admin) or only for me (%LOCALAPPDATA%\Programs, no admin).
; Admins choose on a page and default to everyone; other accounts install for themselves. An
; update keeps the mode of the copy already installed. Silent: /S, with /CurrentUser or /AllUsers.
!define MULTIUSER_EXECUTIONLEVEL Highest
!define MULTIUSER_MUI
!define MULTIUSER_INSTALLMODE_COMMANDLINE
!define MULTIUSER_USE_PROGRAMFILES64
!define MULTIUSER_INSTALLMODE_INSTDIR "Takeoff Desk"
!define MULTIUSER_INSTALLMODE_INSTDIR_REGISTRY_KEY "Software\Takeoff Desk"
!define MULTIUSER_INSTALLMODE_INSTDIR_REGISTRY_VALUENAME "InstallDir"
!define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "Software\Takeoff Desk"
!define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "InstallDir"
!include "MultiUser.nsh"

!define MUI_ICON "..\..\assets\icon.ico"
!define MUI_UNICON "..\..\assets\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\takeoff-desk.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Takeoff Desk"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE}\LICENSE.txt"
!insertmacro MULTIUSER_PAGE_INSTALLMODE
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  !insertmacro MULTIUSER_INIT
FunctionEnd

Function un.onInit
  !insertmacro MULTIUSER_UNINIT
FunctionEnd

Section "Takeoff Desk" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*.*"
  WriteRegStr SHCTX "Software\Takeoff Desk" "InstallDir" "$INSTDIR"
  ; Add/Remove Programs
  !define UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\TakeoffDesk"
  WriteRegStr SHCTX "${UNINST}" "DisplayName" "Takeoff Desk"
  WriteRegStr SHCTX "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINST}" "Publisher" "Neuratwin Inc. (construction.live)"
  WriteRegStr SHCTX "${UNINST}" "DisplayIcon" "$INSTDIR\takeoff-desk.exe"
  WriteRegStr SHCTX "${UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe" /$MultiUser.InstallMode'
  WriteRegStr SHCTX "${UNINST}" "URLInfoAbout" "https://construction.live"
  WriteRegDWORD SHCTX "${UNINST}" "NoModify" 1
  WriteRegDWORD SHCTX "${UNINST}" "NoRepair" 1
  ; "Open with" for PDFs (doesn't take over the default PDF app). SHCTX is HKCU or HKLM to
  ; match the install mode. Each file Explorer opens starts takeoff-desk.exe "<file>".
  WriteRegStr SHCTX "Software\Classes\TakeoffDesk.pdf" "" "PDF drawing (Takeoff Desk)"
  WriteRegStr SHCTX "Software\Classes\TakeoffDesk.pdf\DefaultIcon" "" "$INSTDIR\takeoff-desk.exe,0"
  WriteRegStr SHCTX "Software\Classes\TakeoffDesk.pdf\shell\open\command" "" '"$INSTDIR\takeoff-desk.exe" "%1"'
  WriteRegStr SHCTX "Software\Classes\.pdf\OpenWithProgids" "TakeoffDesk.pdf" ""
  ; Lists the app by name under "Open with → Choose another app".
  WriteRegStr SHCTX "Software\Classes\Applications\takeoff-desk.exe" "FriendlyAppName" "Takeoff Desk"
  WriteRegStr SHCTX "Software\Classes\Applications\takeoff-desk.exe\SupportedTypes" ".pdf" ""
  WriteRegStr SHCTX "Software\Classes\Applications\takeoff-desk.exe\shell\open\command" "" '"$INSTDIR\takeoff-desk.exe" "%1"'
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  CreateDirectory "$SMPROGRAMS\Takeoff Desk"
  CreateShortcut "$SMPROGRAMS\Takeoff Desk\Takeoff Desk.lnk" "$INSTDIR\takeoff-desk.exe"
  CreateShortcut "$DESKTOP\Takeoff Desk.lnk" "$INSTDIR\takeoff-desk.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\Takeoff Desk.lnk"
  RMDir /r "$SMPROGRAMS\Takeoff Desk"
  DeleteRegKey SHCTX "Software\Classes\TakeoffDesk.pdf"
  DeleteRegValue SHCTX "Software\Classes\.pdf\OpenWithProgids" "TakeoffDesk.pdf"
  DeleteRegKey SHCTX "Software\Classes\Applications\takeoff-desk.exe"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  DeleteRegKey SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\TakeoffDesk"
  DeleteRegKey SHCTX "Software\Takeoff Desk"
  RMDir /r "$INSTDIR"
SectionEnd
