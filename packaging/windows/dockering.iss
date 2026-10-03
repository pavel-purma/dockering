; Dockering Windows installer (Inno Setup 6.3+). REL-020…025, UPD-007; ADR-0006.
;
; Built by `cargo xtask package --formats inno`, which passes:
;   /DAppVersion=<semver> /DArch=<x64|arm64> /DSourceDir=<dir with dockering.exe + licence files>
;   /DOutputDir=<dir> /DRepoRoot=<repo root>   and optionally   /Ssigntool=<cmd> /DSign
;
; Command-line switches understood by this script (on top of Inno's own /SILENT, /VERYSILENT,
; /CURRENTUSER, /ALLUSERS, …):
;   /UPDATE    in-app update: progress page only, no other wizard pages (UPD-007)
;   /RELAUNCH  start Dockering after installing, also in silent mode (UPD-007)

#ifndef AppVersion
  #error AppVersion is not defined (run through cargo xtask package)
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef RepoRoot
  #define RepoRoot "..\.."
#endif
#ifndef SourceDir
  #define SourceDir RepoRoot + "\target\release"
#endif
#ifndef OutputDir
  #define OutputDir RepoRoot + "\target\dist"
#endif

#define AppName "Dockering"
#define AppExe "dockering.exe"
#define AppUserModelId "dev.dockering.Dockering"
#define RepoUrl "https://github.com/pavel-purma/dockering"
#define Wizard RepoRoot + "\packaging\windows\wizard"

[Setup]
; Never change AppId: upgrades, winget, and the CI smoke test key on it.
AppId={{8C3F4E2A-6B1D-4E7A-9F2C-5D8B1A3E7C64}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Dockering contributors
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
AppCopyright=Copyright (c) 2026 Dockering contributors
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
; REL-021: per user by default, all users via the scope dialog or /ALLUSERS.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
UsePreviousAppDir=yes
; REL-022
WizardStyle=modern
#ifexist Wizard + "\WizardImage100.bmp"
WizardImageFile={#Wizard}\WizardImage100.bmp,{#Wizard}\WizardImage125.bmp,{#Wizard}\WizardImage150.bmp,{#Wizard}\WizardImage175.bmp,{#Wizard}\WizardImage200.bmp
WizardSmallImageFile={#Wizard}\WizardSmallImage100.bmp,{#Wizard}\WizardSmallImage125.bmp,{#Wizard}\WizardSmallImage150.bmp,{#Wizard}\WizardSmallImage175.bmp,{#Wizard}\WizardSmallImage200.bmp
#endif
SetupIconFile={#RepoRoot}\assets\app-icon\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
; Windows 10 22H2+ (spec 00).
MinVersion=10.0.19045
; REL-024: detect a running Dockering and close it through the Restart Manager.
AppMutex=dev.dockering.Dockering
CloseApplications=force
RestartApplications=no
Compression=lzma2/ultra64
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=Dockering-Setup-{#Arch}
SetupLogging=yes
#ifdef Sign
SignTool=signtool
SignedUninstaller=yes
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD_PARTY_LICENSES.html"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; REL-023: the AUMID matches the one the app sets at startup, so taskbar pins group correctly.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppUserModelId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "{#AppUserModelId}"; Tasks: desktopicon

[Run]
; Interactive installs: "Launch Dockering" checkbox on the finish page.
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent runasoriginaluser; Check: not IsRelaunch
; /RELAUNCH (in-app update): always start the app again, even silently. Never elevated.
Filename: "{app}\{#AppExe}"; Flags: nowait runasoriginaluser; Check: IsRelaunch

[Code]
var
  RemoveUserData: Boolean;

function HasSwitch(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
    begin
      Result := True;
      Exit;
    end;
end;

function IsUpdate: Boolean;
begin
  Result := HasSwitch('/UPDATE');
end;

function IsRelaunch: Boolean;
begin
  Result := HasSwitch('/RELAUNCH');
end;

// UPD-007: an in-app update shows only the progress page.
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := IsUpdate and (PageID <> wpInstalling);
end;

// REL-022: optionally remove settings and logs. Default: keep. Silent uninstall keeps them.
// Paths follow directories::ProjectDirs("dev", "dockering", "Dockering") (spec 10 §6).
function InitializeUninstall: Boolean;
begin
  RemoveUserData := False;
  if not UninstallSilent then
    RemoveUserData := MsgBox('Also remove Dockering settings and logs?', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and RemoveUserData then
  begin
    DelTree(ExpandConstant('{userappdata}\dockering\Dockering'), True, True, True);
    DelTree(ExpandConstant('{localappdata}\dockering\Dockering'), True, True, True);
  end;
end;
