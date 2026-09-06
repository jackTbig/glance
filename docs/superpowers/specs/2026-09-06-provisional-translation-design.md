# live-caption 设计文档 —— 临时翻译（中文不等英文整句）与带前文的翻译

日期：2026-09-06　状态：已与 Jack 逐项确认　项目根：`D:\My_data\Claude\live-caption\`　基线：1.0.1 + 提交 89eb602

## 一、问题与目标

现状：中间识别（EnglishPartial）只上屏不翻译，中文要等英文停顿定稿（`silence_ms` 400 ms）、主模型定稿（0.4 s）、翻译往返（0.6–2 s）之后才出现，一句 8 s 的话中文落后英文 3 s 以上。另外每句独立送翻译，没有前文，半句或短句常译偏。

目标（Jack 拍板）：
1. 说话过程中就翻，中文落后英文约 1.5–2 s；英文定稿后整句重翻一次定稿（**句内修订**）。
2. 每次翻译都带最近 N 句英中对照当上下文（**带前文翻译**）。
3. 定稿后不再回改上一句，记录文件只写定稿中文（不做跨句回改）。

明确不做：跨句回改、Codex 后端的临时翻译（每句 3 s 以上，没意义）。

## 二、数据流

```
[识别线程] --EnglishPartial{id,text}--> [界面]
           \--(id,text)---------------> [临时翻译线程] --ChineseProvisional{id,text}--> [界面]
           --English{id,...}----------> [界面]
           \--(id,text)---------------> [定稿翻译线程] --ChinesePartial/Chinese{id,..}--> [界面] / 记录文件
                                                     \--推入--> [共享前文队列 最近 N 句 (en, zh)] <--读取-- 两个翻译线程
