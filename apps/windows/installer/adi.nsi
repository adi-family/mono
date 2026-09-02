; ADI-Setup-x64.exe -- the Windows installer.
;
; This exists to answer one question a person should never be asked: "which of these files am I
; supposed to run?". The platform is four executables and always will be -- a CLI, a resolver, a
; front door and the control panel, each supervised separately -- and on macOS that is invisible,
; because a .app is a folder and they live inside it. Windows has no such envelope, so the job of
; hiding them falls to the installer: the executables go into a bin\ directory nobody has reason
; to open, and what the person gets is one entry in the Start menu.
;
; It is a per-user install (`RequestExecutionLevel user`) and installs into %LOCALAPPDATA%, so
; installing ADI raises no UAC prompt at all. Exactly one step needs administrator -- pointing the
; .adi namespace at the local resolver, an NRPT rule -- and it is taken here, once, during the
; install where a prompt is expected, rather than surprising the person the first time they open
; the app. Declining it costs nothing but the friendly hostname: the control panel is always
; reachable on 127.0.0.1.
;
; Built by apps/windows/build.sh, which passes the version and the staged package directory:
;   makensis -DVERSION=0.3.1 -DSOURCE_DIR=... -DOUTFILE=... adi.nsi

Unicode true

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef SOURCE_DIR
  !define SOURCE_DIR "../build/ADI-windows-x64"
!endif
!ifndef OUTFILE
  !define OUTFILE "../build/ADI-Setup-x64.exe"
!endif
; Windows' own version field is four numbers and nothing else, so the tag is padded out to one
; by build.sh -- `0.3.1` cannot be written here and `0.3.1.0` cannot be written in a release note.
!ifndef VERSION_QUAD
  !define VERSION_QUAD "0.0.0.0"
!endif

!define APP "ADI"
!define PUBLISHER "ADI"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ADI"
; The uninstaller's own name is user-visible in %LOCALAPPDATA%; give it the app's name rather
; than "uninst.exe".
!define UNINST_EXE "Uninstall ADI.exe"

Name "${APP}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\ADI"
; An upgrade lands wherever the previous install went, including a hand-picked directory.
InstallDirRegKey HKCU "Software\ADI" "InstallDir"
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show
BrandingText "${APP} ${VERSION}"

VIProductVersion "${VERSION_QUAD}"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "${PUBLISHER}"
VIAddVersionKey "FileDescription" "ADI installer"

; -- pages ------------------------------------------------------------------------------------
; Welcome, a progress bar, and done. No directory page and no components page: every extra page
; is a decision handed to someone who has no way to make it, and the two that would go there
; (where to install, whether to set up the domain) have one right answer each.

!define MUI_ICON "../ADI.ico"
!define MUI_UNICON "../ADI.ico"
!define MUI_ABORTWARNING

!define MUI_WELCOMEPAGE_TITLE "Install ADI"
!define MUI_WELCOMEPAGE_TEXT "ADI is your machine's own control plane: a control panel in your \
browser, a local DNS resolver that serves the .adi domain, and the services your projects run.$\r$\n$\r$\n\
Everything installs into your user account -- no administrator needed. One step does ask for \
administrator, once: pointing the .adi domain at this machine, so http://app.adi/ works. You can \
say no to that; the control panel still opens on 127.0.0.1.$\r$\n$\r$\n\
Nothing is sent anywhere. ADI runs entirely on this computer."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES

!define MUI_FINISHPAGE_TITLE "ADI is installed"
!define MUI_FINISHPAGE_TEXT "Open ADI and it starts the platform and takes you to the control \
panel. It then sits in the notification area, next to the clock, so you can open the panel, stop \
ADI, or start it again.$\r$\n$\r$\n\
In a terminal, the whole platform is the `adi` command."
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchADI
!define MUI_FINISHPAGE_RUN_TEXT "Open ADI"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; -- shared helpers ---------------------------------------------------------------------------

; Stop everything that could be holding a file in bin\ open. Windows will not replace a running
; executable, so an upgrade that skipped this would fail halfway through with a file-in-use error
; and leave a half-new install behind. `disable` also unregisters the scheduled tasks, whose
; recorded paths may be about to change.
;
; Note the labels rather than relative jumps. A `nsExec::ExecToLog` line is not one instruction --
; a plugin call compiles to a handful, and the count is the plugin's business, not this script's.
; `IfFileExists ... 0 +3` over one therefore lands somewhere nobody chose. Labels are function-
; scoped, so both copies of this macro may use the same names.
!macro StopRunning un
Function ${un}StopRunning
  IfFileExists "$INSTDIR\bin\adi-mono.exe" 0 no_cli
    nsExec::ExecToLog '"$INSTDIR\bin\adi-mono.exe" disable'
    Pop $0
  no_cli:
  nsExec::ExecToLog 'taskkill /F /IM ADI.exe'
  Pop $0
  ; Windows releases the file handles a moment after the process goes; without this an upgrade
  ; can still lose a race it just won.
  Sleep 700
