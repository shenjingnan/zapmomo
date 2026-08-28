import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/tauri";
import type { VoiceSessionPhase } from "@/types/tauri";

/** 回复完结后气泡定格时长（毫秒），之后淡出消失。 */
const HOLD_MS = 5000;

/**
 * 语音回复气泡（独立 bubble 窗口的内容，galgame 对话框位）。
 *
 * `text` 来自 useVoiceSession 的 pendingReply（流式 token 累积 = 天然打字机）。
 * 清空分两义：正常完结（reply-finished，此刻 phase 仍在 thinking/speaking）→
 * 定格 5s 后淡出；打断/停止（phase 已回 armed/idle）→ 立即消失。
 * 定格期内的 phase 迁移（如播完回 armed）不打断定格。
 *
 * 整个气泡面按住左键即可拖动窗口（纯展示组件、无输入/选择交互，故文本不可选中）。
 * 可见性变化经 `onVisibleChange` 上报，窗口根组件据此切换点击穿透：
 * 有内容时可拖动，无内容时透明区域穿透到下方窗口。
 */
export function VoiceReplyBubble({
  text,
  phase,
  onVisibleChange,
}: {
  text: string;
  phase: VoiceSessionPhase;
  onVisibleChange?: (visible: boolean) => void;
}) {
  const [visibleText, setVisibleText] = useState("");
  const [fading, setFading] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // 定格标志与最新文本走 ref：effect 依赖只挂 text/phase，定格计时器不被自身状态变化重置
  const fadingRef = useRef(false);
  const visibleTextRef = useRef("");

  useEffect(() => {
    if (text) {
      // 流式更新中：跟随最新文本，取消任何待定的淡出
      clearTimeout(timerRef.current);
      fadingRef.current = false;
      setFading(false);
      visibleTextRef.current = text;
      setVisibleText(text);
      return;
    }
    if (fadingRef.current) return; // 定格中：phase 变化（如播完回 armed）不打断定格
    if (!visibleTextRef.current) return; // 无可定格内容
    if (phase === "armed" || phase === "idle") {
      // 打断 / 停止：立即消失
      visibleTextRef.current = "";
      setVisibleText("");
      return;
    }
    // 正常完结：定格后淡出。**不返回 cleanup**——text/phase 变化触发的 effect 重跑
    // 不应清掉定格计时器（定格期内 phase 回 armed 属正常播完路径）；计时器只被
    // 新一轮文本（上方 text 分支）或卸载（下方独立 effect）清除。
    fadingRef.current = true;
    setFading(true);
    timerRef.current = setTimeout(() => {
      fadingRef.current = false;
      setFading(false);
      visibleTextRef.current = "";
      setVisibleText("");
    }, HOLD_MS);
  }, [text, phase]);

  // 卸载时清理定时器
  useEffect(() => () => clearTimeout(timerRef.current), []);

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