```

### 2.1 话轮 id
- 识别线程在一个话轮的**第一个中间结果**时分配 id（`next_id`，随即 +1），该话轮后续中间结果与定稿共用这个 id；定稿后清掉当前 id。现在中间结果 id 恒为 0，改掉。
- 定稿识别为空（噪声）时发 `EnglishPartial { id, text: "" }` 清屏并清掉当前 id；这个 id 作废不复用，id 不要求连续。
- 没有中间结果就直接定稿的话轮（`partial_every_ms = 0`），定稿时分配 id，行为同现在。

### 2.2 临时翻译线程（新）
- 输入：`(id, text)` 的有界通道（容量 4），线程每次循环 drain 到最新一条；旧的作废。
- 发送条件（纯函数 `should_send(now, last_sent_at, last_sent_words, words)`）：
  - 距上次发出 ≥ `translate.partial_every_ms`（默认 1200）；
  - 英文词数比上次发出的多 ≥ `translate.partial_min_new_words`（默认 3）；首次要求词数 ≥ 3；
  - 同一话轮内计数；换 id 后计数清零。
- 翻译调用阻塞；回来后立即再看最新一条是否满足条件。任何时刻每话轮最多一个临时翻译在飞。
- 结果发 `UiMsg::ChineseProvisional { id, text }`（新消息类型）。流式 token 也按这个消息发累积文本。
- 后端实例：按同一份 `TranslatorConfig` 起自己的 `Engine`（Claude = 第二个常驻会话，含现有的轮次轮换逻辑；OpenAI = 独立客户端）；后端为 Codex 或 `partial_every_ms = 0` 时线程只消费不翻译。
- 失败：打 WARN，不进状态红点（临时翻译失败不影响定稿链路），下一条继续。
- 热切换：界面向两个线程各发一份配置（新增第二个配置通道）。

### 2.3 定稿翻译线程（现有，小改）
- 译完一句后把 `(en, zh)` 推进共享前文队列 `Arc<Mutex<VecDeque<(String, String)>>>`，长度上限 `translate.context_sentences`（默认 2）。
- 组提示词时带前文（见 2.4）。其余逻辑不变。

### 2.4 提示词
- 系统提示词改为：会议同传；用户消息里可能带"前文"若干句英中对照，只作参考；**只输出最后"请翻译"那句的简体中文译文**，不加解释、引号、前缀；专有名词保留原文。
- 用户消息由 `build_user_message(context, text, provisional)` 统一组装（三个后端共用）：
  ```
  前文（仅供参考，不要翻译）：
  EN: ...
  ZH: ...
  EN: ...
  ZH: ...

  请翻译：<text>
  ```
  临时翻译在"请翻译"前加一行：`下面是还没说完的半句，只译到目前为止的内容，不要补全、不要猜后文。`
- 前文为空时省略"前文"段。Claude 会话本身有历史，也照样带显式前文（短，几十 token）。

## 三、界面

- `Line` 增加 `zh_provisional: bool` 与 `zh_final_started: bool`。
- `ChineseProvisional { id, text }`：找到 id 对应行；若 `zh_final_started` 为真则忽略；否则 `zh = Some(text)`，`zh_provisional = true`。
- `ChinesePartial` / `Chinese { id }`：置 `zh_final_started = true`、`zh_provisional = false`，再按现在的逻辑处理。
- 临时中文与中间英文同款灰色斜体；定稿中文正常样式。
- 中文替换时的打字机：`zh_shown = min(zh_shown, 新旧文本公共前缀字符数)`，只有变动的尾部重新打字。定稿覆盖临时中文也用这个规则（纯函数 `common_prefix_chars(a, b)`）。
- 中间行（`partial == true`）仍不计入"显示段数"；行定稿后带着临时中文一起转正。
- 记录文件不变：只在 `Chinese` 到达时写。

## 四、配置与设置窗

`config.toml` `[translate]` 新增：
- `partial_every_ms = 1200`（0 = 关闭临时翻译）
- `partial_min_new_words = 3`（仅配置文件）
- `context_sentences = 2`

设置窗"翻译"页新增"临时翻译间隔（ms，0 关）"与"上下文句数"两项，"保存并应用"即生效（通过配置通道热更新，不需重启）。

## 五、测试

- 单测（`src/translate.rs` / `src/ui.rs` / `src/asr.rs`）：
  - `should_send`：间隔不足不发、词数不足不发、首次 3 词才发、换 id 清零。
  - `build_user_message`：无前文 / 有前文 / 临时标记三种形态。
  - `common_prefix_chars`：全同、前缀、完全不同、中文字符。
  - 话轮 id：中间结果与定稿同 id，空定稿作废 id。
- 集成（`tests/translator.rs`，`tests/fake_openai.py`）：
  - 中间英文 → 收到 `ChineseProvisional`，且请求体含"还没说完"标记；
  - 两条中间英文间隔不足只发一次；
  - 定稿后请求体含前文对照；
  - 定稿翻译到达后，晚到的临时结果被界面层忽略（界面 drain 逻辑单测）。
- 验收：Jack 机器上用 DeepSeek 跑 `scripts\e2e_check.ps1`，日志新增 `translate partial[openai] <ms>` 行；预期临时中文落后英文 1.5–2 s，定稿中文照旧。

## 六、涉及文件

`src/messages.rs`（新消息）、`src/asr.rs`（话轮 id、中间结果送翻译）、`src/translate.rs`（提示词、前文队列、`spawn_partial_translator`、`should_send`）、`src/ui.rs`（消息处理、样式、公共前缀、设置页）、`src/config.rs`（三个配置项）、`src/main.rs`（接线）、`config.example.toml`、`tests/translator.rs`、`tests/fake_openai.py`、`CLAUDE.md`。

## 七、试用后的修订（2026-09-06 晚，Jack 对齐）

1. **触发方式**：去掉 1.2 s 定时与"多 3 个词"门槛。每来一条新的中间英文、内容与上次翻过的不同就翻；同一时刻只有一个在飞，飞完取最新一条。配置项改为开关 `translate.partial_translate`（默认开）。
2. **中文续写**：临时翻译的提示词带上本话轮此前译出的临时中文，要求"已译部分与英文仍一致的原样保留，不一致的改掉；不要重复、不要另起一遍"（`CONTINUE_NOTE`）。临时翻译不再流式上屏，每次调用完成后整段发。
3. **屏上文字只增不缩**（英文与中文同规则）：
   - 中间英文：延长或更长的改写才采纳（改写从分歧点重打）；变短或等长改写不采纳；定稿整句替换但相同开头不重打（`ui::merge_partial_en`）。
   - 中文：临时译文与定稿流式增量都只在更长或等长改写时才动（`Line::set_zh_if_grown`）；定稿完成的整句允许变短。
4. **识别去掉 initial_prompt**：日志坐实提示词让 small.en 中间识别 6 s 音频只吐一个词、让 turbo 定稿吞掉开头几个词，是"英文出来又消失"的根源。两个模型裸跑。
