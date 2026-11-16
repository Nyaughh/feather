; The Windows installer, built with Inno Setup:
;   ISCC.exe /DAppVersion=0.2.1 packaging\windows\feather.iss
; Expects the release build at target\release\feather.exe.
;
; It installs for the current user only (no administrator prompt), into
; %LOCALAPPDATA%\Programs\Feather. That folder is writable without elevation,
; which is what lets Feather replace its own program file when it updates.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Identifies Feather to Windows across versions. Never change it.
AppId={{6F1D2B8A-3C4E-4E7B-9A51-FEA7E2B0C0D1}
AppName=Feather
AppVersion={#AppVersion}
AppVerName=Feather {#AppVersion}
AppPublisher=Nyaughh
AppPublisherURL=https://feather.lh3k.dev
AppSupportURL=https://github.com/Nyaughh/feather
AppUpdatesURL=https://github.com/Nyaughh/feather/releases
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Feather
DisableProgramGroupPage=yes
DisableDirPage=auto
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\feather.exe
UninstallDisplayName=Feather
OutputDir=..\..
OutputBaseFilename=feather-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Close a running Feather so its program file can be replaced.
CloseApplications=yes
RestartApplications=no
#if Ver >= EncodeVer(6,3,0)
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#else
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "contextmenu"; Description: "Add ""Open with Feather"" to the right-click menu for folders"; GroupDescription: "File Explorer:"

[Files]
Source: "..\..\target\release\feather.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Feather"; Filename: "{app}\feather.exe"
Name: "{autodesktop}\Feather"; Filename: "{app}\feather.exe"; Tasks: desktopicon

[Registry]
; Right-click a folder, or the empty space inside one.
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Feather"; ValueType: string; ValueName: ""; ValueData: "Open with Feather"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Feather"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\feather.exe"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Feather\command"; ValueType: string; ValueName: ""; ValueData: """{app}\feather.exe"" ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Feather"; ValueType: string; ValueName: ""; ValueData: "Open with Feather"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Feather"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\feather.exe"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Feather\command"; ValueType: string; ValueName: ""; ValueData: """{app}\feather.exe"" ""%V"""; Tasks: contextmenu

[Run]
Filename: "{app}\feather.exe"; Description: "{cm:LaunchProgram,Feather}"; Flags: nowait postinstall skipifsilent
