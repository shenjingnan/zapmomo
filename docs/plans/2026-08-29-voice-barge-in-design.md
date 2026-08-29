# 设计：TTS 播报中的语音打断（ASR Barge-in）

- 日期：2026-08-29
- 状态：已评审定稿
- 关联：`src/voice/session.rs`（编排核心）、`src/voice/state.rs`（状态机）

## 1. 需求

角色播报（TTS）期间，用户直接开口说话（无需喊唤醒词），ASR 识别到有意义的人声内容
即**立刻打断播报**，用户说完后直接进入新一轮对话（LLM → TTS）。

已确认的决策边界：

1. **仅流式 ASR 后端（zipformer/paraformer）支持**：流式才有实时 partial 可做「有意
   义内容」判定；离线族（SenseVoice/Whisper/Qwen3-ASR/audiocpp）无 partial，继续只用
   唤醒词打断。
2. **外放（扬声器）为主要场景**：回声过滤必须默认开启且偏保守——宁可偶尔打断不灵，
   不能自我打断。
3. **打断后不 reset ASR 流**：保住用户话头（「停」「别说了」这类短 query 完整保留），
   回声漏网靠后置文本比对兜底。
4. **被打断轮保留已播部分**：已播出的句子作为 assistant 消息入历史，LLM 下一轮知道
   自己说过什么。

## 2. 现状分析

打断基建已完备，现有三个触发源最终都汇入 `do_barge_in()`（session.rs）：

| 触发源 | 位置 | 行为 |
| --- | --- | --- |
| KWS 唤醒词 | `listen_barge_in()` Speaking/Thinking 期间喂 KWS | 置位 `barge_in` 标志 |
| 全局快捷键 | Tauri `barge_in_flag()`（src-tauri/lib.rs） | 置位同上 |
| 文字输入 | `poll_text_input()` Greeting/Thinking/Speaking | 直接调用 |

两个限制构成本设计的全部改动动机：

1. **Speaking/Thinking 期间麦克风只喂 KWS、不喂 ASR**（防回声的有意设计）：随意说话
   无法打断，必须喊唤醒词。
2. **状态机 `BargeIn → Armed`**：打断后回待唤醒，需再喊唤醒词；没有「打断后接话」
   路径。

下游链路（`handle_user_final` → `start_reply` → LLM → TTS → `finish_reply`）全部现成，
打断进 Listening 后零改动复用。

## 3. 架构分析

- **线程模型允许**：编排循环单线程轮询（`MIC_POLL` 100ms），麦克风整个会话只开一次，
  chunk 按状态分发。Speaking 期间把同一 chunk 同时喂 KWS + ASR 只是多一路纯 CPU 解码
  （zipformer 流式 RTF 低，每 chunk 约 10–30ms）；TTS 合成（`SynthHandle` 线程）与
  LLM（worker 线程）互不争抢。
- **引擎无冲突**：KWS 与 ASR 是独立引擎实例；设备层面单一 `MicLoop` 已解决冲突。
- **回声是唯一核心难点**：外放时角色自己的声音被麦克风拾回，喂 ASR 会把播报内容识别
  成「用户说话」→ 自我打断。唤醒词打断天然免疫（TTS 文本一般不含唤醒词），ASR 全文
  识别则必中，必须过滤。

### 回声应对方案对比

| 方案 | 原理 | 评估 |
| --- | --- | --- |
| **A. 流式 ASR 全程监听 + 播报文本比对**（选用） | partial 与正在播的句子做相似度比对，高相似判回声忽略 | 无新依赖；耳机/外放通吃；延迟约 200–500ms |
| B. RMS 能量抢断 | 音量连续超阈值即停播 | 外放必误伤（角色声音就超阈值），仅耳机可靠 |
| C. 真 AEC（webrtc-audio-processing） | TTS 播放样本作远端参考消除回声 | 教科书正解但引入 C++ 依赖 + rodio tap + 时钟对齐，本期性价比低，列为后续演进 |

## 4. 方案设计

### 4.1 状态机（state.rs）

新增一条迁移，与现有 `BargeIn → Armed`（快捷键/文字，行为不变）并存：

```text
Thinking | Speaking --VoiceBargeIn--> Listening   （新：打断后直接接话）
Thinking | Speaking --BargeIn------> Armed        （保留：唤醒词/快捷键回待唤醒）
```

KWS 唤醒词打断保持回 Armed 不动——语义不同（重新开始 vs 接话），最小变更。

### 4.2 判定链（session.rs `listen_barge_in` 扩展）

Speaking/Thinking 期间，chunk 从「只喂 KWS」变为「喂 KWS + 喂 ASR + decode 取
partial」（仅当 `cfg.asr` 为流式后端且开关开启；否则行为与现状完全一致）。新增纯逻辑
判定器 `VoiceBargeInDetector`，以下条件全部满足才触发：

