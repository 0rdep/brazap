; Per-user Windows installer for Brazap. Build it with scripts/package-windows, which
; passes the version, executable and icon:
;   ISCC /DAppVersion=0.1.22 /DNumericVersion=0.1.22 /DSourceExe=... /DIconFile=... /DOutputDir=... brazap.iss
;
; The install directory is writable by the user, so the in-app updater keeps replacing
; brazap.exe in place. The Start Menu shortcut is the same file, AppUserModelID and toast
; activator that src/windows_integration.rs maintains for notifications, so there is
; only ever one Brazap entry. User data lives in a separate profile directory and is
; never removed by the uninstaller.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef NumericVersion
  #define NumericVersion AppVersion
#endif
#ifndef SourceExe
  #error Pass /DSourceExe=<path to brazap.exe>
#endif
#ifndef IconFile
  #error Pass /DIconFile=<path to brazap.ico>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
; Never change AppId: it identifies existing installations for upgrades and uninstall.
; src/updates.rs uses it to keep the installed version shown in Windows current.
AppId={{85DF23AD-C3D8-448E-B9A6-D3F30AAF4D43}
AppName=Brazap
AppVersion={#AppVersion}
AppVerName=Brazap {#AppVersion}
AppPublisher=Brazap
AppPublisherURL=https://github.com/0rdep/brazap
AppSupportURL=https://github.com/0rdep/brazap/issues
AppUpdatesURL=https://github.com/0rdep/brazap/releases
DefaultDirName={autopf}\Brazap
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=brazap-windows-x86_64-setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\brazap.exe
UninstallDisplayName=Brazap
VersionInfoVersion={#NumericVersion}
VersionInfoProductName=Brazap
VersionInfoDescription=Brazap Setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern dynamic
ShowLanguageDialog=auto
; Upgrades close a running Brazap (including a tray or headless instance) that holds
; brazap.exe open. The finished page offers to start it again.
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "brazap.exe"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Brazap"; Filename: "{app}\brazap.exe"; AppUserModelID: "Brazap.Desktop"; AppUserModelToastActivatorCLSID: "C50284DA-78B7-45B0-A68D-C89FC01EDC21"
Name: "{autodesktop}\Brazap"; Filename: "{app}\brazap.exe"; Tasks: desktopicon; AppUserModelID: "Brazap.Desktop"

[Run]
Filename: "{app}\brazap.exe"; Description: "{cm:LaunchProgram,Brazap}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The in-app updater keeps the previous executable for recovery.
Type: files; Name: "{app}\brazap.exe.previous"

[Code]
function OpensThisCopy(const Key: String): Boolean;
var
  Command: String;
begin
  Result := RegQueryStringValue(HKCU, Key + '\shell\open\command', '', Command) and
    (Pos(Lowercase('"' + ExpandConstant('{app}\brazap.exe') + '"'), Lowercase(Command)) = 1);
end;

procedure StopThisCopy;
var
  Path: String;
  Code: Integer;
begin
  Path := ExpandConstant('{app}\brazap.exe');
  StringChangeEx(Path, '''', '''''', True);
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -Command "Get-Process brazap -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.Path -eq ''' + Path + ''' } | Stop-Process -Force; Start-Sleep -Milliseconds 500"',
    '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopThisCopy;
  if CurUninstallStep <> usPostUninstall then
    exit;
  { Brazap registers these on launch. Remove them only while they still point at this
    copy, so a separately run Brazap keeps its own registrations. }
  if OpensThisCopy('Software\Classes\brazap') then
  begin
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\brazap');
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\AppUserModelId\Brazap.Desktop');
  end;
  if OpensThisCopy('Software\Classes\Brazap.WhatsAppLink') then
  begin
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\Brazap.WhatsAppLink');
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Brazap');
    RegDeleteValue(HKCU, 'Software\RegisteredApplications', 'Brazap');
  end;
  if OpensThisCopy('Software\Classes\whatsapp') then
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\whatsapp');
end;
