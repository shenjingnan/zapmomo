# 聊天气泡手动关闭（去掉自动消失）设计方案

- 日期：2026-08-28
- 范围：`src-tauri/frontend`（bubble 窗口）
- 状态：已评审通过，进入实施

## 1. 背景与需求

聊天气泡（bubble 窗口的 `VoiceReplyBubble`）当前在内容展示后按定时自动消失：
回复完结定格 5s 后淡出、打断/停止立即清除、dsh 插播 5s 定时消失。用户希望
**有内容时不自动消失，由用户手动点击关闭**，避免想看的内容被程序收走。

## 2. 现状分析

气泡唯一渲染路径：`bubble.tsx → BubbleRoot → VoiceReplyBubble`。
两条内容通道共用同一气泡：

- `text`：语音/文字对话的 LLM 流式回复（token 累积 = 打字机）；
- `announcement`：dsh 事件播报台词（被流式压制时暂存，新鲜期内补展示）。

现有 3 条自动消失路径（均在 `VoiceReplyBubble`）：

| 路径 | 触发 | 行为 |
| --- | --- | --- |
| 正常完结 | `reply-finished`（text 清空） | 定格 `HOLD_MS`=5s → 淡出 `FADE_MS`=0.5s → 移除 |
| 打断/停止 | phase 回 `armed`/`idle` | 立即 `clearDisplay()` |
| 插播台词 | 自身定时 | 5s 定格 → 淡出（另含被压制 5s 新鲜期丢弃） |

核心交互冲突：气泡面整个是拖动把手（`onMouseDown → startDragging()`）。
`startDragging` 是 OS 级窗口拖动，进入系统事件循环后 **后续 click 事件到不了
WebView**，因此不能简单叠加 `onClick`，必须把拖动判定延迟到位移超阈值之后。

## 3. 决策记录（已与需求方确认）

1. **打断/停止一并保留**：半截回复不清除，统一「出现过的内容归用户管、点击才消失」。
2. **dsh 插播一致改手动**：同一气泡同一套规则，插播本就是 galgame 台词性质。
3. **交互形态**：点击气泡任意处关闭；与拖动用 5px 位移阈值区分（点 = 关，按住拖 = 移）。

## 4. 方案设计

### 4.1 行为语义

核心原则：内容一旦出现就归用户管，点击才消失；唯一例外是新一轮内容自然顶替。

| 场景 | 现状 | 改造后 |
| --- | --- | --- |
| 回复正常完结 | 定格 5s → 淡出 | 静置不消失，等点击 |
| 回复被打断（phase 回 armed） | 立即清除 | 保留半截内容，等点击 |
| 停止会话 | 立即清除 | 保留内容，等点击 |
| dsh 插播台词 | 5s 定时消失 | 静置不消失，等点击 |
| 新一轮流式回复开始 | 顶掉旧内容 | 不变（顶掉） |
| 新插播到达 | 替换旧插播 | 不变（替换） |
| 插播被压制超新鲜期丢弃 | 5s 新鲜期 | 不变（管「过期不补展示」，与消失时长无关） |
| 用户点击气泡 | 无（拖动把手） | 关闭气泡（见 4.2） |

边界：**流式输出进行中（`text` 非空）点击不响应**——内容未定稿，点击会被下一个
token 顶回来。仅内容定稿后（text 已清空：完结/打断后的静置态；插播天然定稿）响应关闭。

连带简化：删除 `HOLD_MS`/`FADE_MS` 定时器、`fading` state、`holdingRef`/`fadingRef`、
「定格期内 phase 重入保护」等防御性代码；`phase` prop 不再驱动任何展示变化，从
props 中移除。组件预计 171 行 → 约 110 行。

### 4.2 交互实现（Pointer Events + 位移阈值）

```
onPointerDown (左键)
  → el.setPointerCapture(pointerId)   // 按住移出气泡面仍能收事件
  → 记录起点，moved = false（不启动拖动）

onPointerMove (按住中)
  → 位移 hypot(dx, dy) > 5px 且未标记 moved
  → moved = true，此时才 startDragging()

onPointerUp
  → !moved 且内容定稿（text === ""）→ 关闭气泡
```

- `setPointerCapture` 必需：拖动时鼠标移出气泡面，无 capture 则后续事件丢失；
  现有 `touch-none` class 恰为 Pointer Events 前置条件。
- `startDragging()` 调用后 OS 接管、后续事件收不到——`moved` 已置位，无副作用。
- 右键（`button !== 0`）不响应，与现状一致。
- `cursor-grab` 保留，追加 `title="点击关闭 · 按住拖动"` 可发现性提示。
- 关闭即内部 `setVisibleText("")`，`onVisibleChange(false)` 链路复用，`BubbleRoot`
  的点穿切换逻辑不变（仅去掉 `phase` 传参一行）；`useVoiceSession` 零改动。

### 4.3 测试计划

`VoiceReplyBubble.test.tsx` 现有 17 用例约半数钉住定时消失行为：

- 删除：定格/淡出时序断言、「打断立即消失」、「定格期内 phase 重入保护」、
  插播定时消失等用例。
- 新增：完结后静置不消失、打断保留半截、点击关闭并上报不可见、流式中点击不响应、
  位移 >5px 触发 `startDragging` 且不关闭、位移 <5px 关闭且不触发 `startDragging`、
  右键不响应、新一轮文本顶掉静置内容。
- jsdom 无 `setPointerCapture`，测试需 stub；`ANNOUNCEMENT_FRESH_MS` 保留，继续用 fake timers。

## 5. 实施与验收

单阶段实施（改动收敛在 `VoiceReplyBubble.tsx` + 测试 + `BubbleRoot.tsx` 一行）：

1. 先写/改测试（TDD 红）；
2. 重写 `VoiceReplyBubble` 展示生命周期为「静置 + 手动关闭」；
3. `BubbleRoot` 去掉 `phase` 传参；
4. 验收命令：`pnpm tsc -b && pnpm vitest run src/components/bubble`
   （注意：必须 `tsc -b`，根目录裸 `tsc --noEmit` 会空通过）；
5. 实跑 `pnpm tauri dev` 手动验证：点击关闭、按住拖动、打断保留、新一轮顶替。