FunctionEnd
!macroend
!insertmacro StopRunning ""
!insertmacro StopRunning "un."

; -- install ----------------------------------------------------------------------------------

Section "ADI" SecMain
  SectionIn RO
  DetailPrint "Stopping any running copy..."
  Call StopRunning

  SetOutPath "$INSTDIR"
  ; The whole staged package: bin\ (the four platform binaries, the launcher, the icon and the
  ; `adi` shim) plus the README and the VERSION file the updater reads.
  File /r "${SOURCE_DIR}/bin"
  File "${SOURCE_DIR}/README.txt"
  File "${SOURCE_DIR}/VERSION"
  File "${SOURCE_DIR}/LICENSE.txt"
  ; Kept out of bin\ deliberately: an auto-update replaces every file in bin\ with the ones from
  ; the release archive, and this script belongs to the installer, not to the payload.
  File "path.ps1"

  ; A second copy of VERSION, beside the binaries, because that is where the updater looks for
  ; the installed version (adi-update's Payload::installed_version reads it next to the running
  ; executable) -- and where it writes it after every update. apps/linux/install.sh does exactly
  ; the same thing for the same reason.
  SetOutPath "$INSTDIR\bin"
  File "${SOURCE_DIR}/VERSION"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\${UNINST_EXE}"

  ; The two places a person looks for an app they just installed. Both point at the launcher and
  ; take their icon from it, so a rebuilt icon needs no reinstall to show up.
  CreateShortCut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\bin\ADI.exe" "" "$INSTDIR\bin\ADI.exe" 0
  CreateShortCut "$DESKTOP\${APP}.lnk" "$INSTDIR\bin\ADI.exe" "" "$INSTDIR\bin\ADI.exe" 0

  ; `adi` on PATH is not a convenience: agents re-invoke the CLI by name (the `harness:adi`
  ; backend), so a PATH without it is a platform whose agents cannot call home.
  DetailPrint "Adding ADI to your PATH..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\path.ps1" -Action Add -Directory "$INSTDIR\bin"'
  Pop $0

  WriteRegStr HKCU "Software\ADI" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\bin\ADI.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\${UNINST_EXE}"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINST_EXE}" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"

  ; The one privileged step, taken here rather than on first launch. `adi-mono up` would do it
  ; itself the first time the app runs (see adi-core's Dns::on_enable), but a UAC prompt that
  ; arrives while someone is opening an app reads as something going wrong; during an install it
  ; reads as an install.
  ;
  ; Skipped for a silent install, where by definition nobody is there to answer a prompt. Such a
  ; machine gets the route the first time ADI is opened instead.
  ${IfNot} ${Silent}
    DetailPrint "Setting up the .adi domain (this is the step that asks for administrator)..."
    nsExec::ExecToLog '"$INSTDIR\bin\adi-mono.exe" dns install-route'
    Pop $0
    ${If} $0 != 0
      DetailPrint "The .adi domain was not set up. ADI works without it -- the control panel opens on 127.0.0.1, and you can set it up later from ADI's icon by the clock."
    ${EndIf}
  ${EndIf}
SectionEnd

Function LaunchADI
  ; Not ExecShell "open": that would run the launcher as a child of the installer and, on an
  ; elevated install, with the installer's token. ExecShell with an explicit verb-less call from
  ; the shell keeps it in the person's own session.
  ExecShell "" "$INSTDIR\bin\ADI.exe"
FunctionEnd

; -- uninstall --------------------------------------------------------------------------------

Section "Uninstall"
  DetailPrint "Stopping ADI..."
  Call un.StopRunning

  ; Give back the .adi namespace: leaving the NRPT rule behind would point every .adi name at a
  ; resolver that is no longer there, which breaks nothing else but is not ours to leave lying
  ; around. It asks for administrator, so it is skipped when nobody is watching.
  ${IfNot} ${Silent}
    IfFileExists "$INSTDIR\bin\adi-mono.exe" 0 no_route
      nsExec::ExecToLog '"$INSTDIR\bin\adi-mono.exe" dns remove-route'
      Pop $0
    no_route:
  ${EndIf}

  IfFileExists "$INSTDIR\path.ps1" 0 no_path_script
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\path.ps1" -Action Remove -Directory "$INSTDIR\bin"'
    Pop $0
  no_path_script:

  Delete "$SMPROGRAMS\${APP}.lnk"
  Delete "$DESKTOP\${APP}.lnk"

  RMDir /r "$INSTDIR\bin"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\VERSION"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\path.ps1"
  Delete "$INSTDIR\${UNINST_EXE}"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\ADI"

  ; %USERPROFILE%\.adi -- projects, secrets, the database, every agent transcript -- is left
  ; exactly where it is. It is the person's work, not the program's files, and an uninstall that
  ; deleted it would be unforgivable. Reinstalling ADI picks it straight back up.
SectionEnd
