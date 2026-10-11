# 文件翻译功能（一期：docx / markdown / txt）

> 日期：2026-10-11 · 状态：待 Jack 审核

## 目标

让简译 Glance 能直接翻译本地 `.docx / .md / .txt`，译文另起一份带 `.zh.` 后缀留在同目录，原文件不动。

## 范围（一期）

- 入口：托盘菜单「翻译文件…」（Windows 文件选择对话框，允许多选）
- 格式：`.docx / .md / .txt`（UTF-8 / GBK / UTF-16 自动探测；`.doc` 旧版二进制格式一期不做）
- 输出：原文件同目录，文件名加 `.zh.` 段；同名已存在时弹托盘通知询问下一步
- 反馈：托盘气泡通知（开始 / 完成 / 失败摘要）
- 复用：完全复用现有 `ai_translate::Engine` 与 `Scene::FullPage`，翻译核心一行不动

**不做**：翻译历史、批处理进度条、PDF / EPUB / HTML 文件、Excel / PowerPoint、拖拽上传、命令行入口、`.doc` 旧版。

## 架构

新增一个模块 `apps/desktop/src/file_translate.rs`，对外一个公开函数：

```rust
pub fn translate_paths(
    paths: Vec<PathBuf>,
    engine: &Engine,
    notify: impl FnMut(FileEvent),
) -> anyhow::Result<FileSummary>
```

`FileEvent` 是枚举（`Started { path }` / `Progress { path, done, total }` / `Done { path, output }` / `Failed { path, error }`），由调用方决定怎么渲染（一期统一映射为托盘气泡）。

模块内按扩展名分派：
- `.txt` → `parse_txt`（纯函数）
- `.md` → `parse_md`（纯函数）
- `.docx` → `parse_docx` + `write_docx`（纯函数，I/O 仅在 zip）

三个 parser 都产出统一中间表示 `Vec<TextBlock>`：

```rust
pub struct TextBlock {
    pub kind: BlockKind,    // Paragraph | Heading(u8) | ListItem
    pub text: String,       // 原文；多行合并为 \n
}

pub struct ParsedDoc {
    pub blocks: Vec<TextBlock>,
    pub source_meta: SourceMeta,  // 编码、原文 hash、格式特征
}
```

`write_*` 系列按 `Vec<TextBlock>`（**带译文**）写回，docx 用 `Vec<DocxSegment>`（段落级 ID 对齐）。

## 各格式处理细则

### .txt
- 按非空行分段，空行作段间分隔
- 编码：`encoding_rs::Encoding::for_bom` 优先；无 BOM 则用 `chardetng` 探测（依赖 crate）
- 回写：原编码不变；译文编码与原文件一致

### .markdown
- 用 `pulldown-cmark` 解析事件流（依赖 crate，~200KB）
- 跳过事件：`CodeBlock` / `InlineCode` / `Html` / `Url`（自动从 `Link` 中剥）/ 前置 front matter（解析为 `FrontMatter` 后整体跳过）
- 翻译单元：`Heading(level, text)` / `Paragraph(text)` / `ListItem(text)`（列表项缩进保留）
- 译文回写：按 markdown 语法重新组装（heading 加 `#`，listitem 加 `- `，不破坏原结构）；不重新排版

### .docx（脚本来）
- 用 `zip` + `quick-xml`（两个依赖 crate，都很小）
- 解压 `word/document.xml`，对每个 `<w:p>`：
  - 收集所有 `<w:r>/<w:t>` 文本片段，按出现顺序合并为段落字符串
  - 合并后送翻译（替换为 `TranslationMode::Paragraph`）
  - 译文按原文本节点数**尽量均分**，多退少补（最后一个文本节点承接余数）
- 表格、图片、页眉页脚、样式、超链接一律不动
- 回写：用 `zip` 重新打包；除 `word/document.xml` 外全部原样写入

## 输出文件命名

