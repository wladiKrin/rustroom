; Rustroom's Windows installer (v0.1, piece 3a) — compiled by packaging\windows\build.ps1, which passes /DVersion,
; /DStage (the folder holding Rustroom.exe and the licences) and /DOutput. Per user: no admin prompt, installed into
; %LOCALAPPDATA%\Programs\Rustroom — or, with /ALLUSERS (Chocolatey's), into Program Files. Uninstalling never touches the catalog, previews or settings (%LOCALAPPDATA%\rustroom,
; %APPDATA%\rustroom) — they are not in {app}.
;
; ⚠⚠ AppId is FIXED FOREVER: an upgrade finds the earlier install by it, and a new one would orphan every install made.

#ifndef Version
  #error Pass /DVersion=x.y.z (build.ps1 does)
#endif
#ifndef Stage
  #error Pass /DStage=<folder> (build.ps1 does)
#endif
#ifndef Output
  #define Output "..\..\target\installer"
#endif

[Setup]
AppId={{240EB815-FED8-4E19-84FE-EFE2069F1D00}
AppName=Rustroom
AppVersion={#Version}
AppVerName=Rustroom {#Version}
AppPublisher=The Rustroom contributors
AppPublisherURL=https://github.com/wladiKrin/rustroom
AppSupportURL=https://github.com/wladiKrin/rustroom/issues
VersionInfoVersion={#Version}
DefaultDirName={autopf}\Rustroom
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; Piece 4 goal 3: Chocolatey runs elevated, often as another account, so it passes /ALLUSERS and installs into Program
; Files (administrative mode, {autopf} = {commonpf}). Without a switch Setup stays per user and asks for no admin, as
; before. Only the command line may override — never a dialog in front of everyone — so UsePreviousPrivileges stays
; inert: a double-click beside a Chocolatey install adds a per-user copy; `choco upgrade` is how that one is updated.
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=..\icons\rustroom.ico
UninstallDisplayIcon={app}\Rustroom.exe
UninstallDisplayName=Rustroom
; The GPL needs no acceptance: shown as information, with Next rather than "I accept".
InfoBeforeFile={#Stage}\LICENSE.txt
OutputDir={#Output}
OutputBaseFilename=Rustroom-{#Version}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Stage}\Rustroom.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY-LICENSES.html"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\Rustroom"; Filename: "{app}\Rustroom.exe"
Name: "{autodesktop}\Rustroom"; Filename: "{app}\Rustroom.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Rustroom.exe"; Description: "{cm:LaunchProgram,Rustroom}"; Flags: nowait postinstall skipifsilent
