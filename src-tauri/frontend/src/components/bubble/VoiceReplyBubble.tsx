import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/tauri";

/** 拖动判定位移阈值（CSS 像素）：超过视为拖动，未超过松开视为点击关闭。 */
const DRAG_THRESHOLD_PX = 5;
/** 插播新鲜期（毫秒）：被流式回复压制超过此时长的插播到期丢弃，完结后不再补展示。 */
const ANNOUNCEMENT_FRESH_MS = 5000;
/** 等回复耐心窗口（毫秒）：用户句落屏后此窗口内压制插播；回复始终未始（如 LLM 出错）
 * 超过窗口则插播恢复正常展示（最新发言胜出）。 */
const AWAITING_REPLY_PATIENCE_MS = 5000;

/** 当前展示内容的来源。 */
type ContentSource = "turn" | "announcement";

/**
 * 聊天气泡（独立 bubble 窗口的唯一内容视图，有且只有一个聊天气泡）。
 *
 * 内容三路共用同一气泡，构成「一轮对话视图」（先用户句、后回复）：
 * - `userText`：当前轮用户句。到达即上屏（消除首 token 前的空窗）；新一轮到达
 *   顶掉旧轮全部内容（含静置旧回复与展示中插播）。
 * - `text`：语音/文字对话的流式回复（token 累积 = 天然打字机），追加在用户句
 *   下方。text 清空（正常完结或被打断/停止）后内容静置保留，不自动消失。
 * - `announcement`：dsh（DeepSeek Harness）事件播报台词。被流式回复或「用户句
 *   等回复耐心窗口」（AWAITING_REPLY_PATIENCE_MS）压制时暂存，压制解除后新鲜期
 *   内补展示，超期丢弃；插播是独立发言，补展示时清掉用户句；展示中新插播替换
 *   旧插播（最新发言胜出）。
 *
 * 内容一旦出现即静置常驻，唯一消失途径是用户点击气泡（新一轮内容到达时
 * 自然顶替除外）——「想看的内容不被程序收走」。点击与拖动共用气泡面：按住后
 * 位移超过阈值才交给 OS 拖动窗口，未超阈值松开视为点击关闭。
 *
 * 整个气泡面按住左键拖动窗口（纯展示组件、无输入/选择交互，故文本不可选中）。
 * 可见性变化经 `onVisibleChange` 上报，窗口根组件据此切换点击穿透：
 * 有内容时可交互，无内容时透明区域穿透到下方窗口。
 */
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
  const [visibleUser, setVisibleUser] = useState("");
  const [visibleText, setVisibleText] = useState("");
  const sourceRef = useRef<ContentSource | null>(null);
  // 插播暂存与新鲜期判定（现有，不变）
  const pendingAnnouncementRef = useRef<{ text: string; at: number } | null>(null);
  const lastAnnouncementRef = useRef("");
  const freshTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // 用户新到判定 + 「等回复」压制窗口（耐心窗口内压制插播，回复开始即解除）
  const lastUserTextRef = useRef("");
  const awaitingReplyRef = useRef(false);
  const awaitingSinceRef = useRef(0);
  // 按住状态（pointer down 起点与拖动标记），ref 不触发渲染
  const pressRef = useRef<{ x: number; y: number; moved: boolean } | null>(null);

  useEffect(() => {
    // 新插播登记（同一条不重复处理）；暂存后视流式状态决定立即展示或等待补展示
    if (announcement !== lastAnnouncementRef.current) {
      lastAnnouncementRef.current = announcement;
      if (announcement) {
        pendingAnnouncementRef.current = { text: announcement, at: Date.now() };
        // 新鲜期兜底：始终被流式压制时到期即弃
        clearTimeout(freshTimerRef.current);
        freshTimerRef.current = setTimeout(() => {
          pendingAnnouncementRef.current = null;
        }, ANNOUNCEMENT_FRESH_MS);
      }
    }

    // 新用户句登记：开启新一轮，顶掉旧轮全部内容（含静置旧回复与展示中插播）
    if (userText !== lastUserTextRef.current) {
      lastUserTextRef.current = userText ?? "";
      if (userText) {
        awaitingReplyRef.current = true;
        awaitingSinceRef.current = Date.now();
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

    // 用户句在屏、回复未始：耐心窗口内压制插播（用户主动对话优先，暂存等补展示）
    if (
      awaitingReplyRef.current &&
      visibleUser !== "" &&
      Date.now() - awaitingSinceRef.current <= AWAITING_REPLY_PATIENCE_MS
    ) {
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

  // 卸载时清理定时器
  useEffect(
    () => () => {
      clearTimeout(freshTimerRef.current);
    },
    [],
  );

  // 上报可见性（窗口根组件据此切换点击穿透）
  useEffect(() => {
    onVisibleChange?.(visibleText !== "" || visibleUser !== "");
  }, [visibleText, visibleUser, onVisibleChange]);

  if (!visibleText && !visibleUser) return null;

  /** 点击关闭：清空展示（ref 与 state 同步），气泡随之消失、窗口回到点穿态。 */
  const dismiss = () => {
    awaitingReplyRef.current = false;
    sourceRef.current = null;
    setVisibleUser("");
    setVisibleText("");
  };

  return (
    // 纯展示气泡无输入/选择交互，气泡面即交互面：点击关闭，按住拖动窗口
    // （startDragging 延迟到位移超阈值，避免 OS 拖动吞掉 click 语义）。
    <div
      className="w-full cursor-grab touch-none select-none active:cursor-grabbing"
      title="点击关闭 · 按住拖动"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        // capture 保证按住移出气泡面后仍能收到 move/up（拖动必然移出）
        e.currentTarget.setPointerCapture(e.pointerId);
        pressRef.current = { x: e.clientX, y: e.clientY, moved: false };
      }}
      onPointerMove={(e) => {
        const press = pressRef.current;
        if (!press || press.moved) return;
        if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < DRAG_THRESHOLD_PX) return;
        press.moved = true;
        void api.bubbleDebugLog({ message: "气泡拖动判定命中 → startDragging" });
        getCurrentWindow()
          .startDragging()
          .catch((err) => void api.bubbleDebugLog({ message: `startDragging 失败: ${err}` }));
      }}
      onPointerUp={(e) => {
        if (e.button !== 0) return; // 只认左键释放（press 也只登记左键）
        const press = pressRef.current;
        pressRef.current = null;
        if (!press || press.moved) return;
        if (text) return; // 流式进行中内容未定稿，点击不响应（点了也会被下一 token 顶回）
        void api.bubbleDebugLog({ message: "气泡点击 → 关闭" });
        dismiss();
      }}
      onPointerCancel={() => {
        pressRef.current = null;
      }}
    >
      <div className="max-h-44 w-full space-y-1 overflow-hidden rounded-xl border border-border bg-popover px-4 py-2.5 text-sm text-text-primary shadow-lg">
        {visibleUser && (
          <p className="line-clamp-2 whitespace-pre-wrap break-words text-xs text-muted-foreground">
            我：{visibleUser}
          </p>
        )}
        {visibleText && (
          <p className="line-clamp-4 whitespace-pre-wrap break-words">{visibleText}</p>
        )}
      </div>
    </div>
  );
}
