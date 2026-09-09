# 简译 Glance 与 live-caption 合并评估

2026-09-09　评估人：translate-tool 会话 + live-caption 会话（实测数据由后者提供）

## 结论先行

**「合并成一个应用」能做，但不能做成一个 exe。**

朴素方案（两套代码编进同一个可执行文件、同一个托盘）被一条硬事实否决：
`live-caption.exe` 的 PE 导入表里 **`vulkan-1.dll` 是 load-time 导入，没有 delay-load**。
合成单 exe 后，任何没有 Vulkan ICD 的机器（干净虚拟机、部分 RDP 会话、服务器 SKU、老显卡驱动）
**连划词取词都启动不了**——Windows 加载器直接 `0xC0000135`，进不到 main。
外加 exe 本身 77 MB（whisper + ggml + Vulkan 静态链接），简译 9.2 MB 的轻量属性被彻底吃掉。

推荐做法是**一个仓库、一个安装包、一个托盘、两个进程**：简译是主进程并持有托盘，
字幕按需被拉起为子进程。用户看到的是一个应用，工程上两边互不拖累。

## 一、事实基础

### 1.1 编译污染：能挡住，靠 package 选择而不是 feature

live-caption 会话搭了最小 workspace 实测（heavy 成员的 `build.rs` 直接 panic 模拟 whisper-rs 的 CMake）：

| 命令 | heavy 的 build.rs |
|---|---|
| `cargo build -p light` | 不触发 |
| `cargo build`（配了 `default-members=["light"]`） | 不触发 |
| `cargo test -p light` | 不触发 |
| `cargo check -p light` | 不触发 |
| `cargo build --workspace` | **触发** |

live-caption 作为独立成员 crate 时，whisper-rs 根本不在 translate-tool 的依赖图里，
resolver v2 的 feature 统一也无从谈起——**所以别设计成 feature 开关，就用两个 crate**。

现在 6 秒的增量编译不会变成「必须先有 Vulkan SDK」，前提是三件事都做到：

1. 根 `Cargo.toml` 写 `default-members`，排除 live-caption，让裸 `cargo build/test` 不碰它。
2. 脚本与 CI 禁用 `--workspace` / `--all`，一律显式 `-p`。
3. **配 `rust-analyzer.check.overrideCommand` 或 `linkedProjects` 限制范围**——
   rust-analyzer 默认跑 `cargo check --workspace`，别人克隆下来一打开编辑器就触发
   whisper 的 CMake，卡在缺 Vulkan SDK 上。**这条最容易漏**：Jack 的机器有 SDK，本地无感。

附带影响（可接受）：Cargo.lock 全 workspace 共享，whisper-rs 及其传递依赖会进锁文件，
`cargo fetch` 会下载它们（只下载不编译），`cargo update` 会解析它们。

### 1.2 环境变量不传染，但 target 目录锁会互相阻塞

`CMAKE_GENERATOR=Ninja` 只被 whisper-rs 的 build.rs 读取，不构建 live-caption 就完全不读，
不会传染给 `-p translate-tool`。**「要不要 Vulkan SDK」完全取决于「有没有人跑 `--workspace`」。**

但共用一个 target 目录意味着共享同一把目录锁：live-caption 一次全量 5–10 分钟，
期间简译的 6 秒增量会阻塞等待。**建议合并后仍给 live-caption 单独的 `CARGO_TARGET_DIR`**。

另外 `[profile.release]` 是 workspace 级的，以后谁改就是全局生效（目前两边都是 `opt-level = 3`，暂无冲突）。

### 1.3 同进程共存的两处必炸

先说好消息：两边依赖版本完全一致（eframe 0.36 glow / egui 0.36 / global-hotkey 0.8 / tray-icon 0.24），
且都被迫用 Glow 渲染器，注释里写的还是同一个原因——Arc 核显上 wgpu 不给带 alpha 的合成模式。
**渲染器不冲突。**

