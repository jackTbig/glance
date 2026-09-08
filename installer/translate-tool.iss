; translate-tool 安装脚本（Inno Setup 7）
; 由 scripts/build_installer.sh 转成带 BOM 的 UTF-8 后交给 ISCC。
; TargetDir 与 RepoDir 一律由脚本用 /D 传绝对路径进来：.iss 里的相对路径以
; installer\build\ 为基准，很容易算错（第一次就把 D:\DevCache 算成了 D:\My_data\DevCache）。

#define AppName "translate-tool"
#define AppVer "0.1.0"
#define AppExe "translate-tool.exe"
#ifndef TargetDir
  #define TargetDir "D:\DevCache\tt-target\release"
#endif
#ifndef RepoDir
  #define RepoDir "D:\My_data\Claude\translate-tool"
#endif

[Setup]
AppId={{7B3E9C41-2A6D-4F58-9E17-5C0D8A4B6F32}
AppName={#AppName}
AppVersion={#AppVer}
AppPublisher=Jack
DefaultDirName={code:DefaultRoot}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir={#RepoDir}\dist
OutputBaseFilename={#AppName}-setup-{#AppVer}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
UninstallDisplayIcon={app}\{#AppExe}

[Languages]
Name: "zh"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"
Name: "startup"; Description: "开机自动启动（翻译工具常驻托盘）"; GroupDescription: "附加任务:"

[Files]
Source: "{#TargetDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#TargetDir}\translate-tool-service.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoDir}\extension\dist\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#RepoDir}\docs\安装与使用.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\浏览器插件目录"; Filename: "{app}\extension"
Name: "{group}\卸载 {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon
Name: "{userstartup}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Description: "立即启动 {#AppName}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\extension"; Description: "打开插件目录（Chrome 扩展页用「加载已解压的扩展程序」选它）"; Flags: shellexec nowait postinstall skipifsilent unchecked

[Code]
// 有 D 盘就装 D 盘（C 盘紧张）
function DefaultRoot(Param: String): String;
begin
  if DirExists('D:\') then
    Result := 'D:\Program Files'
  else
    Result := ExpandConstant('{autopf}');
end;
