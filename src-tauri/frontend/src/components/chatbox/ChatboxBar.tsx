import { LogicalSize, PhysicalPosition } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { GripVertical, SendHorizontal } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/tauri";

/** 与后端 CHATBOX_W / CHATBOX_H 一致（lib.rs 建窗尺寸，双端常量）。 */
const WINDOW_W = 520;
const BASE_WINDOW_H = 96;
/** 文本域最大行数，超出后内部滚动。 */
const MAX_LINES = 4;
/** 底部透明外边距：给 CSS 阴影留的扩散空间（透明窗口会裁剪窗口外的阴影）。 */
const MARGIN_BOTTOM = 26;
/** 顶部最小留白（与底部外边距一起决定窗口高度计算）。 */
const PAD_TOP = 12;
/** 错误提示行占位高度（text-[11px] 行 + gap-0.5）。 */
const ERROR_LINE_H = 18;

/**
 * 文字输入条（chatbox 窗口根组件）：galgame 式迷你输入条。
 *
 * Enter 发送，Shift+Enter 换行（IME 组词中的 Enter 不触发发送）；
 * 文字与 ASR 最终文本等价，走后端语音会话的 LLM → TTS 完整链路。
 * Esc 关闭窗口（持久化隐藏，托盘/右键菜单可重新打开）。左侧把手拖动窗口，
 * 拖动停止后位置写回 settings 供下次启动恢复。
 * 多行时窗口随内容向上生长（底边锚定），与默认「屏幕底部居中」位置协调；
 * 发送清空后缩回单行高度。
 *
 * 本窗口是普通可激活窗口（非 nspanel），键盘聚焦与中文 IME 行为标准。
 */
export function ChatboxBar() {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const errorTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // 窗口聚焦时把焦点放进输入框（从菜单/快捷键打开后可立即打字）
  useEffect(() => {
    textareaRef.current?.focus();
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) textareaRef.current?.focus();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 监听窗口移动：拖动停止（debounce）后把逻辑像素坐标写回 settings。
  useEffect(() => {
    const win = getCurrentWindow();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unlisten = win.onMoved(({ payload }) => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        void (async () => {
          const factor = await win.scaleFactor();
          const x = Math.round(payload.x / factor);
          const y = Math.round(payload.y / factor);
          await api.saveChatboxPosition({ x, y });
        })();
      }, 300);
    });
    return () => {
      clearTimeout(timer);
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 卸载时清理错误定时器
  useEffect(() => {
    return () => clearTimeout(errorTimerRef.current);
  }, []);

  // 文本域自动高度（1–4 行，超出内部滚动）+ 窗口随内容向上生长（底边锚定）。
  // biome-ignore lint/correctness/useExhaustiveDependencies: text 不直接引用，但作为触发器重算 DOM 高度
  useEffect(() => {
    const ta = textareaRef.current;
    const bar = barRef.current;
    if (!ta || !bar) return;
    ta.style.height = "auto";
    const style = getComputedStyle(ta);
    const line = parseFloat(style.lineHeight) || 24;
    const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
    const maxTa = line * MAX_LINES + padY;
    ta.style.height = `${Math.min(ta.scrollHeight, maxTa)}px`;
    ta.style.overflowY = ta.scrollHeight > maxTa ? "auto" : "hidden";

    const desired = Math.max(
      BASE_WINDOW_H,
      Math.ceil(bar.getBoundingClientRect().height) +
        MARGIN_BOTTOM +
        PAD_TOP +
        (error ? ERROR_LINE_H : 0),
    );
    void (async () => {
      const win = getCurrentWindow();
      const factor = await win.scaleFactor();
      const curH = (await win.innerSize()).height / factor;
      if (Math.abs(curH - desired) < 1) return;
      const pos = await win.outerPosition();
      const dy = desired - curH;
      await win.setSize(new LogicalSize(WINDOW_W, desired));
      // 底边锚定：长高往上顶、缩短往下落
      await win.setPosition(new PhysicalPosition(pos.x, pos.y - Math.round(dy * factor)));
    })();
  }, [text, error]);

  const showError = (message: string) => {
    setError(message);
    clearTimeout(errorTimerRef.current);
    errorTimerRef.current = setTimeout(() => setError(null), 3000);
  };

  const send = async () => {
    const trimmed = text.trim();
    if (!trimmed || sending) return;
    setSending(true);
    setError(null);
    try {
      await api.sendVoiceText({ text: trimmed });
      setText("");
    } catch (e) {
      showError(typeof e === "string" ? e : String(e));
    } finally {
      setSending(false);
      textareaRef.current?.focus();
    }
  };

  return (
    <div className="flex h-screen w-screen flex-col justify-end gap-0.5 bg-transparent px-4 pt-3 pb-[26px]">
      <div
        ref={barRef}
        className="flex w-full items-center gap-1.5 rounded-full bg-popover px-2 py-1.5 shadow-[0_4px_16px_rgba(0,0,0,0.14),0_0_4px_rgba(0,0,0,0.06)]"
      >
        {/* 拖拽把手（输入框区域不抢拖拽） */}
        <button
          type="button"
          aria-label="拖动输入条"
          className="shrink-0 cursor-grab touch-none rounded-full p-1 text-muted-foreground hover:bg-accent active:cursor-grabbing"
          onMouseDown={(e) => {
            if (e.button !== 0) return;
            void getCurrentWindow().startDragging();
          }}
        >
          <GripVertical className="size-4" />
        </button>
        <textarea
          ref={textareaRef}
          rows={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter 发送；Shift+Enter 换行（走默认行为）；IME 组词中的 Enter 只上屏
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            } else if (e.key === "Escape") {
              e.preventDefault();
              void api.hideChatbox();
            }
          }}
          placeholder="和 AI 伙伴说点什么…"
          aria-label="消息输入框"
          className="min-h-8 flex-1 resize-none self-center border-0 bg-transparent px-1 py-1 text-sm leading-6 shadow-none focus-visible:outline-none"
        />
        <Button
          type="button"
          size="icon"
          aria-label="发送"
          disabled={sending || text.trim().length === 0}
          onClick={() => void send()}
          className="size-8 shrink-0 rounded-full"
        >
          <SendHorizontal className="size-4" />
        </Button>
      </div>
      {error && (
        <p role="alert" className="truncate px-4 text-center text-[11px] text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}
