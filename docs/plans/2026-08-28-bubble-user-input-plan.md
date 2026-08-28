# 聊天气泡展示用户输入 实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 气泡从「单一回复视图」升级为「一轮对话视图」——用户句先亮、回复在下方流式追加。

**Architecture:** 纯前端改动。`useVoiceSession` 在既有 `voice-session-transcript` 订阅点上
新增 `turnUserText` state；`VoiceReplyBubble` 新增 `userText` prop 渲染两段布局；插播压制
条件从「流式回复存在」扩展为「用户句待回复或流式回复存在」。后端零改动。

**Tech Stack:** React 19 + TypeScript + vitest 4 + @testing-library/react（jsdom）。

**设计文档:** `docs/plans/2026-08-28-bubble-user-input-design.md`

**工作目录:** 所有命令在 `src-tauri/frontend/` 下执行（worktree 已 `pnpm install`）。

---

### Task 1: `useVoiceSession` 新增 `turnUserText`

**Files:**
- Modify: `src-tauri/frontend/src/hooks/useVoiceSession.ts`
- Test: `src-tauri/frontend/src/hooks/useVoiceSession.test.tsx`

**Step 1: 写失败测试**

测试文件两处改动：

a) `Probe` 组件加一个展示位（放在 `data-testid="reply"` 之后）：

```tsx
<span data-testid="turnUserText">{voice.turnUserText}</span>
```

b) `describe` 内新增用例：

```tsx
it("transcript is_final 置位 turnUserText，新一轮顶替，打断不清", () => {
  render(<Probe />);
  emit("voice-session-transcript", { text: "第一句", is_final: true });
  expect(screen.getByTestId("turnUserText").textContent).toBe("第一句");
  // partial 不置位
  emit("voice-session-transcript", { text: "说到一半", is_final: false });
  expect(screen.getByTestId("turnUserText").textContent).toBe("第一句");
  // 打断（state 回 armed）保留——静置语义：想看的内容不被程序收走
  emit("voice-session-state", { running: true, state: "armed" });
  expect(screen.getByTestId("turnUserText").textContent).toBe("第一句");
  // 新一轮顶替
  emit("voice-session-transcript", { text: "第二句", is_final: true });
  expect(screen.getByTestId("turnUserText").textContent).toBe("第二句");
});
```

**Step 2: 跑测试确认失败**

Run: `pnpm vitest run src/hooks/useVoiceSession.test.tsx`
Expected: FAIL —— `turnUserText` 类型报错（接口无此字段）。

**Step 3: 最小实现**

`useVoiceSession.ts` 四处改动：

a) `VoiceSessionState` 接口，`pendingReply` 字段后新增：

```ts
/** 当前轮用户句（transcript is_final 置位；气泡「先用户句后回复」用） */
turnUserText: string;
```

b) state 声明（`pendingReply` 之后）：

```ts
const [turnUserText, setTurnUserText] = useState("");
```

c) `onVoiceSessionTranscript` 的 `is_final` 分支，`setPartial("")` 后新增：

```ts
setTurnUserText(p.text);
```

（`is_final` 为 false 的分支不动——partial 只进 `partial`。）

d) 返回对象加 `turnUserText`；hook 顶部文档注释「记录流」段落补一句：
`transcript is_final 同时置位 turnUserText 供气泡展示当前轮用户句`。

**Step 4: 跑测试确认通过**

Run: `pnpm vitest run src/hooks/useVoiceSession.test.tsx`
Expected: PASS（新旧全部用例）。

**Step 5: Commit**

```bash
git add src-tauri/frontend/src/hooks/useVoiceSession.ts src-tauri/frontend/src/hooks/useVoiceSession.test.tsx
git commit -m "feat(bubble): useVoiceSession 暴露 turnUserText 当前轮用户句"
```

---

### Task 2: `VoiceReplyBubble` 新增 `userText` prop（核心改动）

**Files:**
- Modify: `src-tauri/frontend/src/components/bubble/VoiceReplyBubble.tsx`
- Test: `src-tauri/frontend/src/components/bubble/VoiceReplyBubble.test.tsx`

**Step 1: 写失败测试**

测试文件新增一个 `describe` 块（现有用例一律不动）：