1. chunk RMS 超过现有 `vad_silence_threshold`；
2. ASR partial 有效（去标点后 ≥2 个汉字）；
3. partial 与回声参考文本的字符 bigram Dice 相似度 < `barge_in_similarity_threshold`；
4. 连续 ≥2 个 chunk 满足（防瞬时噪音，模式同 WaitingSpeech 的 `speech_hits`）。

触发 → 置位语音打断标志 → 编排循环执行 `do_barge_in_voice()`。

### 4.3 双层回声过滤（外放为主的关键）

- **前置（判定时）**：回声参考窗口 = 当前正在播的句子 + 最近播完的 1 句（回声滞后，
  partial 常对应上一句）。`step_speaking` 弹句发 `PlaySentence` 时维护
  `current_speech` / `recent_speech` 两个新字段。
- **后置（兜底）**：打断后 **ASR 流不 reset**（保用户话头），说完走现有
  `handle_user_final`；若 finalize 文本与回声参考仍高度相似 → 判回声漏网，丢弃并保持
  聆听（现有「空识别 → 保持聆听」路径复用，零新逻辑）。

失败方向安全：外放混响导致前置漏拦时，最坏是「打断不灵」（后置兜底拦住）或「多听一
句」，不会把角色台词当作用户输入自我打断。

### 4.4 打断序列与延迟

`do_barge_in_voice()` 在现有 `do_barge_in()` 基础上差异：ASR 不 reset、不依赖
`skip_for` 丢话头（仍保留 300ms skip 供回声尾巴衰减）、迁移到 Listening 而非 Armed。

延迟预期：RMS 命中（~64ms）→ 流式 partial 出字（~100–300ms）→ 连续确认（+1 chunk）
→ `speaker.stop()` 即停。**总延迟约 200–500ms**，与主流 Realtime 产品 barge-in 同量
级；外放保守过滤下偏上限。

### 4.5 已播部分入历史

新增 `spoken_text: String` 字段：`step_speaking` 每次弹句发 `PlaySentence` 时把原句
（含标点）追加。语音打断时：

- 非空 → 作为 assistant 消息 push 入历史（跟在被打断轮的 user 消息后）；
- Thinking 阶段打断（未播出任何句）→ 为空，只留 user 消息，不特殊处理；
- `start_reply()` 与非语音打断路径照常清空。

### 4.6 配置面与事件

| 配置 | 默认 | 说明 |
| --- | --- | --- |
| `voice_barge_in: bool` | `true` | 总开关；非流式 ASR 后端自动忽略 |
| `barge_in_similarity_threshold: f32` | `0.5` | 字符 bigram Dice 阈值，外放保守值 |

settings.toml `[voice]` 段落新增两键；CLI 对齐现有风格加 `--no-voice-barge-in`。
`VoiceEvent::BargeIn` 区分来源（唤醒词 / 语音 / 快捷键 / 文字），文案区分
（语音打断提示「检测到你的声音，请继续说」）。

## 5. 实施方案

分两期，各自可独立验收：

### 阶段 1（根 crate，核心链路）

任务：

1. 新模块 `src/voice/bargein.rs`：相似度函数 + `VoiceBargeInDetector`（纯逻辑，可单测）；
2. `state.rs` 新增 `VoiceBargeIn` 事件与迁移 + 测试；
3. `session.rs`：`listen_barge_in` 扩展喂 ASR、回声参考字段、`spoken_text` 累积、
   `do_barge_in_voice()`、后置过滤接入 `handle_user_final`；
4. `config.rs` / settings / CLI 覆盖项；
5. `events.rs` 打断来源区分。

验收：`cargo fmt --check && cargo clippy -- -D warnings && cargo test` 全绿；CLI
`cargo run -- voice run` 外放真机验收：

- 播报中说新内容 → 播报停止，说完的内容进入新一轮对话；
- 播报中跟读台词 → 不误触发；
- 打断后短 query（「停」）完整保留，不丢话头；
- 离线 ASR 后端下行为与现状一致（无回归）。

### 阶段 2（Tauri，配置 UI 与前端提示）

任务：设置页开关与阈值、打断来源事件文案、前端打断状态提示。

验收：`pnpm tauri dev` 真机（src-tauri 侧 CI 无覆盖，必须实机验证）。

## 6. 风险与边界

| 风险 | 应对 |
| --- | --- |
| 大音量外放 + 混响 → partial 混杂 | 前置阈值调保守（默认 0.5）+ 连续命中；漏拦由后置过滤兜底 |
| 用户跟读角色台词 | 被判回声忽略（极端场景，接受） |
| 阈值不适配个别环境 | `barge_in_similarity_threshold` 可配 |
| Speaking 期间 CPU 增加 | zipformer RTF 低（10–30ms/100ms chunk），与 TTS/LLM 异线程，无争抢 |
