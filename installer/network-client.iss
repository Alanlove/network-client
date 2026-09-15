; ============================================================
; Network Client — Inno Setup Installer
; Produces: NetworkClient-Setup-{version}-x64.exe
; Run from repo root: ISCC installer\network-client.iss
; ============================================================

#define AppName        "Network Client"
#define AppShort       "NetworkClient"
#define AppVersion     "0.1.0"
#define AppPublisher   "Alanlove"
#define AppURL         "https://github.com/Alanlove/network-client"
#define AppExeName     "WindowsClient.exe"

[Setup]
AppId={{8F3B2A1D-4E57-4A6C-9D1E-5F8B7C6D4A3B}}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=artifacts
OutputBaseFilename=NetworkClient-Setup-{#AppVersion}-x64
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\{#AppExeName}
ArchitecturesInstallIn64BitMode=x64
ArchitecturesAllowed=x64
MinVersion=10.0.19041

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create desktop shortcut"; GroupDescription: "Additional icons:"; Flags: unchecked

[Files]
; Source path relative to repo root (where ISCC is invoked from CI)
Source: "artifacts\package\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch Network Client"; Flags: nowait postinstall skipifsilent
