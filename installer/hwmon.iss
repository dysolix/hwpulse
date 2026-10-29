; Per-user installer for the portable hwmon.exe. Build with installer\build.ps1,
; which passes the version from Cargo.toml.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
; Numeric part only (no "-beta"), as Windows file versions require.
#ifndef FileVersion
  #define FileVersion AppVersion
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
DefaultDirName={localappdata}\Programs\hwmon
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Close a running hwmon (Restart Manager) before replacing the exe.
CloseApplications=yes
RestartApplications=no
SetupIconFile=..\assets\hwmon.ico
UninstallDisplayIcon={app}\hwmon.exe
UninstallDisplayName=Hardware Monitor
WizardStyle=modern dynamic
OutputDir=..\target\installer
OutputBaseFilename=hwmon-setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes

[Tasks]
Name: autostart; Description: "Start with Windows"; GroupDescription: "Startup:"; Flags: unchecked
Name: desktopicon; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "..\target\release\hwmon.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Hardware Monitor"; Filename: "{app}\hwmon.exe"
Name: "{autodesktop}\Hardware Monitor"; Filename: "{app}\hwmon.exe"; Tasks: desktopicon

[Registry]
; Same value the app's "Start with Windows" switch writes (src/autostart.rs).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "hwmon"; ValueData: """{app}\hwmon.exe"""; Tasks: autostart
; Removed on uninstall even if it was switched on later in the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "hwmon"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\hwmon.exe"; Description: "Launch Hardware Monitor"; Flags: nowait postinstall skipifsilent
