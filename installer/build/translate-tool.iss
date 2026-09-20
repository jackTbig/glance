; 简译 Glance 安装脚本（Inno Setup 7）
; 由 scripts/build_installer.sh 转成带 BOM 的 UTF-8 后交给 ISCC。
; TargetDir / CaptionDir / RepoDir / CaptionStamp / TurboSize / SmallSize 一律由脚本用 /D 传进来：
; .iss 里的相对路径以 installer\build\ 为基准，很容易算错（第一次就把 D:\DevCache 算成了 D:\My_data\DevCache）。
;
; 一个安装包装两个程序：简译（必装，约 9 MB）+ 实时字幕（可选，含模型约 700 MB）。
; **两个 exe 而不是一个**：字幕的 vulkan-1.dll 是加载期导入，合成单 exe 后没有
; Vulkan ICD 的机器连划词取词都起不来（0xC0000135）。
;
; 升级（覆盖安装）的三条规矩，Jack 2026-09-11 定：
;   1. 配置一律保留。两个应用的配置都在 %APPDATA% 下，本脚本任何地方都不许写或删那里。
;   2. 装之前明确告诉用户「会覆盖旧版本」，以及哪些组件无需升级。
;   3. 没变的组件不重装：字幕按源码指纹比对（scripts/caption_stamp.py），模型按文件名 + 大小比对。

; AppName 是**文件名与目录名**用的短名：无空格，中英文名都在。
; 界面上显示的全名走 AppNameZh。crate 与仓库名仍是 translate-tool，两者不必一致。
#define AppName "简译Glance"
#define AppNameZh "简译 Glance"
#define AppVer "1.0.6"
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
; 字幕源码指纹：拿不到就是 unknown，一律按「有更新」处理
#ifndef CaptionStamp
  #define CaptionStamp "unknown"
#endif
; 模型字节数：0 表示不知道，一律重新拷
#ifndef TurboSize
  #define TurboSize "0"
#endif
#ifndef SmallSize
  #define SmallSize "0"
#endif
#define ModelTurbo "D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin"
#define ModelSmall "D:\models\whisper\ggml-base.en.bin"

