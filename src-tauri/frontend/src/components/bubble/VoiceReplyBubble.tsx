import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/tauri";

/** 拖动判定位移阈值（CSS 像素）：超过视为拖动，未超过松开视为点击关闭。 */
const DRAG_THRESHOLD_PX = 5;
/** 插播新鲜期（毫秒）：被流式回复压制超过此时长的插播到期丢弃，完结后不再补展示。 */
const ANNOUNCEMENT_FRESH_MS = 5000;

/** 当前展示内容的来源。 */
type ContentSource = "reply" | "announcement";

/**
 * 聊天气泡（独立 bubble 窗口的唯一内容视图，有且只有一个聊天气泡）。
 *
 * 内容两路共用同一气泡，流式回复优先级恒高于插播：
 * - `text`：语音/文字对话的流式回复（token 累积 = 天然打字机）。text 清空
 *   （正常完结或被打断/停止）后内容静置保留，不自动消失。
 * - `announcement`：dsh（DeepSeek Harness）事件播报台词。被流式回复压制时暂存，
 *   回复完结后新鲜期内补展示，超期丢弃；展示中新插播替换旧插播（最新发言胜出）。
 *
 * 内容一旦出现即静置常驻，唯一消失途径是用户点击气泡（新一轮文本/插播到达时
 * 自然顶替除外）——「想看的内容不被程序收走」。点击与拖动共用气泡面：按住后
 * 位移超过阈值才交给 OS 拖动窗口，未超阈值松开视为点击关闭。
 *
 * 整个气泡面按住左键拖动窗口（纯展示组件、无输入/选择交互，故文本不可选中）。
 * 可见性变化经 `onVisibleChange` 上报，窗口根组件据此切换点击穿透：
 * 有内容时可交互，无内容时透明区域穿透到下方窗口。
 */
export function VoiceReplyBubble({
  text,
  announcement = "",
  onVisibleChange,
}: {
  text: string;
  /** dsh 事件播报台词（空串表示无插播） */
  announcement?: string;
  onVisibleChange?: (visible: boolean) => void;
}) {
  const [visibleText, setVisibleText] = useState("");
  const sourceRef = useRef<ContentSource | null>(null);
  const visibleTextRef = useRef("");
  // 被流式回复压制期间暂存的插播（含到达时间，用于新鲜期判定）
  const pendingAnnouncementRef = useRef<{ text: string; at: number } | null>(null);
  const lastAnnouncementRef = useRef("");
  const freshTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
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

    if (text) {
      // 流式更新中：跟随最新文本
      sourceRef.current = "reply";
      visibleTextRef.current = text;
      setVisibleText(text);
      return;
    }

    // 无流式文本 → 有新鲜插播则展示（覆盖静置中的旧内容，最新发言胜出）
    const pending = pendingAnnouncementRef.current;
    if (pending && Date.now() - pending.at <= ANNOUNCEMENT_FRESH_MS) {
      clearTimeout(freshTimerRef.current);
      pendingAnnouncementRef.current = null;
      sourceRef.current = "announcement";
      visibleTextRef.current = pending.text;
      setVisibleText(pending.text);
      return;
    }

    // text 清空（正常完结 / 打断 / 停止）：内容静置保留，等用户点击关闭。
    // 同 props 重渲染不复活——展示仅由新内容或点击关闭驱动。
  }, [text, announcement]);

  // 卸载时清理定时器
  useEffect(
    () => () => {
      clearTimeout(freshTimerRef.current);
    },
    [],
  );

  // 上报可见性（窗口根组件据此切换点击穿透）
  useEffect(() => {
    onVisibleChange?.(visibleText !== "");
  }, [visibleText, onVisibleChange]);

  if (!visibleText) return null;

  /** 点击关闭：清空展示（ref 与 state 同步），气泡随之消失、窗口回到点穿态。 */
  const dismiss = () => {
    sourceRef.current = null;
    visibleTextRef.current = "";
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
      {/* 内容完整展示不截断（静置常驻的前提是看得全）；超高兜底：约 20 行起内部滚动 */}
      <div className="max-h-[400px] w-full overflow-y-auto rounded-xl border border-border bg-popover px-4 py-2.5 text-sm text-text-primary shadow-lg">
        <p className="whitespace-pre-wrap break-words">{visibleText}</p>
      </div>
    </div>
  );
}