但同进程有两处会炸：

1. **`GlobalHotKeyEvent::receiver()` 与 `TrayIconEvent::receiver()` 是 crate 级进程全局单例通道。**
   live-caption 在 `ui.rs:1267/1281` 轮询，简译在 `ui.rs:327/417` 轮询。合进一个进程后
   两个轮询点互相偷事件，谁先 `try_recv` 谁拿到——表现是**热键和托盘随机失灵**。
   必须改成一个 dispatcher 统一收，再按 hotkey id / tray icon id 路由。
   这是「看着像但语义不同」的典型。

2. **`ViewportCommand::Close` 发到 ROOT 会退出整个进程。**
   live-caption 的 `restart_app`（改识别/切句/热键后重启）和托盘「退出」都这么干
   （`ui.rs:498/1308/1383`），简译 `ui.rs:541` 也是。
   合并后「重启字幕功能」会把取词一起干掉。

还有 ROOT 视口只有一个：简译的 1×1@-32000 宿主窗和 live-caption 的字幕条不能都当 ROOT，
必有一个降级为子视口。而即时子视口（`show_viewport_immediate`）是在父帧里同步渲染的——
**谁当 ROOT，谁的重绘节奏就是全局节奏**。字幕条降级的话，它的 16 ms 动画会把简译的宿主一起拖到 16 ms。

次要但别忘：两边都每秒把窗口位置写回 config，合成一份要处理写竞争；
config 迁移版本号会撞（live-caption 已到 3，简译到 3）。

> **这三处，在「两个进程」方案下全部自动消失。**

### 1.4 可抽走的重叠代码

live-caption 的 `ui.rs` 共 1648 行，去掉测试约 1520 行，**纯基建约 300 行（20%）**：

| 块 | 行数 | 备注 |
|---|---|---|
| 托盘：图标绘制 + `TrackPopupMenu(TPM_RETURNCMD)` + AttachThreadInput 前台接管 + build | 126 | **弹出机制可共用，菜单项列表不能**（暂停/穿透/迷你条是字幕专有），抽的时候把项定义作为参数传进去 |
| 窗口边缘拖拽 resize + 光标 | 29 | |
| `open_path`：ShellExecuteW + AssocQueryStringW 关联判断 + 记事本兜底 | 40 | |
| 字体安装 | 23 | 简译这边还多了 IPA/符号兜底，抽的时候取并集 |
| 设置窗 section/grid 布局助手 | 12 | |
| 设置窗壳：左侧分页 + 底部按钮栏 + 需重启判定 | 62 行里约 40 | |

`ui.rs` 之外还有：config 版本迁移框架（约 40 行）、日志滚动初始化（约 15 行）、
Inno 打包脚本与 BOM 转换（`scripts/build_installer.sh` 整个）。

**必须留在字幕侧**：Line 模型与只增不缩规则 120 行、drain 51 行、五个设置分页内容 258 行、
记录页 161 行、主渲染 276 行。

**三条「看着像但别合」**（live-caption 会话点名）：

- 它的 `needs_restart` 语义是「识别/切句/热键改了要重启进程」，简译这边设置多半即时生效。
  合成一个「保存并重启」按钮会误导用户。
- 它的透明窗 `clear_color` 返回全透明 + 每帧重算高度（`fit_height`），
  跟简译固定尺寸的面板宿主逻辑不能共用。
- 迷你条那套「切换后 2 秒内不写尺寸进配置」的保护是踩坑换来的（展开后会变成窄长条），
  抽窗口几何代码时别顺手删掉。

## 二、方案对比

