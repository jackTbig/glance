; 简译 Glance 安装脚本（Inno Setup 7）
; 由 scripts/build_installer.sh 转成带 BOM 的 UTF-8 后交给 ISCC。
; TargetDir / CaptionDir / RepoDir 一律由脚本用 /D 传绝对路径进来：.iss 里的相对路径以
; installer\build\ 为基准，很容易算错（第一次就把 D:\DevCache 算成了 D:\My_data\DevCache）。
;
; 一个安装包装两个程序：简译（必装，约 9 MB）+ 实时字幕（可选，含模型约 700 MB）。
; **两个 exe 而不是一个**：字幕的 vulkan-1.dll 是加载期导入，合成单 exe 后没有
; Vulkan ICD 的机器连划词取词都起不来（0xC0000135）。

#define AppName "translate-tool"
#define AppNameZh "简译 Glance"
#define AppVer "0.3.0"
#define AppExe "translate-tool.exe"
#define CaptionExe "live-caption.exe"
#ifndef TargetDir
  #define TargetDir "D:\DevCache\tt-target\release"
#endif
#ifndef CaptionDir
  #define CaptionDir "D:\DevCache\lc-target\release"
#endif
#ifndef RepoDir
  #define RepoDir "D:\My_data\Claude\translate-tool"
#endif
#define ModelTurbo "D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin"
#define ModelSmall "D:\models\whisper\ggml-base.en.bin"

[Setup]
AppId={{7B3E9C41-2A6D-4F58-9E17-5C0D8A4B6F32}
AppName={#AppNameZh}
AppVersion={#AppVer}
AppPublisher=Jack
DefaultDirName={code:DefaultRoot}\{#AppName}
DefaultGroupName={#AppNameZh}
DisableProgramGroupPage=yes
OutputDir={#RepoDir}\dist
OutputBaseFilename={#AppName}-setup-{#AppVer}
Compression=lzma2/max
SolidCompression=yes
LZMAUseSeparateProcess=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
UninstallDisplayIcon={app}\{#AppExe}

[Languages]
Name: "zh"; MessagesFile: "compiler:Default.isl"

[Types]
Name: "compact"; Description: "只装简译（划词、悬停、截屏、整页翻译，约 9 MB）"
Name: "full"; Description: "简译 + 实时字幕（含识别模型，约 700 MB）"
Name: "custom"; Description: "自定义"; Flags: iscustom

[Components]
Name: "glance"; Description: "简译（翻译工具，必装）"; Types: compact full custom; Flags: fixed
Name: "caption"; Description: "实时字幕（本机识别英文语音并翻译，需要支持 Vulkan 的显卡）"; Types: full custom
Name: "caption\turbo"; Description: "large-v3-turbo 模型（最终句定稿，最准，574 MB）"; Types: full custom
Name: "caption\small"; Description: "base.en 模型（说话过程中逐字上屏，最快，148 MB）"; Types: full custom

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"
Name: "startup"; Description: "开机自动启动（简译常驻托盘）"; GroupDescription: "附加任务:"

[Files]
Source: "{#TargetDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion; Components: glance
Source: "{#TargetDir}\translate-tool-service.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: glance
Source: "{#RepoDir}\extension\dist\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs; Components: glance
Source: "{#RepoDir}\docs\安装与使用.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme; Components: glance
; 字幕的 exe 就放在简译旁边——`caption::find_exe` 头一个就找这里
Source: "{#CaptionDir}\{#CaptionExe}"; DestDir: "{app}"; Flags: ignoreversion; Components: caption
Source: "{#RepoDir}\apps\caption\config.example.toml"; DestDir: "{app}"; Flags: ignoreversion; Components: caption
Source: "{#ModelTurbo}"; DestDir: "{app}\models"; Flags: ignoreversion; Components: caption\turbo
Source: "{#ModelSmall}"; DestDir: "{app}\models"; Flags: ignoreversion; Components: caption\small
Source: "{#RepoDir}\apps\caption\installer\deps\vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Components: caption; Check: VCRedistNeedsInstall

[Icons]
Name: "{group}\{#AppNameZh}"; Filename: "{app}\{#AppExe}"
Name: "{group}\浏览器插件目录"; Filename: "{app}\extension"
Name: "{group}\卸载 {#AppNameZh}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppNameZh}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon
Name: "{userstartup}\{#AppNameZh}"; Filename: "{app}\{#AppExe}"; Tasks: startup

[Run]
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "正在安装 VC++ 运行库..."; Components: caption; Check: VCRedistNeedsInstall; Flags: waituntilterminated
Filename: "{app}\{#AppExe}"; Description: "立即启动 {#AppNameZh}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\extension"; Description: "打开插件目录（Chrome 扩展页用「加载已解压的扩展程序」选它）"; Flags: shellexec nowait postinstall skipifsilent unchecked

[UninstallDelete]
Type: filesandordirs; Name: "{app}\models"

[Code]
// 有 D 盘就装 D 盘（C 盘紧张）
function DefaultRoot(Param: String): String;
begin
  if not IsAdminInstallMode then
    Result := ExpandConstant('{userpf}')
  else if DirExists('D:\') then
    Result := 'D:\Program Files'
  else
    Result := ExpandConstant('{autopf}');
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

// 只在勾了字幕组件时才提醒 Vulkan。简译本身不碰 Vulkan，
// 对只装简译的用户弹这个框纯属吓人。
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    if IsComponentSelected('caption') then
      if not FileExists(ExpandConstant('{sys}\vulkan-1.dll')) then
        MsgBox('未检测到 Vulkan 运行库（vulkan-1.dll）。它随显卡驱动一起安装。' + #13#10 +
               '缺少它「实时字幕」将无法启动——不是只是没有 GPU 加速，vulkan-1.dll 是它的加载期依赖。' + #13#10 +
               '简译本身不受影响。请先更新显卡驱动。安装会继续。', mbInformation, MB_OK);
end;
