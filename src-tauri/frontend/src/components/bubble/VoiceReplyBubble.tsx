import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/tauri";
import type { VoiceSessionPhase } from "@/types/tauri";

/** 回复完结后气泡定格时长（毫秒），之后进入淡出。 */
const HOLD_MS = 5000;
/** 淡出过渡时长（毫秒，与 opacity 过渡 duration-500 一致），结束后移除内容。 */
const FADE_MS = 500;
/** 插播新鲜期（毫秒）：被流式回复压制超过此时长的插播到期丢弃，完结后不再补展示。 */
const ANNOUNCEMENT_FRESH_MS = 5000;

/** 当前展示内容的来源（决定 text 清空后的清场语义归属）。 */
type ContentSource = "reply" | "announcement";

/**
 * 聊天气泡（独立 bubble 窗口的唯一内容视图，有且只有一个聊天气泡）。
 *
 * 内容两路共用同一气泡，流式回复优先级恒高于插播：
 * - `text`：语音/文字对话的流式回复（token 累积 = 天然打字机）。清空分两义：
 *   正常完结（reply-finished，此刻 phase 仍在 thinking/speaking）→ 定格 HOLD_MS
 *   后淡出；打断/停止（phase 已回 armed/idle）→ 立即消失。定格期内的 phase
 *   迁移（如播完回 armed）不打断定格。
 * - `announcement`：dsh（DeepSeek Harness）事件播报台词。被流式回复压制时暂存，
 *   回复完结后新鲜期内补展示，超期丢弃；展示中新插播替换旧插播（最新发言胜出）；
 *   不随会话打断消失（与会话状态无关），按自身定时定格→淡出。
 *
 * 整个气泡面按住左键即可拖动窗口（纯展示组件、无输入/选择交互，故文本不可选中）。
 * 可见性变化经 `onVisibleChange` 上报，窗口根组件据此切换点击穿透：
 * 有内容时可拖动，无内容时透明区域穿透到下方窗口。
 */
export function VoiceReplyBubble({
  text,
  phase,
  announcement = "",
  onVisibleChange,
}: {
  text: string;
  phase: VoiceSessionPhase;
  /** dsh 事件播报台词（空串表示无插播） */
  announcement?: string;
  onVisibleChange?: (visible: boolean) => void;
}) {
  const [visibleText, setVisibleText] = useState("");
  const [fading, setFading] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // 定格/淡出标志、展示来源与最新文本走 ref：effect 依赖只挂 text/phase/announcement，
  // 定格计时器不被自身状态变化重置。holdingRef 标记「已进入定格→淡出生命周期」，
  // 防止定格期内 phase 重入（如播完回 armed）误清内容。
  const fadingRef = useRef(false);
  const holdingRef = useRef(false);
  const sourceRef = useRef<ContentSource | null>(null);
  const visibleTextRef = useRef("");
  // 被流式回复压制期间暂存的插播（含到达时间，用于新鲜期判定）
  const pendingAnnouncementRef = useRef<{ text: string; at: number } | null>(null);
  const lastAnnouncementRef = useRef("");
  const freshTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    const clearDisplay = () => {
      clearTimeout(timerRef.current);
      holdingRef.current = false;
      fadingRef.current = false;
      setFading(false);
      sourceRef.current = null;
      visibleTextRef.current = "";
      setVisibleText("");
    };
    /** 展示当前内容：取消在途定时 → 定格 HOLD_MS → 淡出 FADE_MS → 移除。 */
    const show = (content: string, source: ContentSource) => {
      clearTimeout(timerRef.current);
      holdingRef.current = true;
      fadingRef.current = false;
      setFading(false);
      sourceRef.current = source;
      visibleTextRef.current = content;
      setVisibleText(content);
      timerRef.current = setTimeout(() => {
        fadingRef.current = true;
        setFading(true);
        timerRef.current = setTimeout(clearDisplay, FADE_MS);
      }, HOLD_MS);
    };

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

    if (text) {
      // 流式更新中：跟随最新文本，取消任何待定的淡出/插播展示
      clearTimeout(timerRef.current);
      holdingRef.current = false;
      fadingRef.current = false;
      setFading(false);
      sourceRef.current = "reply";
      visibleTextRef.current = text;
      setVisibleText(text);
      return;
    }

    // 无流式文本 → 有新鲜插播则展示（覆盖定格/淡出中的旧内容，最新发言胜出）
    const pending = pendingAnnouncementRef.current;
    if (pending && Date.now() - pending.at <= ANNOUNCEMENT_FRESH_MS) {
      clearTimeout(freshTimerRef.current);
      pendingAnnouncementRef.current = null;
      show(pending.text, "announcement");
      return;
    }

    // 回复文本清空后的语义（插播展示由自身定时管理，不归此处管）
    if (sourceRef.current !== "reply" || !visibleTextRef.current) return;
    if (holdingRef.current || fadingRef.current) return; // 定格/淡出中：phase 变化不打断（播完回 armed 属正常）
    if (phase === "armed" || phase === "idle") {
      // 打断 / 停止：立即消失
      clearDisplay();
      return;
    }
    // 正常完结：定格后淡出。**不返回 cleanup**——text/phase 变化触发的 effect 重跑
    // 不应清掉定格计时器（定格期内 phase 回 armed 属正常播完路径）；计时器只被
    // 新一轮文本/插播（上方分支）或卸载（下方独立 effect）清除。
    show(visibleTextRef.current, "reply");
  }, [text, phase, announcement]);

  // 卸载时清理定时器
  useEffect(
    () => () => {
      clearTimeout(timerRef.current);
      clearTimeout(freshTimerRef.current);
    },
    [],
  );

  // 上报可见性（窗口根组件据此切换点击穿透）
  useEffect(() => {
    onVisibleChange?.(visibleText !== "");
  }, [visibleText, onVisibleChange]);

  if (!visibleText) return null;

  return (
    // 纯展示气泡无输入/选择交互，整个可视面按住左键即拖动窗口（代价：文本不可选中）。
    // biome-ignore lint/a11y/noStaticElementInteractions: 气泡面即拖动把手（startDragging），无键盘等价交互（窗口拖动由 OS 承载）
    <div
      className="w-full cursor-grab touch-none select-none active:cursor-grabbing"
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        void api.bubbleDebugLog({ message: "气泡 mousedown 到达 → startDragging" });
        getCurrentWindow()
          .startDragging()
          .catch((err) => void api.bubbleDebugLog({ message: `startDragging 失败: ${err}` }));
      }}
    >
      <div
        className={`max-h-32 w-full overflow-hidden rounded-xl border border-border bg-popover px-4 py-2.5 text-sm text-text-primary shadow-lg transition-opacity duration-500 ${
          fading ? "opacity-0" : "opacity-100"
        }`}
      >
        <p className="line-clamp-4 whitespace-pre-wrap break-words">{visibleText}</p>
      </div>
    </div>
  );
}