| | A 单 exe 单托盘 | B 两 exe 两托盘 | **C 两 exe 单托盘（推荐）** | D 单 exe + delay-load |
|---|---|---|---|---|
| 用户体感「一个应用」 | 最强 | 最弱 | 强 | 最强 |
| 没有 Vulkan 的机器 | **取词也起不动** | 各自独立，取词正常 | 取词正常 | 取词正常 |
| exe 体积 | 77 MB | 9.2 + 77 | 9.2 + 77 | 77 MB |
| 全局事件通道打架 | **要改 dispatcher** | 无 | 无 | 要改 dispatcher |
| ROOT Close 误杀 | **要重构** | 无 | 无 | 要重构 |
| 重绘节奏互相拖累 | **有** | 无 | 无 | 有 |
| 额外要写的东西 | dispatcher + ROOT 重构 | 无 | 进程管理 + IPC | 改链接参数、可能改 whisper-rs 构建 |
| 风险 | 高 | 低 | 中低 | **高**（动第三方 crate 构建） |

### 推荐 C：一个仓库、一个安装包、一个托盘、两个进程

- **简译是主进程**，持有唯一的托盘图标与全局热键，菜单里分两块：翻译功能、实时字幕。
- **字幕按需拉起为子进程**，退出时由主进程回收。字幕自己不建托盘（加个 `--managed` 参数）。
- **IPC 复用现成的本地 HTTP 服务**：简译已经有 `127.0.0.1` 上只接受特定来源的服务端，
  加两个内部端点（起/停/查状态）即可，不用引新依赖。
- 安装包做成「简译（9 MB）必选 + 实时字幕（含模型 679 MB）可选组件」。
  没勾字幕的用户装出来还是 9 MB，托盘里那块菜单置灰并提示「未安装」。

这个方案下，1.3 的三处硬约束全部不存在——因为它们的根因都是「同一个进程」。

## 三、分阶段与工作量

| 阶段 | 内容 | 规模 | 可独立交付 |
|---|---|---|---|
| 0 | live-caption 发 1.1.0 正式版、冻结 master、双方开合并分支 | 小 | ✅ 已在进行 |
| 1 | 抽 `crates/desktop-shell`：托盘弹出、`open_path`、字体、设置窗壳、config 迁移框架、日志滚动、打包脚本 | 中（约 400 行迁移 + 两边回归） | ✅ 两边各自仍是独立应用，只是共享了基建 |
| 2 | 合仓：live-caption 进 workspace，配 `default-members` + rust-analyzer 范围 + 独立 target | 小 | ✅ |
| 3 | 统一安装包（组件可选） | 小 | ✅ 此时已经是「一个安装包」 |
| 4 | 统一托盘 + 子进程管理 + 内部端点 | 中 | ✅ 此时才是「一个应用」 |

**阶段 1 就有价值且零风险**：改一处基建两边都受益，不必等整个合并做完。
阶段 3 之后 Jack 就能拿到「一个安装包」，阶段 4 之后才是「一个托盘的一个应用」。
任何一个阶段都可以停下来长期停留，不会留半成品。

## 四、纪律

- **分支**：translate-tool 用 `feature/merge-live-caption`（已建），live-caption 用 `feature/merge-translate-tool`。
  两边 main/master 保持可发布，只接 bugfix。
- **耦合警告**（live-caption 会话提出）：两个仓库现在靠 `../translate-tool` 相对路径耦合。
  **合并分支期间简译这边如果动目录结构或 crate 名，live-caption 的 master 会立刻编译失败**
  （不是静默降级）。动之前必须先说，或者在合并分支上先把 path 依赖换成 workspace 内部依赖。
- **副产品**：live-caption 安装脚本里「缺 `vulkan-1.dll` 只提示不阻断」这句是乐观的——
  真缺了 exe 根本起不来。live-caption 会话会改掉这个提示。

## 五、待 Jack 决策

1. 采纳 C 方案，还是接受 B（两个托盘、最省事）？
2. 阶段 1 现在就做，还是等 live-caption 1.1.0 发完再动？
3. 合并版版本号：简译当前 0.2.0，live-caption 即将 1.1.0。合并后的应用建议直接 2.0.0（比两边都高，语义不冲突）。
