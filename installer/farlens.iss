; Build with:  ISCC /DAppVersion=0.1.4 installer\farlens.iss   (see installer\build.ps1)
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6B1F0E52-4C0E-4D5B-9A57-FA21E75C1E01}
AppName=Farlens
AppVersion={#AppVersion}
AppPublisher=Jesse Hufstetler
AppPublisherURL=https://github.com/Jesse-Hufstetler/farlens
DefaultDirName={autopf}\Farlens
DefaultGroupName=Farlens
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\farlens.exe
OutputDir=..\dist
OutputBaseFilename=Farlens-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; The running app holds this mutex, so setup asks to close it before replacing the exe.
AppMutex=Local\Farlens.SingleInstance

[Tasks]
Name: "autostart"; Description: "Start Farlens when I sign in to Windows"; Flags: unchecked
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\farlens.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Farlens"; Filename: "{app}\farlens.exe"
Name: "{group}\Uninstall Farlens"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Farlens"; Filename: "{app}\farlens.exe"; Tasks: desktopicon

[Registry]
; Same value the app's own "Start with Windows" checkbox writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Farlens"; ValueData: """{app}\farlens.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\farlens.exe"; Description: "Launch Farlens"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM farlens.exe"; Flags: runhidden; RunOnceId: "KillFarlens"