```tsx
// ---- 用户句通道（userText，一轮对话视图：先用户句、后回复）----

it("用户句先亮：仅 userText 时展示「我：」前缀句", () => {
  render(<VoiceReplyBubble text="" userText="你好呀" />);
  expect(screen.getByText("我：你好呀")).toBeTruthy();
});

it("回复流式追加在用户句下方，两者同屏", () => {
  const { rerender } = render(<VoiceReplyBubble text="" userText="你好呀" />);
  rerender(<VoiceReplyBubble text="你好，很高兴" userText="你好呀" />);
  expect(screen.getByText("我：你好呀")).toBeTruthy();
  expect(screen.getByText("你好，很高兴")).toBeTruthy();
});

it("新一轮 userText 顶掉旧轮内容（静置的旧回复一并清场）", () => {
  const { rerender } = render(<VoiceReplyBubble text="旧回复" userText="旧问题" />);
  rerender(<VoiceReplyBubble text="" userText="旧问题" />);
  rerender(<VoiceReplyBubble text="" userText="新问题" />);
  expect(screen.getByText("我：新问题")).toBeTruthy();
  expect(screen.queryByText("旧回复")).toBeNull();
  expect(screen.queryByText("我：旧问题")).toBeNull();
});

it("userText 与首 token 同批到达时直接显示流式回复（不清场竞态）", () => {
  render(<VoiceReplyBubble text="你" userText="你好" />);
  expect(screen.getByText("我：你好")).toBeTruthy();
  expect(screen.getByText("你")).toBeTruthy();
});

it("userText 等回复期间插播被压制，回复完结后补展示且清用户句", () => {
  const { rerender } = render(<VoiceReplyBubble text="" announcement="插播台词" />);
  // 用户句先到，回复未始：插播暂存不抢屏
  rerender(<VoiceReplyBubble text="" userText="问题" announcement="插播台词" />);
  expect(screen.getByText("我：问题")).toBeTruthy();
  expect(screen.queryByText("插播台词")).toBeNull();
  // 回复完结（text 清空）：新鲜期内插播补展示，插播是独立发言不带用户句
  rerender(<VoiceReplyBubble text="" userText="问题" announcement="插播台词" />);
  act(() => {
    vi.advanceTimersByTime(0);
  });
  rerender(<VoiceReplyBubble text="" userText="问题" announcement="插播台词" />);
  expect(screen.getByText("插播台词")).toBeTruthy();
  expect(screen.queryByText("我：问题")).toBeNull();
});

it("userText 静置（无回复）期间到达的新鲜插播正常补展示", () => {
  const { rerender } = render(<VoiceReplyBubble text="" userText="问题" />);
  act(() => {
    vi.advanceTimersByTime(1000);
  });
  // 回复始终未开始（等过 awaiting 窗口后 text 仍空）——用新一轮插播直接验证顶替
  rerender(<VoiceReplyBubble text="" userText="问题" announcement="插播台词" />);
  expect(screen.getByText("插播台词")).toBeTruthy();
});

it("点击关闭一次性清空用户句与回复", () => {
  const { rerender } = render(<VoiceReplyBubble text="完整回复" userText="问题" />);
  rerender(<VoiceReplyBubble text="" userText="问题" />);
  const el = screen.getByText("完整回复");
  press(el);
  release(el);
  expect(screen.queryByText("完整回复")).toBeNull();
  expect(screen.queryByText("我：问题")).toBeNull();
});

it("仅用户句时点击同样关闭", () => {
  render(<VoiceReplyBubble text="" userText="问题" />);
  const el = screen.getByText("我：问题");
  press(el);
  release(el);
  expect(screen.queryByText("我：问题")).toBeNull();
});

it("仅用户句（回复未始）时可见性上报为可见，关闭后不可见", () => {
  const onVisibleChange = vi.fn();
  const { rerender } = render(
    <VoiceReplyBubble text="" userText="问题" onVisibleChange={onVisibleChange} />,
  );
  expect(onVisibleChange).toHaveBeenLastCalledWith(true);
  const el = screen.getByText("我：问题");
  press(el);
  release(el);
  expect(onVisibleChange).toHaveBeenLastCalledWith(false);
});

it("dismiss 后同 props 重渲染不复活用户句", () => {
  const { rerender } = render(<VoiceReplyBubble text="" userText="问题" />);
  const el = screen.getByText("我：问题");
  press(el);
  release(el);
  rerender(<VoiceReplyBubble text="" userText="问题" />);
  expect(screen.queryByText("我：问题")).toBeNull();
});
```

