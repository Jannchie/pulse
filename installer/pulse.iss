; Pulse 安装程序（Inno Setup 6.5 及以上，也兼容 7）。
;
; 本地构建：
;   cargo build --release
;   ISCC.exe /DAppVersion=0.3.2 /DIconFile=<target\release\build\pulse-*\out\pulse.ico> installer\pulse.iss
; CI（.github/workflows/release.yml）会传入版本号、图标与输出文件名。
;
; 自动更新时 Pulse 以 /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /LAUNCH=1 运行它，
; /LAUNCH=1 让静默安装结束后重新启动 Pulse（普通的静默部署不会启动）。

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
; 文件版本只能是数字（预发布标签如 0.4.0-rc.1 要去掉后缀）。
#ifndef FileVersion
  #define FileVersion AppVersion
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\pulse.exe"
#endif
#ifndef IconFile
  #define IconFile "pulse.ico"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#ifndef OutputName
  #define OutputName "pulse-v" + AppVersion + "-windows-x64-setup"
#endif

[Setup]
; AppId 决定卸载信息的位置（Pulse 的更新检查也靠它判断是否为安装版），不要修改。
AppId={{99CDD8A9-DE19-4475-9D3B-DEA7657484E0}
AppName=Pulse
AppVersion={#AppVersion}
AppVerName=Pulse {#AppVersion}
AppPublisher=Jannchie
AppPublisherURL=https://github.com/Jannchie/pulse
AppSupportURL=https://github.com/Jannchie/pulse/issues
AppUpdatesURL=https://github.com/Jannchie/pulse/releases
VersionInfoVersion={#FileVersion}
DefaultDirName={autopf}\Pulse
DisableProgramGroupPage=yes
DisableWelcomePage=yes
; 默认为所有用户安装到 Program Files；/CURRENTUSER 可改为当前用户安装（不需要管理员权限）。
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\pulse.exe
UninstallDisplayName=Pulse
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; 用 Restart Manager 关闭正在运行的 Pulse，才能替换 pulse.exe。
CloseApplications=yes
RestartApplications=no
ShowLanguageDialog=auto
LanguageDetectionMethod=uilanguage

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "zh"; MessagesFile: "ChineseSimplified.isl"
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "pulse.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Pulse"; Filename: "{app}\pulse.exe"
Name: "{autodesktop}\Pulse"; Filename: "{app}\pulse.exe"; Tasks: desktopicon

[Registry]
; App Paths：Win+R 或 ShellExecute 可以直接用 pulse 启动。HKA 在所有用户安装时是 HKLM，当前用户安装时是 HKCU。
Root: HKA; Subkey: "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\pulse.exe"; ValueType: string; ValueName: ""; ValueData: "{app}\pulse.exe"; Flags: uninsdeletekey
Root: HKA; Subkey: "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\pulse.exe"; ValueType: string; ValueName: "Path"; ValueData: "{app}"

[Run]
Filename: "{app}\pulse.exe"; Description: "{cm:LaunchProgram,Pulse}"; Flags: nowait postinstall skipifsilent runasoriginaluser
Filename: "{app}\pulse.exe"; Flags: nowait runasoriginaluser; Check: RelaunchAfterUpdate

[Code]
const
  IFEO_TASKMGR = 'SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe';

{ 自动更新传入 /LAUNCH=1：静默安装完成后重新启动 Pulse。 }
function RelaunchAfterUpdate: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:LAUNCH|0}') = '1');
end;

{ Debugger 值是否指向 Exe：Pulse 写入的是 "<exe>" --taskmgr，也接受不带引号的路径。 }
function PointsTo(Value, Exe: String): Boolean;
var
  Rest: String;
begin
  Value := Lowercase(Trim(Value));
  Exe := Lowercase(Exe);
  Result := False;
  if Pos('"' + Exe + '"', Value) = 1 then
    Result := True
  else if Pos(Exe, Value) = 1 then
  begin
    Rest := Copy(Value, Length(Exe) + 1, MaxInt);
    Result := (Rest = '') or (Rest[1] = ' ');
  end;
end;

{ "替换任务管理器"写的 IFEO 值：只在它指向本次卸载的 pulse.exe 时删除，否则保持原样。 }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Value: String;
begin
  if (CurUninstallStep = usUninstall) and
     RegQueryStringValue(HKLM64, IFEO_TASKMGR, 'Debugger', Value) and
     PointsTo(Value, ExpandConstant('{app}\pulse.exe')) then
    RegDeleteValue(HKLM64, IFEO_TASKMGR, 'Debugger');
end;
