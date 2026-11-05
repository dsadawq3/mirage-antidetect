; ---------------------------------------------------------------------------
; installer/mirage.nsi - Nullsoft (NSIS) installer for Mirage.
;
; Build (NSIS 3.x required, makensis on PATH, run from the repository root;
; dist\ is created by scripts/package.ps1 - create it manually when building
; the installer standalone:  mkdir dist):
;
;   makensis installer\mirage.nsi
;   makensis /DMIRAGE_VERSION=0.2.0 installer\mirage.nsi     (version override)
;
; MIRAGE_VERSION defaults to the !define below; keep it in sync with the
; [workspace.package] version in the root Cargo.toml (that is where
; scripts/package.ps1 reads the version from), or pass it via /D on each build.
;
; The installer targets the wave-2 CLI/API product: no desktop or Start Menu
; shortcuts by design - consumers drive Mirage through its CLI and local API.
;
; NOTE: this script was NOT test-built in the development environment
; (makensis is not installed there). The first real build must happen on a
; machine with NSIS before any release ships - see docs/RELEASE.md.
;
; NOTE (wave 1): the workspace is library-only (no [[bin]] targets yet).
; "File ..\target\release\*.exe" therefore matches no files and makensis
; aborts with a "no matching files" error until the wave-2 binaries exist.
; The line stays ready for them on purpose.
; ---------------------------------------------------------------------------

!ifndef MIRAGE_VERSION
  ; Keep in sync with [workspace.package] in the root Cargo.toml.
  !define MIRAGE_VERSION "0.1.0"
!endif

!define MIRAGE_NAME       "Mirage"
!define MIRAGE_PUBLISHER  "Mirage Contributors"
!define MIRAGE_UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Mirage"

; All relative paths below resolve against this script's directory,
; regardless of where makensis was invoked from.
!cd "${__FILEDIR__}"

Name "${MIRAGE_NAME} ${MIRAGE_VERSION}"
OutFile "..\dist\Mirage-Setup-${MIRAGE_VERSION}-win-x64.exe"

Unicode true
SetCompressor /SOLID lzma
BrandingText "${MIRAGE_NAME}"

; Writing into Program Files needs elevation; the installer needs nothing
; beyond that.
RequestExecutionLevel admin

; Reuse the previous install location on upgrades.
InstallDir "$PROGRAMFILES64\Mirage"
InstallDirRegKey HKLM "${MIRAGE_UNINST_KEY}" "InstallLocation"

; Version metadata. VIProductVersion needs a 4-part numeric form: fine for a
; plain X.Y.Z release; adjust manually for a pre-release suffix.
VIProductVersion "${MIRAGE_VERSION}.0"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "${MIRAGE_NAME}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductVersion" "${MIRAGE_VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "CompanyName" "${MIRAGE_PUBLISHER}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "${MIRAGE_NAME} installer"
VIAddVersionKey /LANG=${LANG_ENGLISH} "LegalCopyright" "(c) 2026 ${MIRAGE_PUBLISHER}, MIT License"

; Classic pages: MIT license -> install directory -> install.
; No shortcut pages: CLI/API product (wave 2), see the header.
LicenseData "..\LICENSE"
LicenseText "Mirage is distributed under the MIT License." "I agree"

Page license
Page directory
Page instfiles

Section "Mirage (required)"
  SectionIn RO

  SetOutPath "$INSTDIR"

  ; Wave-2 CLI/API binaries. Requires a prior `cargo build --release
  ; --workspace` with [[bin]] targets present (see the wave-1 note in the
  ; header). Examples never sit in the release root, so they stay out.
  File "..\target\release\*.exe"

  File "..\README.md"
  File "..\LICENSE"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; Add/Remove Programs entry; InstallLocation feeds InstallDirRegKey above.
  WriteRegStr   HKLM "${MIRAGE_UNINST_KEY}" "DisplayName"     "${MIRAGE_NAME} ${MIRAGE_VERSION}"
  WriteRegStr   HKLM "${MIRAGE_UNINST_KEY}" "DisplayVersion"  "${MIRAGE_VERSION}"
  WriteRegStr   HKLM "${MIRAGE_UNINST_KEY}" "Publisher"       "${MIRAGE_PUBLISHER}"
  WriteRegStr   HKLM "${MIRAGE_UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKLM "${MIRAGE_UNINST_KEY}" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegDWORD HKLM "${MIRAGE_UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${MIRAGE_UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$INSTDIR\*.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  DeleteRegKey HKLM "${MIRAGE_UNINST_KEY}"

  ; Removes $INSTDIR only when empty; user data (profiles, vault) lives in
  ; %LOCALAPPDATA%\Mirage and is never touched by the uninstaller.
  RMDir "$INSTDIR"
SectionEnd
