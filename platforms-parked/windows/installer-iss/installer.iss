; -- It Hurts When IP Installer --
#define AppName "It Hurts When IP"
#define AppVersion "1.13.1"
#define AppPublisher "Jaylen Jupp"
#define AppExeName "ItHurtsWhenIP.exe"
#define ServiceExeName "ithurtswhenip-service.exe"
#define ServiceName "ItHurtsWhenIPService"

[Setup]
; AppId uniquely identifies this app. Never change it across versions.
AppId={{A8F4C2E1-9D3B-4A6F-B7E5-1C8D9A2E5F4B}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\ItHurtsWhenIP
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=admin
OutputDir=..\installer-build
OutputBaseFilename=ItHurtsWhenIP-Setup-{#AppVersion}
Compression=lzma
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName={#AppName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"

[Files]
Source: "..\src-tauri\target\release\it-hurts-when-ip.exe"; DestDir: "{app}"; DestName: "{#AppExeName}"; Flags: ignoreversion
Source: "..\service-tool\target\release\{#ServiceExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{commondesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
; Stop and delete any existing service. Wrapped in cmd /c with "& exit 0" so a
; clean machine (no service) does not trigger an Inno Setup error dialog.
Filename: "{cmd}"; Parameters: "/c sc.exe stop {#ServiceName} & exit 0"; Flags: runhidden waituntilterminated
Filename: "{cmd}"; Parameters: "/c sc.exe delete {#ServiceName} & exit 0"; Flags: runhidden waituntilterminated
; Brief pause to let Windows release the previous binary file handle on upgrade.
Filename: "{cmd}"; Parameters: "/c timeout /t 2 /nobreak"; Flags: runhidden waituntilterminated
; Install the fresh service.
Filename: "sc.exe"; Parameters: "create {#ServiceName} binPath= ""{app}\{#ServiceExeName}"" start= auto obj= LocalSystem DisplayName= ""It Hurts When IP Network Service"""; Flags: runhidden waituntilterminated
Filename: "sc.exe"; Parameters: "description {#ServiceName} ""Handles privileged network configuration changes for It Hurts When IP."""; Flags: runhidden waituntilterminated
Filename: "sc.exe"; Parameters: "start {#ServiceName}"; Flags: runhidden waituntilterminated
; Optionally launch the app at the end.
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Same wrapper trick on uninstall in case the service was already removed.
Filename: "{cmd}"; Parameters: "/c sc.exe stop {#ServiceName} & exit 0"; Flags: runhidden waituntilterminated; RunOnceId: "StopService"
Filename: "{cmd}"; Parameters: "/c sc.exe delete {#ServiceName} & exit 0"; Flags: runhidden waituntilterminated; RunOnceId: "DeleteService"

[UninstallDelete]
; Clean up the service log directory on uninstall.
Type: filesandordirs; Name: "{commonappdata}\ItHurtsWhenIP"