[Setup]
; **AppId 不许动**：Inno 靠它认「是不是同一个应用」，改了升级就会并存两份。
; [Code] 里查旧版卸载记录用的是同一个 GUID，build_installer.sh 会核对两处一致。
AppId={{7B3E9C41-2A6D-4F58-9E17-5C0D8A4B6F32}
AppName={#AppNameZh}
AppVersion={#AppVer}
AppPublisher=Jack
DefaultDirName={code:DefaultRoot}\{#AppName}
DefaultGroupName={#AppNameZh}
DisableProgramGroupPage=yes
; 升级时沿用上次的安装位置、组件选择与附加任务（开机自启、桌面快捷方式）
UsePreviousAppDir=yes
UsePreviousSetupType=yes
UsePreviousTasks=yes
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
; 程序开着时覆盖不了 exe：用两个应用的单实例互斥量拦住，让用户先退出
; （名字见 desktop_shell::single_instance::claim 的调用处，前缀 Global\）
AppMutex=Global\translate-tool-single-instance,Global\live-caption-managed-single
; 升级出问题时排查用：日志在 %TEMP%\Setup Log *.txt，升级提示页的全文与跳过了哪些组件都会记进去
SetupLogging=yes

[Languages]
Name: "chs"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Types]
Name: "compact"; Description: "只装简译（划词、悬停、截屏、整页翻译，约 9 MB）"
Name: "full"; Description: "简译 + 实时字幕（含识别模型，约 700 MB）"
Name: "custom"; Description: "自定义"; Flags: iscustom

; 顺序别动：[Code] 的 CurPageChanged 按下标 0..3 给组件页标注「无需升级」
[Components]
Name: "glance"; Description: "简译（翻译工具，必装）"; Types: compact full custom; Flags: fixed
Name: "dict"; Description: "离线词典（29 MB，模型慢或断网时先给本地释义）"; Types: compact full custom
Name: "caption"; Description: "实时字幕（本机识别英文语音并翻译，需要支持 Vulkan 的显卡）"; Types: full custom
Name: "caption\turbo"; Description: "large-v3-turbo 模型（最终句定稿，最准，574 MB）"; Types: full custom
Name: "caption\small"; Description: "base.en 模型（说话过程中逐字上屏，最快，148 MB）"; Types: full custom

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"
Name: "startup"; Description: "开机自动启动（简译常驻托盘）"; GroupDescription: "附加任务:"

[Files]
Source: "{#TargetDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion; Components: glance
Source: "{#TargetDir}\translate-tool-service.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: glance
Source: "{#TargetDir}\tt-doctor.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: glance
; 内嵌运行库不随安装删除：装完之后自检页要能一键重装它（{tmp} 那份装完就没了）
Source: "{#RepoDir}\apps\caption\installer\deps\vc_redist.x64.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: caption
Source: "{#RepoDir}\extension\dist\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs; Components: glance
Source: "{#RepoDir}\docs\安装与使用.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme; Components: glance
Source: "{#RepoDir}\docs\第三方组件授权.md"; DestDir: "{app}"; Flags: ignoreversion; Components: glance
; 离线词库：内容取自 ECDICT（MIT），同名同大小就跳过——它不会原地改，改了必然换版本
Source: "{#DictBin}"; DestDir: "{app}\dict"; DestName: "ecdict.bin"; Flags: ignoreversion; Components: dict; Check: DictNeedsCopy({#DictSize})
; 字幕的 exe 就放在简译旁边——`caption::find_exe` 头一个就找这里。与已装的源码指纹一致就跳过
Source: "{#CaptionDir}\{#CaptionExe}"; DestDir: "{app}"; Flags: ignoreversion; Components: caption; Check: CaptionNeedsCopy
Source: "{#RepoDir}\apps\caption\config.example.toml"; DestDir: "{app}"; Flags: ignoreversion; Components: caption
; 模型从不原地改（换模型必换文件名），已装的同名同大小就跳过，省下 700 多 MB 的解压
Source: "{#ModelTurbo}"; DestDir: "{app}\models"; Flags: ignoreversion; Components: caption\turbo; Check: ModelNeedsCopy('ggml-large-v3-turbo-q5_0.bin', {#TurboSize})
Source: "{#ModelSmall}"; DestDir: "{app}\models"; Flags: ignoreversion; Components: caption\small; Check: ModelNeedsCopy('ggml-base.en.bin', {#SmallSize})
Source: "{#RepoDir}\apps\caption\installer\deps\vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Components: caption; Check: VCRedistNeedsInstall

[Icons]
Name: "{group}\{#AppNameZh}"; Filename: "{app}\{#AppExe}"
Name: "{group}\浏览器插件目录"; Filename: "{app}\extension"
Name: "{group}\运行环境自检"; Filename: "{app}\tt-doctor.exe"
Name: "{group}\卸载 {#AppNameZh}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppNameZh}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon
Name: "{userstartup}\{#AppNameZh}"; Filename: "{app}\{#AppExe}"; Tasks: startup

[Run]
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "正在安装 VC++ 运行库..."; Components: caption; Check: VCRedistNeedsInstall; Flags: waituntilterminated
Filename: "{app}\{#AppExe}"; Description: "立即启动 {#AppNameZh}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\extension"; Description: "打开插件目录（Chrome 扩展页用「加载已解压的扩展程序」选它）"; Flags: shellexec nowait postinstall skipifsilent unchecked

; 卸载只清安装目录里的东西。**不许加 %APPDATA% 下的任何路径**：
; 那里是用户的配置、Key、术语表与会议记录，卸了重装还要接着用。
[UninstallDelete]
Type: filesandordirs; Name: "{app}\dict"
Type: filesandordirs; Name: "{app}\models"
Type: files; Name: "{app}\live-caption.stamp"

[Code]
const
  // 与 [Setup] 的 AppId 同一个 GUID（build_installer.sh 会核对）。Inno 把卸载记录写在这个键下
  UninstKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{7B3E9C41-2A6D-4F58-9E17-5C0D8A4B6F32}_is1';
  TurboName = 'ggml-large-v3-turbo-q5_0.bin';
  SmallName = 'ggml-base.en.bin';

var
  // 同一种安装方式（为所有用户 / 仅为我）下的旧版：这才是本次会被覆盖的那份
  PrevFound: Boolean;
  PrevVersion, PrevDir: String;
  // 另一种安装方式下的旧版：本次不会覆盖它，而是另装一份
  OtherFound: Boolean;
  OtherVersion, OtherDir: String;
  CaptionPresent, CaptionUpToDate, TurboPresent, SmallPresent, DictPresent: Boolean;
  ComponentsTagged: Boolean;
  // Inno 会对同一个文件多次调用 Check，「跳过」只记一次日志，免得一条刷四遍
  LoggedCaptionSkip, LoggedTurboSkip, LoggedSmallSkip: Boolean;

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

// ---------------- 升级检测 ----------------

function ReadInstallRecord(Root: Integer; var Ver, Dir: String): Boolean;
begin
  Dir := '';
  Result := RegQueryStringValue(Root, UninstKey, 'Inno Setup: App Path', Dir) and (Dir <> '');
  if Result then
    if not RegQueryStringValue(Root, UninstKey, 'DisplayVersion', Ver) then
      Ver := '（版本未知）';
end;

function InstalledCaptionStamp(Dir: String): String;
var
  S: AnsiString;
begin
  Result := '';
  if LoadStringFromFile(AddBackslash(Dir) + 'live-caption.stamp', S) then
    Result := Trim(String(S));
end;

// 已装的字幕与本安装包里的是同一份源码编出来的吗。
// 1.0.1 及更早的安装没有指纹文件，会被判「有更新」覆盖一次，之后就准了。
function CaptionSameAs(Dir: String): Boolean;
begin
  Result := ('{#CaptionStamp}' <> 'unknown')
    and FileExists(AddBackslash(Dir) + '{#CaptionExe}')
    and (InstalledCaptionStamp(Dir) = '{#CaptionStamp}');
end;

function ModelPresentIn(Dir, Name: String; Size: Integer): Boolean;
var
  S: Integer;
begin
  Result := (Size > 0) and FileSize(AddBackslash(Dir) + 'models\' + Name, S) and (S = Size);
end;

// [Files] 的 Check：返回 False 就跳过这个文件
function CaptionNeedsCopy(): Boolean;
begin
  Result := not CaptionSameAs(ExpandConstant('{app}'));
  if (not Result) and (not LoggedCaptionSkip) then
  begin
    LoggedCaptionSkip := True;
    Log('实时字幕与本安装包一致，跳过 {#CaptionExe}（无需升级）');
  end;
end;

var LoggedDictSkip: Boolean;

/// 词库已经在且大小一样就不重装：它不会原地改，改了必然换版本，29 MB 解压能省则省
/// 旧安装目录里已经有同样大小的词库吗。与 `DictNeedsCopy` 同一套判据，
/// 区别只是查的目录不同：这个给升级提示页用，那个给拷贝时用
function DictPresentIn(Dir: String; Size: Integer): Boolean;
var
  Found: TFindRec;
begin
  Result := False;
  if FindFirst(AddBackslash(Dir) + 'dict\ecdict.bin', Found) then
  begin
    try
      Result := (Found.SizeLow = Size) and (Size > 0);
    finally
      FindClose(Found);
    end;
  end;
end;

function DictNeedsCopy(Size: Integer): Boolean;
var
  Found: TFindRec;
begin
  Result := True;
  if FindFirst(ExpandConstant('{app}\dict\ecdict.bin'), Found) then
  begin
    try
      if Found.SizeLow = Size then
        Result := False;
    finally
      FindClose(Found);
    end;
  end;
  if Result or LoggedDictSkip then
    exit;
  LoggedDictSkip := True;
  Log('离线词库已存在且大小一致，跳过（无需升级）');
end;

function ModelNeedsCopy(Name: String; Size: Integer): Boolean;
begin
  Result := not ModelPresentIn(ExpandConstant('{app}'), Name, Size);
  if Result then
    exit;
  if ((Name = TurboName) and LoggedTurboSkip) or ((Name = SmallName) and LoggedSmallSkip) then
    exit;
  if Name = TurboName then
    LoggedTurboSkip := True
  else
    LoggedSmallSkip := True;
  Log('识别模型 ' + Name + ' 已存在且大小一致，跳过（无需升级）');
end;

// 按十进制、四舍五入算，与组件页写的「574 MB / 148 MB」对得上：
// 按 1024 算会显示 547 / 141；直接整除会把 147.96 截成 147，用户都对不上号
function SizeText(Bytes: Integer): String;
begin
  Result := IntToStr((Bytes + 500000) div 1000000) + ' MB';
end;

function UpgradeText(): String;
var
  NL, AppLine, CapLine, DictLine, TurboLine, SmallLine, AppData: String;
begin
  NL := #13#10;
  AppData := ExpandConstant('{userappdata}');
  if PrevVersion = '{#AppVer}' then
    AppLine := '{#AppVer}，同版本重新安装（覆盖）'
  else
    AppLine := PrevVersion + ' → {#AppVer}，覆盖升级';

  if not CaptionPresent then
    CapLine := '未安装（需要的话可在「选择组件」页勾选）'
  else if CaptionUpToDate then
    CapLine := '已是最新，无需升级（跳过）'
  else
    CapLine := '有更新，会随本次安装一并覆盖';

  if DictPresent then
    DictLine := '已安装，无需升级（跳过 ' + SizeText({#DictSize}) + '）'
  else
    DictLine := '未安装（需要的话可在「选择组件」页勾选）';

  if TurboPresent then
    TurboLine := '已安装，无需升级（跳过 ' + SizeText({#TurboSize}) + '）'
  else
    TurboLine := '未安装';
  if SmallPresent then
    SmallLine := '已安装，无需升级（跳过 ' + SizeText({#SmallSize}) + '）'
  else
    SmallLine := '未安装';

  Result :=
    '检测到已安装：{#AppNameZh} ' + PrevVersion + NL +
    '安装位置：' + PrevDir + NL +
    '本次会覆盖安装到同一位置，开始菜单与卸载记录沿用原来的，不会并存两份。' + NL + NL +
    '■ 以下内容全部保留，不会改动' + NL +
    '  · 简译的设置（翻译服务与 Key、快捷键、术语表等）：' + AppData + '\translate-tool' + NL +
    '  · 实时字幕的设置与会议记录：' + AppData + '\live-caption' + NL +
    '  · Chrome 插件里的设置（保存在浏览器里，与安装包无关）' + NL +
    '  · 上次选择的开机自启、桌面快捷方式' + NL + NL +
    '■ 各组件' + NL +
    '  · 简译（主程序）：' + AppLine + NL +
    '  · 离线词典：' + DictLine + NL +
    '  · 实时字幕：' + CapLine + NL +
    '  · 识别模型 large-v3-turbo：' + TurboLine + NL +
    '  · 识别模型 base.en：' + SmallLine + NL + NL +
    '■ 请注意' + NL +
    '  · 继续之前，请先在托盘图标上右键「退出」简译；实时字幕开着的话会随之退出' + NL +
    '  · 装完到 Chrome 扩展页，点插件卡片上的刷新按钮，插件才会换成新版本';
end;

function OtherModeText(): String;
var
  NL, Chosen, ToChoose: String;
begin
  NL := #13#10;
  if IsAdminInstallMode then
  begin
    Chosen := '为所有用户安装';
    ToChoose := '仅为我安装';
  end
  else
  begin
    Chosen := '仅为我安装';
    ToChoose := '为所有用户安装';
  end;
  Result :=
    '本机已经装有一份 {#AppNameZh} ' + OtherVersion + '，是用「' + ToChoose + '」方式装的：' + NL +
    '位置：' + OtherDir + NL + NL +
    '你这次选的是「' + Chosen + '」，不会覆盖那一份，而是在另一个位置再装一份——' + NL +
    '两份并存，开始菜单里也会出现两个。' + NL + NL +
    '建议：点「取消」，重新运行安装包，在开头选择安装方式时选「' + ToChoose + '」，' + NL +
    '就会覆盖升级原来那一份。' + NL + NL +
    '无论选哪种方式，你的设置都保存在 ' + ExpandConstant('{userappdata}') + ' 下，不会丢。';
end;

procedure InitializeWizard();
var
  SameRoot, OtherRoot: Integer;
  Page: TWizardPage;
  Memo: TNewMemo;
  Txt, Caption, Desc: String;
begin
  // Inno 覆盖的是「同一种安装方式」下的那份记录：为所有用户装在 HKLM，仅为我装在 HKCU
  if IsAdminInstallMode then
  begin
    SameRoot := HKLM;
    OtherRoot := HKCU;
  end
  else
  begin
    SameRoot := HKCU;
    OtherRoot := HKLM;
  end;
  PrevFound := ReadInstallRecord(SameRoot, PrevVersion, PrevDir);
  OtherFound := (not PrevFound) and ReadInstallRecord(OtherRoot, OtherVersion, OtherDir);

  if PrevFound then
  begin
    CaptionPresent := FileExists(AddBackslash(PrevDir) + '{#CaptionExe}');
    CaptionUpToDate := CaptionSameAs(PrevDir);
    TurboPresent := ModelPresentIn(PrevDir, TurboName, {#TurboSize});
    SmallPresent := ModelPresentIn(PrevDir, SmallName, {#SmallSize});
    DictPresent := DictPresentIn(PrevDir, {#DictSize});
    Caption := '覆盖升级';
    Desc := '检测到本机已安装 {#AppNameZh}，本次会覆盖旧版本，你的配置全部保留';
    Txt := UpgradeText();
  end
  else if OtherFound then
  begin
    Caption := '注意：会另装一份';
    Desc := '本机已有另一种安装方式装的 {#AppNameZh}';
    Txt := OtherModeText();
  end
  else
    exit;

  // 静默安装看不到页面，全文记进安装日志，排查升级问题时有据可查
  Log(Caption + #13#10 + Txt);

  Page := CreateCustomPage(wpWelcome, Caption, Desc);
  Memo := TNewMemo.Create(Page);
  Memo.Parent := Page.Surface;
  Memo.SetBounds(0, 0, Page.SurfaceWidth, Page.SurfaceHeight);
  Memo.ReadOnly := True;
  Memo.ScrollBars := ssVertical;
  Memo.WordWrap := True;
  Memo.Text := Txt;
end;

// 组件页上逐项标出升级情况。下标按 [Components] 的顺序：0 简译 / 1 字幕 / 2 turbo / 3 base.en
procedure CurPageChanged(CurPageID: Integer);
var
  L: TNewCheckListBox;
begin
  if (CurPageID = wpSelectComponents) and PrevFound and (not ComponentsTagged) then
  begin
    // 来回翻页会多次进这里，只标一次，免得后缀越叠越长
    ComponentsTagged := True;
    L := WizardForm.ComponentsList;
    // 下标对应 [Components] 的行序：0 简译 / 1 离线词典 / 2 字幕 / 3 turbo 模型 / 4 base.en 模型。
    // **加组件要同步改这里**，不然标注会挂到隔壁那一行上
    L.ItemCaption[0] := L.ItemCaption[0] + '　（覆盖升级）';
    if DictPresent then
      L.ItemCaption[1] := L.ItemCaption[1] + '　（已安装，无需升级）';
    if CaptionUpToDate then
      L.ItemCaption[2] := L.ItemCaption[2] + '　（已是最新，无需升级）';
    if TurboPresent then
      L.ItemCaption[3] := L.ItemCaption[3] + '　（已安装，无需升级）';
    if SmallPresent then
      L.ItemCaption[4] := L.ItemCaption[4] + '　（已安装，无需升级）';
  end;
end;

// 装之前先把依赖结论摆给用户。**只检不装**：能自动装的（VC++ 运行库）在 [Run] 里装，
// 装不了的（Vulkan 随显卡驱动来）说清影响，让用户自己决定要不要继续。
//
// 只在勾了字幕组件时才提 Vulkan：简译本身不碰它，对只装简译的用户弹这个框纯属吓人。
function DepsReport(): String;
var
  S: String;
begin
  S := '';
  if WizardIsComponentSelected('caption') then
  begin
    if not FileExists(ExpandConstant('{sys}\vulkan-1.dll')) then
      S := S + 'X Vulkan 运行库：未检测到 vulkan-1.dll' + #13#10 +
               '   影响：「实时字幕」完全无法启动——不是没有 GPU 加速，它是加载期依赖，' + #13#10 +
               '   缺了 Windows 会直接拒绝启动该程序。简译（翻译工具）本身不受影响。' + #13#10 +
               '   办法：更新显卡驱动。它随驱动一起装，没有独立的运行库安装包。' + #13#10 + #13#10
    else
      S := S + 'OK Vulkan 运行库：已就绪' + #13#10;
    if VCRedistNeedsInstall then
      S := S + '!  VC++ 2015-2022 运行库：未安装，本安装包会自动装（已内嵌，无需联网）' + #13#10
    else
      S := S + 'OK VC++ 2015-2022 运行库：已安装' + #13#10;
  end;
  Result := S;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  R: String;
begin
  if CurStep = ssInstall then
  begin
    R := DepsReport();
    if (Pos('X ', R) > 0) and (not WizardSilent) then
      if MsgBox('安装前环境检查：' + #13#10 + #13#10 + R + #13#10 +
                '标 X 的项会让对应功能不可用。仍要继续安装吗？' + #13#10 +
                '（装完后可用开始菜单里的「运行环境自检」随时复查，' + #13#10 +
                '或在简译设置窗的「运行环境」页里逐项一键修复）',
                mbConfirmation, MB_YESNO) = IDNO then
        Abort();
  end;
  // 记下这次装的字幕是哪份源码编的，下次升级据此判断要不要覆盖
  if CurStep = ssPostInstall then
    if WizardIsComponentSelected('caption') and FileExists(ExpandConstant('{app}\{#CaptionExe}')) then
      SaveStringToFile(ExpandConstant('{app}\live-caption.stamp'), '{#CaptionStamp}', False);
end;
