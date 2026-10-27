#ifndef Version
  #error Pass the version: /DVersion=<version>
#endif
#define Source SourcePath + "..\dist\whisperia-client"

[Setup]
AppId={{EB2C952D-5090-4CBB-BD8F-DB1BEDA901F4}
AppName=Whisperia
AppVersion={#Version}
AppPublisherURL=https://github.com/jagerente/whisperia
VersionInfoVersion={#Version}
DefaultDirName={autopf}\Whisperia
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
InfoBeforeFile={#Source}\LICENSE
SetupIconFile={#SourcePath}..\assets\whisperia.ico
UninstallDisplayIcon={app}\whisperia.exe
UninstallDisplayName=Whisperia
OutputBaseFilename=whisperia-client-{#Version}-windows-x64-setup
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=yes

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\*"; Excludes: "*.old-*"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Whisperia"; Filename: "{app}\whisperia.exe"
Name: "{autodesktop}\Whisperia"; Filename: "{app}\whisperia.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\whisperia.exe"; Description: "{cm:LaunchProgram,Whisperia}"; Flags: nowait postinstall skipifsilent

[Code]
const
  RmSessionKeyLength = 32;

function RmStartSession(var Session: DWORD; Flags: DWORD; Key: String): DWORD;
  external 'RmStartSession@rstrtmgr.dll stdcall uninstallonly';
function RmRegisterResources(Session: DWORD; FileCount: DWORD; Files: TArrayOfString; ProcessCount: DWORD; Processes: Longint; ServiceCount: DWORD; Services: Longint): DWORD;
  external 'RmRegisterResources@rstrtmgr.dll stdcall uninstallonly';
function RmShutdown(Session: DWORD; Flags: DWORD; Status: Longint): DWORD;
  external 'RmShutdown@rstrtmgr.dll stdcall uninstallonly';
function RmEndSession(Session: DWORD): DWORD;
  external 'RmEndSession@rstrtmgr.dll stdcall uninstallonly';

function CloseWhisperia(): DWORD;
var
  Session: DWORD;
  Files: TArrayOfString;
begin
  Result := RmStartSession(Session, 0, StringOfChar(' ', RmSessionKeyLength + 1));
  if Result <> 0 then
    Exit;
  SetArrayLength(Files, 1);
  Files[0] := ExpandConstant('{app}\whisperia.exe');
  Result := RmRegisterResources(Session, 1, Files, 0, 0, 0, 0);
  if Result = 0 then
    Result := RmShutdown(Session, 0, 0);
  RmEndSession(Session);
end;

var
  DeleteSettings: TNewCheckBox;

procedure AskWhatToDelete();
var
  Form: TUninstallProgressForm;
  Page: TNewNotebookPage;
  Question: TNewStaticText;
  UninstallButton: TNewButton;
  PageName, PageDescription: String;
begin
  Form := UninstallProgressForm;
  PageName := Form.PageNameLabel.Caption;
  PageDescription := Form.PageDescriptionLabel.Caption;

  Page := TNewNotebookPage.Create(Form);
  Page.Notebook := Form.InnerNotebook;
  Page.Parent := Form.InnerNotebook;
  Page.Align := alClient;

  Question := TNewStaticText.Create(Form);
  Question.Parent := Page;
  Question.AutoSize := False;
  Question.WordWrap := True;
  Question.Width := Page.ClientWidth;
  Question.Height := ScaleY(30);
  Question.Caption := 'Your settings and the log stay on this PC, so a later install starts where you left off.';

  DeleteSettings := TNewCheckBox.Create(Form);
  DeleteSettings.Parent := Page;
  DeleteSettings.Top := Question.Top + Question.Height + ScaleY(8);
  DeleteSettings.Width := Page.ClientWidth;
  DeleteSettings.Height := ScaleY(17);
  DeleteSettings.Caption := 'Delete my settings and the log too';
  DeleteSettings.Checked := False;

  UninstallButton := TNewButton.Create(Form);
  UninstallButton.Parent := Form;
  UninstallButton.Left := Form.CancelButton.Left - Form.CancelButton.Width - ScaleX(10);
  UninstallButton.Top := Form.CancelButton.Top;
  UninstallButton.Width := Form.CancelButton.Width;
  UninstallButton.Height := Form.CancelButton.Height;
  UninstallButton.Caption := 'Uninstall';
  UninstallButton.ModalResult := mrOk;
  UninstallButton.Default := True;
  UninstallButton.TabOrder := Form.CancelButton.TabOrder;

  Form.InnerNotebook.ActivePage := Page;
  Form.PageNameLabel.Caption := 'Uninstall Whisperia';
  Form.PageDescriptionLabel.Caption := 'Choose what goes with it.';
  Form.CancelButton.Enabled := True;
  Form.CancelButton.ModalResult := mrCancel;

  if Form.ShowModal = mrCancel then
    Abort;

  UninstallButton.Visible := False;
  Form.CancelButton.Enabled := False;
  Form.InnerNotebook.ActivePage := Form.InstallingPage;
  Form.PageNameLabel.Caption := PageName;
  Form.PageDescriptionLabel.Caption := PageDescription;
end;

procedure InitializeUninstallProgressForm();
var
  Code: DWORD;
begin
  if not UninstallSilent then
    AskWhatToDelete();
  Code := CloseWhisperia();
  Log(Format('Restart Manager closing Whisperia: %d', [Code]));
  if Code <> 0 then
  begin
    SuppressibleMsgBox('Whisperia is running and could not be closed. Quit it and start the uninstaller again.', mbError, MB_OK, IDOK);
    Abort;
  end;
end;

procedure DeleteClientData();
var
  Data: String;
begin
  DelTree(ExpandConstant('{userappdata}\whisperia\config'), True, True, True);
  RemoveDir(ExpandConstant('{userappdata}\whisperia'));
  Data := ExpandConstant('{localappdata}\whisperia\data');
  DeleteFile(Data + '\client.log');
  DeleteFile(Data + '\client.previous.log');
  RemoveDir(Data);
  RemoveDir(ExpandConstant('{localappdata}\whisperia'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and (DeleteSettings <> nil) then
    if DeleteSettings.Checked then
      DeleteClientData();
end;
