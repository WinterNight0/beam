; beam's setup wizard for Windows (Inno Setup 6, ADR-0056).
;
; Build it with scripts\windows\build-installer.ps1, which compiles beam and
; beam-gui in release mode and passes the version and paths below. By hand:
;   "C:\Inno Setup 6\ISCC.exe" /DAppVersion=0.1.0 /DRepo=E:\path\to\beam packaging\windows\beam.iss
;
; It installs for the current user only (no administrator rights), into the
; same folder as scripts\windows\install.ps1: %LOCALAPPDATA%\Programs\beam.
; It never touches ~/.beam, where the device key and paired peers live, not
; even when uninstalling.

#ifndef AppVersion
  #error Pass /DAppVersion=<version> (build-installer.ps1 does)
#endif
#ifndef Repo
  #define Repo AddBackslash(SourcePath) + "..\.."
#endif

[Setup]
; The same id for every version, so a new setup updates an old install.
AppId={{76B8B7A9-D8C1-4122-9826-3DB3CE9A620A}
AppName=beam
AppVersion={#AppVersion}
AppVerName=beam {#AppVersion}
AppPublisher=The beam project
AppComments=Send files straight to the people you pair with.
DefaultDirName={autopf}\beam
DefaultGroupName=beam
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile={#Repo}\crates\beam-gui\assets\images\beam-gui.ico
; The window's icon in Settings -> Apps, or the CLI's without the window.
UninstallDisplayIcon={code:UninstallIcon}
UninstallDisplayName=beam
; Closes a running beam (or its background agent) before replacing it.
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes
Compression=lzma2/max
SolidCompression=yes
OutputDir={#Repo}\target\installer
OutputBaseFilename=beam-setup-{#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

; The page "Select Components": the command line always, the desktop window
; ticked unless the person unticks it.
[Types]
Name: "full"; Description: "beam with the desktop window"
Name: "cli"; Description: "Command line and full-screen view only"
Name: "custom"; Description: "Custom"; Flags: iscustom

[Components]
Name: "cli"; Description: "beam: the command line and the full-screen terminal view"; Types: full cli custom; Flags: fixed
Name: "gui"; Description: "beam-gui: the desktop window"; Types: full

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Components: gui; Flags: unchecked
Name: "addtopath"; Description: "Add &beam to the PATH, so the beam command works in any terminal"; GroupDescription: "Command line:"

[Files]
Source: "{#Repo}\target\release\beam-gui.exe"; DestDir: "{app}"; Components: gui; Flags: ignoreversion
Source: "{#Repo}\target\release\beam.exe"; DestDir: "{app}"; Components: cli; Flags: ignoreversion
; Licences: beam's own, the crates built into it (made by build-installer.ps1
; with cargo-about, ADR-0058), and the bundled fonts and icons.
Source: "{#Repo}\LICENSE.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Repo}\target\licenses\THIRD-PARTY.html"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Repo}\crates\beam-core\LICENSE"; DestDir: "{app}\licenses"; DestName: "beam-core-MIT.txt"; Flags: ignoreversion
Source: "{#Repo}\crates\beam-model\LICENSE"; DestDir: "{app}\licenses"; DestName: "beam-model-MIT.txt"; Flags: ignoreversion
Source: "{#Repo}\crates\beam-gui\assets\LICENSES\*"; DestDir: "{app}\licenses"; Flags: ignoreversion

; Installing without the window over an install with it: take the window away.
[InstallDelete]
Type: files; Name: "{app}\beam-gui.exe"; Components: not gui
Type: files; Name: "{group}\beam.lnk"; Components: not gui
Type: files; Name: "{autodesktop}\beam.lnk"; Components: not gui

[Icons]
Name: "{group}\beam"; Filename: "{app}\beam-gui.exe"; WorkingDir: "{%USERPROFILE}"; Comment: "Send files straight to the people you pair with"; Components: gui
Name: "{group}\beam (terminal)"; Filename: "{app}\beam.exe"; WorkingDir: "{%USERPROFILE}"; Comment: "beam's full-screen view in a terminal"
Name: "{autodesktop}\beam"; Filename: "{app}\beam-gui.exe"; WorkingDir: "{%USERPROFILE}"; Tasks: desktopicon

[Run]
Filename: "{app}\beam-gui.exe"; Description: "Open beam now"; Components: gui; Flags: nowait postinstall skipifsilent
Filename: "{app}\beam.exe"; Description: "Open beam now (full-screen view)"; Components: not gui; Flags: nowait postinstall skipifsilent

[UninstallRun]
; If the background agent was turned on from this copy of beam, it starts
; from {app}\beam.exe at sign-in: stop it and remove that entry, so nothing
; points at a deleted file. An agent run from another beam is left alone.
Filename: "{app}\beam.exe"; Parameters: "service disable"; Flags: runhidden; RunOnceId: "DisableAgent"; Check: AgentRunsFromHere

[Code]
const
  EnvKey = 'Environment';
  { Where `beam service enable` registers the agent (beam-core agent::service). }
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  AgentName = 'beam-agent';

{ Whether the agent's sign-in entry starts this folder's beam.exe. }
function AgentRunsFromHere: Boolean;
var
  Command: string;
begin
  Result := RegQueryStringValue(HKCU, RunKey, AgentName, Command) and
    (Pos(Lowercase(AddBackslash(ExpandConstant('{app}')) + 'beam.exe'),
         Lowercase(Command)) > 0);
end;

{ The program whose icon Settings -> Apps shows. }
function UninstallIcon(Param: string): string;
begin
  if WizardIsComponentSelected('gui') then
    Result := ExpandConstant('{app}\beam-gui.exe')
  else
    Result := ExpandConstant('{app}\beam.exe');
end;

{ Path entries compared without case or a trailing backslash. }
function SameDir(A, B: string): Boolean;
begin
  Result := CompareText(RemoveBackslashUnlessRoot(Trim(A)),
                        RemoveBackslashUnlessRoot(Trim(B))) = 0;
end;

{ The user PATH with Dir taken out (Add = False) or put at the end once
  (Add = True). The value keeps its type, so %VARIABLES% stay as they were. }
procedure ChangeUserPath(Dir: string; Add: Boolean);
var
  Path, Entry, Rest, Kept: string;
  Found: Boolean;
  P: Integer;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Path) then
    Path := '';
  Rest := Path;
  Kept := '';
  Found := False;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P = 0 then
    begin
      Entry := Rest;
      Rest := '';
    end else
    begin
      Entry := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end;
    if Trim(Entry) = '' then
      Continue;
    if SameDir(Entry, Dir) then
    begin
      Found := True;
      if not Add then
        Continue;
    end;
    if Kept <> '' then
      Kept := Kept + ';';
    Kept := Kept + Entry;
  end;
  if Add and not Found then
  begin
    if Kept <> '' then
      Kept := Kept + ';';
    Kept := Kept + Dir;
  end;
  if Kept <> Path then
    RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Kept);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('addtopath') then
      ChangeUserPath(ExpandConstant('{app}'), True)
    else
      ChangeUserPath(ExpandConstant('{app}'), False);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    ChangeUserPath(ExpandConstant('{app}'), False);
end;