注意「等回复期间插播被压制，回复完结后补展示」用例：插播压制解除依赖「text 曾非空后
变空」，rerender 相同 props 不触发 effect，所以该用例里插播补展示需要 text 经历
非空→空。修正为（替换上面第 5 个用例）：

```tsx
it("userText 等回复期间插播被压制，回复完结后补展示且清用户句", () => {
  const { rerender } = render(<VoiceReplyBubble text="" userText="问题" />);
  expect(screen.queryByText("插播台词")).toBeNull();
  // 回复流式开始（压制持续）……
  rerender(<VoiceReplyBubble text="回复中" userText="问题" announcement="插播台词" />);
  expect(screen.getByText("回复中")).toBeTruthy();
  expect(screen.queryByText("插播台词")).toBeNull();
  // ……完结变空：新鲜期内补展示，插播是独立发言不带用户句
  rerender(<VoiceReplyBubble text="" userText="问题" announcement="插播台词" />);
  expect(screen.getByText("插播台词")).toBeTruthy();
  expect(screen.queryByText("我：问题")).toBeNull();
});
```

**Step 2: 跑测试确认失败**

Run: `pnpm vitest run src/components/bubble/VoiceReplyBubble.test.tsx`
Expected: FAIL —— `userText` prop 不存在（TS 报错/用例失败）。

**Step 3: 实现**

`VoiceReplyBubble.tsx` 改动：

a) props 新增 `userText`（`text` 之前），组件 doc 注释同步：

```tsx
export function VoiceReplyBubble({
  userText,
  text,
  announcement = "",
  onVisibleChange,
}: {
  /** 当前轮用户句（新一轮到达顶掉旧轮内容；空串表示无） */
  userText?: string;
  text: string;
  /** dsh 事件播报台词（空串表示无插播） */
  announcement?: string;
  onVisibleChange?: (visible: boolean) => void;
}) {
```

b) 展示状态：`visibleText` 旁新增 `visibleUser`；删除无读取的死 ref
`visibleTextRef`；新增 `lastUserTextRef` 与 `awaitingReplyRef`：

```tsx
const [visibleUser, setVisibleUser] = useState("");
const [visibleText, setVisibleText] = useState("");
const sourceRef = useRef<ContentSource | null>(null);
// 插播暂存与新鲜期判定（现有，不变）
const pendingAnnouncementRef = useRef<{ text: string; at: number } | null>(null);
const lastAnnouncementRef = useRef("");
const freshTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
// 用户新到判定 + 「等回复」压制窗口（userText 落屏到本轮回复开始前）
const lastUserTextRef = useRef("");
const awaitingReplyRef = useRef(false);
// 按住状态（不变）
const pressRef = useRef<{ x: number; y: number; moved: boolean } | null>(null);
```

`ContentSource` 类型：`"reply"` 改为 `"turn"`（用户句+回复同一来源）。

c) 主 effect 重写（`[text, announcement]` → `[text, announcement, userText]`）：

```tsx
useEffect(() => {
  // 新插播登记（同一条不重复处理）；暂存后视流式状态决定立即展示或等待补展示
  if (announcement !== lastAnnouncementRef.current) {
    lastAnnouncementRef.current = announcement;
    if (announcement) {
      pendingAnnouncementRef.current = { text: announcement, at: Date.now() };
      clearTimeout(freshTimerRef.current);
      freshTimerRef.current = setTimeout(() => {
        pendingAnnouncementRef.current = null;
      }, ANNOUNCEMENT_FRESH_MS);
    }
  }

  // 新用户句登记：开启新一轮，顶掉旧轮全部内容（含静置旧回复与展示中插播）
  if (userText !== lastUserTextRef.current) {
    lastUserTextRef.current = userText;
    if (userText) {
      awaitingReplyRef.current = true;
      sourceRef.current = "turn";
      setVisibleUser(userText);
      // 同批已有首 token 则不清场（直接被下方 text 分支覆盖为新回复）
      if (!text) setVisibleText("");
    }
  }

  if (text) {
    // 流式更新中：跟随最新文本，用户句保留在上方
    awaitingReplyRef.current = false;
    sourceRef.current = "turn";
    setVisibleText(text);
    return;
  }

  // 用户句在屏、回复未始：压制插播（用户主动对话优先，暂存等补展示）
  if (awaitingReplyRef.current && visibleUser !== "") {
    return;
  }

  // 无流式文本 → 有新鲜插播则展示（插播是独立发言，清掉用户句）
  const pending = pendingAnnouncementRef.current;
  if (pending && Date.now() - pending.at <= ANNOUNCEMENT_FRESH_MS) {
    clearTimeout(freshTimerRef.current);
    pendingAnnouncementRef.current = null;
    awaitingReplyRef.current = false;
    sourceRef.current = "announcement";
    setVisibleUser("");
    setVisibleText(pending.text);
  }

  // text 清空（正常完结 / 打断 / 停止）：内容静置保留，等用户点击关闭。
  // 同 props 重渲染不复活——展示仅由新内容或点击关闭驱动。
}, [text, announcement, userText, visibleUser]);
```

