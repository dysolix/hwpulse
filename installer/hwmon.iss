; Per-user installer for the portable hwmon.exe. Build with installer\build.ps1,
; which passes the version from Cargo.toml.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
; Numeric part only (no "-beta"), as Windows file versions require.
#ifndef FileVersion
  #define FileVersion AppVersion
#endif
; Drawn by build.rs into Cargo's build folder; build.ps1 passes its path.
#ifndef IconFile
  #error IconFile is not set; build with installer\build.ps1
#endif

[Setup]
AppId={{6E3F0B52-8C4A-4D1E-9B7A-2F5C1D8E4A93}
AppName=Hardware Monitor
AppVersion={#AppVersion}
AppVerName=Hardware Monitor {#AppVersion}
AppPublisher=dysolix
VersionInfoVersion={#FileVersion}
VersionInfoProductVersion={#AppVersion}
VersionInfoProductName=Hardware Monitor
VersionInfoDescription=Hardware Monitor Setup
; "Install for me only" (default, no admin): %LOCALAPPDATA%\Programs\hwmon.
; "Install for all users" (admin): Program Files, where only admins can replace the exe,
; so "Restart as administrator" cannot be used to run a swapped-in program elevated.
; Updates keep the mode of the existing install without asking again.
DefaultDirName={autopf}\hwmon
; An empty folder left behind by an earlier uninstall is no reason to warn.
DirExistsWarning=no
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
; The only per-user entry is the uninstall cleanup of "Start with Windows" below.
UsedUserAreasWarning=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Close a running hwmon (Restart Manager) before replacing the exe.
CloseApplications=yes
RestartApplications=no
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\hwmon.exe
UninstallDisplayName=Hardware Monitor
WizardStyle=modern dynamic
OutputDir=..\target\installer
OutputBaseFilename=hwmon-setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes

[Tasks]
; "Start with Windows" is only in the app's Settings: it writes the Run entry of the
; user who switches it on, which an installer running elevated cannot always do.
Name: desktopicon; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "..\target\release\hwmon.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Hardware Monitor"; Filename: "{app}\hwmon.exe"
Name: "{autodesktop}\Hardware Monitor"; Filename: "{app}\hwmon.exe"; Tasks: desktopicon

[Registry]
; The value the app's "Start with Windows" switch writes (src/autostart.rs), removed
; on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "hwmon"; Flags: uninsdeletevalue

[UninstallDelete]
; Without this, the uninstaller keeps a folder that already existed before setup ran,
; so one leftover would survive every later install and uninstall.
Type: dirifempty; Name: "{app}"

[Run]
; As the user who started setup: after an all-users install, setup itself runs as admin,
; and hwmon would otherwise start elevated.
Filename: "{app}\hwmon.exe"; Description: "Launch Hardware Monitor"; Flags: nowait postinstall skipifsilent runasoriginaluser
