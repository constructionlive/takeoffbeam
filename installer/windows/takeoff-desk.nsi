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
InstallDir "$PROGRAMFILES64\Takeoff Desk"
InstallDirRegKey HKLM "Software\Takeoff Desk" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Takeoff Desk"
VIAddVersionKey "CompanyName" "Neuratwin Inc. (construction.live)"
VIAddVersionKey "LegalCopyright" "© 2026 Neuratwin Inc. · Apache-2.0"
VIAddVersionKey "FileDescription" "Takeoff Desk installer"
VIAddVersionKey "FileVersion" "${VERSION}"

!define MUI_ICON "..\..\assets\icon.ico"
!define MUI_UNICON "..\..\assets\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\takeoff-desk.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Takeoff Desk"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE}\LICENSE.txt"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Takeoff Desk" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*.*"
  WriteRegStr HKLM "Software\Takeoff Desk" "InstallDir" "$INSTDIR"
  ; Add/Remove Programs
  !define UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\TakeoffDesk"
  WriteRegStr HKLM "${UNINST}" "DisplayName" "Takeoff Desk"
  WriteRegStr HKLM "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST}" "Publisher" "Neuratwin Inc. (construction.live)"
  WriteRegStr HKLM "${UNINST}" "DisplayIcon" "$INSTDIR\takeoff-desk.exe"
  WriteRegStr HKLM "${UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST}" "URLInfoAbout" "https://construction.live"
  WriteRegDWORD HKLM "${UNINST}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST}" "NoRepair" 1
  ; "Open with" for PDFs (doesn't take over the default PDF app).
  WriteRegStr HKLM "Software\Classes\TakeoffDesk.pdf" "" "PDF drawing (Takeoff Desk)"
  WriteRegStr HKLM "Software\Classes\TakeoffDesk.pdf\DefaultIcon" "" "$INSTDIR\takeoff-desk.exe,0"
  WriteRegStr HKLM "Software\Classes\TakeoffDesk.pdf\shell\open\command" "" '"$INSTDIR\takeoff-desk.exe" "%1"'
  WriteRegStr HKLM "Software\Classes\.pdf\OpenWithProgids" "TakeoffDesk.pdf" ""
  CreateDirectory "$SMPROGRAMS\Takeoff Desk"
  CreateShortcut "$SMPROGRAMS\Takeoff Desk\Takeoff Desk.lnk" "$INSTDIR\takeoff-desk.exe"
  CreateShortcut "$DESKTOP\Takeoff Desk.lnk" "$INSTDIR\takeoff-desk.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

; llama.cpp (local AI) needs the Visual C++ runtime; install it only if missing.
Section "-Visual C++ runtime"
  ReadRegDWORD $0 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  StrCmp $0 "1" vc_done
  SetOutPath "$TEMP\TakeoffDesk"
  File "${VCREDIST}"
  DetailPrint "Installing the Microsoft Visual C++ runtime…"
  ExecWait '"$TEMP\TakeoffDesk\vc_redist.x64.exe" /install /quiet /norestart'
  Delete "$TEMP\TakeoffDesk\vc_redist.x64.exe"
  vc_done:
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\Takeoff Desk.lnk"
  RMDir /r "$SMPROGRAMS\Takeoff Desk"
  DeleteRegKey HKLM "Software\Classes\TakeoffDesk.pdf"
  DeleteRegValue HKLM "Software\Classes\.pdf\OpenWithProgids" "TakeoffDesk.pdf"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\TakeoffDesk"
  DeleteRegKey HKLM "Software\Takeoff Desk"
  RMDir /r "$INSTDIR"
SectionEnd