d) `dismiss` 清用户句：

```tsx
const dismiss = () => {
  awaitingReplyRef.current = false;
  sourceRef.current = null;
  setVisibleUser("");
  setVisibleText("");
};
```

e) 可见性上报与空渲染判定：

```tsx
useEffect(() => {
  onVisibleChange?.(visibleText !== "" || visibleUser !== "");
}, [visibleText, visibleUser, onVisibleChange]);

if (!visibleText && !visibleUser) return null;
```

f) 「流式进行中点击不响应」条件不变（`if (text) return`）。

g) 渲染改两段（容器 `max-h-32` → `max-h-44`，用户句弱化前置）：

```tsx
<div className="max-h-44 w-full overflow-hidden rounded-xl border border-border bg-popover px-4 py-2.5 text-sm text-text-primary shadow-lg">
  {visibleUser && (
    <p className="line-clamp-2 whitespace-pre-wrap break-words text-xs text-muted-foreground">
      我：{visibleUser}
    </p>
  )}
  {visibleText && (
    <p className="line-clamp-4 whitespace-pre-wrap break-words">{visibleText}</p>
  )}
</div>
```

两段并存时加间距：外层容器再加 `space-y-1`。

h) 组件 doc 注释更新：内容三路（userText / text / announcement），优先级与压制语义
（用户句等回复期间压制插播；插播补展示为独立发言）。

**Step 4: 跑测试确认通过**

Run: `pnpm vitest run src/components/bubble/VoiceReplyBubble.test.tsx`
Expected: PASS（新旧全部用例，旧用例零改动全绿）。

**Step 5: Commit**

```bash
git add src-tauri/frontend/src/components/bubble/VoiceReplyBubble.tsx src-tauri/frontend/src/components/bubble/VoiceReplyBubble.test.tsx
git commit -m "feat(bubble): 气泡展示当前轮用户句，回复在其下方流式追加"
```

---

### Task 3: `BubbleRoot` 接线 + 全量验证

**Files:**
- Modify: `src-tauri/frontend/src/components/bubble/BubbleRoot.tsx`

**Step 1: 透传 prop**

`VoiceReplyBubble` 调用处新增 `userText={voice.turnUserText}`：

```tsx
<VoiceReplyBubble
  text={voice.pendingReply}
  userText={voice.turnUserText}
  announcement={announcement}
  onVisibleChange={setBubbleVisible}
/>
```

组件 doc 注释「有且只有一个聊天气泡」段落补：气泡呈现当前轮用户句与流式回复。

**Step 2: 全量前端验证**

```bash
pnpm vitest run          # 全部测试通过
pnpm tsc -b              # 类型检查通过（根目录 tsc --noEmit 空通过，须用 -b）
pnpm check               # biome 检查（格式/导入序）
```

Expected: 三项全绿。

**Step 3: Commit**

```bash
git add src-tauri/frontend/src/components/bubble/BubbleRoot.tsx
git commit -m "feat(bubble): BubbleRoot 接通 turnUserText，气泡呈现一轮对话"
```

**Step 4: 手动验收（`pnpm tauri dev`，可选提交前）**

1. 输入条发消息 → 气泡立即显示「我：」+ 用户句；回复下方流式追加
2. 语音对话 → ASR 最终文本同样先上气泡
3. 点击气泡 → 一次性关闭；拖动正常；空闲点穿无回归
4. dsh 插播不在用户句/回复期间抢屏

---

## 风险与回退

- 改动集中在 3 个文件，旧用例零改动全绿是回归底线；任何旧用例变红说明语义被破坏，
  修实现而非改用例。
- 插播压制新增 `awaitingReplyRef` 窗口：若手测发现插播长期不补展示，优先检查该 ref
  是否在 text 非空分支正确复位。
