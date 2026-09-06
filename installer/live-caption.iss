; live-caption 安装包（Inno Setup 7）
; 由 scripts/build_installer.sh 编译；本文件带 BOM 的 UTF-8 由构建脚本生成到 build/ 下再喂给 ISCC
#define AppName "live-caption"
#define AppVer "1.0.2"
#define ExeSrc "D:\DevCache\lc-target\release\live-caption.exe"
#define ModelTurbo "D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin"
#define ModelSmall "D:\models\whisper\ggml-small.en.bin"

[Setup]
AppId={{7E1C2C1A-5B6E-4D8F-9C2B-4A1F0C9D2E31}
AppName={#AppName}
AppVersion={#AppVer}
AppPublisher=JackT
DefaultDirName={code:DefaultDir}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; 相对路径以本脚本的编译位置 installer\build\ 为基准
OutputDir=..\..\dist
OutputBaseFilename=live-caption-setup-{#AppVer}
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
; 允许用 /CURRENTUSER 装到用户目录（不提权；此时 VC++ 运行库若缺失需自行安装），或安装时弹框选择
PrivilegesRequiredOverridesAllowed=commandline dialog
UninstallDisplayIcon={app}\live-caption.exe
WizardStyle=modern
ShowLanguageDialog=no

[Languages]
Name: "chs"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Types]
Name: "full"; Description: "完整安装（最终句模型 + 逐字上屏模型，推荐）"
Name: "compact"; Description: "精简安装（只装最终句模型，逐字上屏也用它，稍慢）"
Name: "custom"; Description: "自定义"; Flags: iscustom

[Components]
Name: "app"; Description: "主程序"; Types: full compact custom; Flags: fixed
Name: "models"; Description: "识别模型（至少装一个）"; Types: full compact custom
Name: "models\turbo"; Description: "large-v3-turbo（最终句定稿，最准，574 MB）"; Types: full compact custom
Name: "models\small"; Description: "small.en（说话过程中逐字上屏用，最快，488 MB）"; Types: full custom

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："
Name: "installclaude"; Description: "安装 Claude Code CLI（翻译默认走它，需要网络；已装可不勾）"; GroupDescription: "依赖："; Flags: unchecked

[Files]
Source: "{#ExeSrc}"; DestDir: "{app}"; Flags: ignoreversion; Components: app
Source: "..\..\config.example.toml"; DestDir: "{app}"; Flags: ignoreversion; Components: app
Source: "..\..\README-安装版.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme; Components: app
Source: "{#ModelTurbo}"; DestDir: "{app}\models"; Flags: ignoreversion; Components: models\turbo
Source: "{#ModelSmall}"; DestDir: "{app}\models"; Flags: ignoreversion; Components: models\small
Source: "..\deps\vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: VCRedistNeedsInstall

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\live-caption.exe"; WorkingDir: "{app}"
Name: "{group}\卸载 {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\live-caption.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "正在安装 VC++ 运行库..."; Check: VCRedistNeedsInstall; Flags: waituntilterminated
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -Command ""irm https://claude.ai/install.ps1 | iex"""; StatusMsg: "正在安装 Claude Code CLI..."; Tasks: installclaude; Flags: waituntilterminated runasoriginaluser
Filename: "{app}\live-caption.exe"; Description: "现在启动 {#AppName}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallDelete]
Type: filesandordirs; Name: "{app}\models"

[Code]
function DefaultDir(Param: String): String;
begin
  if not IsAdminInstallMode then
    Result := ExpandConstant('{userpf}\live-caption')
  else if DirExists('D:\') then
    Result := 'D:\Program Files\live-caption'
  else
    Result := ExpandConstant('{autopf}\live-caption');
end;

function VCRedistNeedsInstall: Boolean;
var
  Installed: Cardinal;
begin
  Result := True;
  if RegQueryDWordValue(HKLM, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Installed', Installed) then
    if Installed = 1 then
      Result := False;
end;

function InitializeSetup: Boolean;
begin
  Result := True;
  if not FileExists(ExpandConstant('{sys}\vulkan-1.dll')) then
    MsgBox('未检测到 Vulkan 运行库（vulkan-1.dll）。它随显卡驱动一起安装，请先更新显卡驱动，否则识别无法使用 GPU。安装会继续。', mbInformation, MB_OK);
end;