`foo.md` → `foo.zh.md`
`foo.docx` → `foo.zh.docx`
`foo.zh.md` 已存在：
- 弹气泡「`foo.zh.md` 已存在，是否覆盖？」点击动作：覆盖 / 加序号（`foo.zh (1).md`）/ 跳过
- 一期：托盘菜单点击动作不好做，改为**自动加序号**（`foo.zh (1).md` ... `foo.zh (99).md`），气泡报一句「已自动避开同名文件」

## 进度反馈

托盘气泡（Windows `Shell_NotifyIcon` + `NIF_INFO`）：
- 开始：`正在翻译 N 个文件…`
- 完成：`N 个文件翻译完成`（含成功/失败计数）
- 失败：`K 个文件翻译失败`，详情放标题

实现位置：`desktop_shell::tray` 加 `pub fn show_balloon(title: &str, body: &str, kind: BalloonKind)`。现有 tray.rs 自己拼 `NotifyIcon` 结构，加一个 info 字段即可。

## 配置

设置窗「翻译服务」页加一栏（在「场景预设」下）：
- **每批段落数**：默认 20（与现有 `Scene::FullPage` 批次策略一致）

不复用「并发上限」「超时」等已有开关——文件翻译场景下沿用 Engine 默认。

## 依赖

新增（cargo.toml `apps/desktop`）：
```toml
zip = "0.6"
quick-xml = "0.36"
encoding_rs = "0.8"
chardetng = "0.1"
pulldown-cmark = { version = "0.12", default-features = false }
```

体积影响：合计约 +1.5 MB。装机大小从 9MB 涨到约 10.5MB。

## 测试

### 单元测试（`file_translate.rs` 同文件 `#[cfg(test)]`）

**纯函数**
- `parse_txt`: 空文件 / 只有空行 / 含 \r\n / GBK 编码
- `parse_md`: 含代码块（围栏+行内）/ 含链接 / 含 front matter / 含列表 / 含 heading
- `parse_docx`: 单段 / 多段 / 含表格 / 含图片 / 含页眉页脚 / 段落带 `<w:br/>` 软换行
- `write_docx`: 译文长度差 ±20% 内仍能均分
- 输出文件命名：常规 / 同名已存在加序号 / 加到 (99) 之后

**集成**（用现有 `tests/mock-llm`）
- 三个 fixture 文件 → 跑 `translate_paths` → 验回写文件可被打开 / 段落位置对齐 / 原文未动

### 端到端

不在本期范围（一期先在主流程跑 fixture + UI 冒烟，托盘入口需要手动测）。

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `apps/desktop/src/file_translate.rs` | **新增**：parse / write / 编排 |
| `apps/desktop/src/lib.rs` | 加 `pub mod file_translate;` |
| `apps/desktop/src/main.rs` | 拉起 translate 后台线程时挂上 file_translate 调度 |
| `apps/desktop/src/tray.rs` | 加菜单项「翻译文件…」+ `show_balloon` 函数 |
| `apps/desktop/src/ui.rs` | 设置窗「翻译服务」页加「每批段落数」 |
| `apps/desktop/src/config.rs` | `UiConfig` 加 `file_batch_size: usize`（默认 20） |
| `tests/fixtures/file_translate/` | **新增**：sample.docx / sample.md / sample.txt |
| `Cargo.toml` (`apps/desktop`) | 加 5 个依赖 |
| `installer/简译Glance.iss` | 不动（依赖未装机） |

## 后续（二期及以后，未做）

- 拖拽：主窗口接受文件拖拽
- PDF / EPUB：先 OCR → 再走文件翻译流程
- `.doc` 旧版：调 LibreOffice headless 转 .docx
- 翻译历史（设置窗加一页，最近翻过的文件列表）
- 多文件并发调度（现在是一文件一任务，简单够用）

---

## Self-Review

- ✅ 没有 TBD / TODO 占位
- ✅ 范围限定明确（一期三种格式，复用 Engine）
- ✅ 命名冲突处理给出明确规则（自动加序号，无歧义）
- ✅ docx 段落对齐策略写明（均分 + 最后一个吃余数）
- ⚠ 「托盘菜单点击动作确认覆盖」一期不做，自动加序号代替——已写明原因