import { CircleAlert, Send, Square, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import { currentModelName } from "./llmMeta";

/** 退出动画时长，需与卡片/遮罩的 duration 一致。 */
const EXIT_MS = 200;

interface LlmTestDialogProps {
  open: boolean;
  onClose: () => void;
}

/**
 * 测试模型对话框：复用全局 useLlm 的 chat/stop/generating/response，保留 streaming 与 cancel。
 * 直接显示 llm.response（含 thinking 原文），关闭只关 UI，不 unload、不 reset。
 */
export function LlmTestDialog({ open, onClose }: LlmTestDialogProps) {
  const { llm } = useRuntime();
  const [mounted, setMounted] = useState(open);
  const [closing, setClosing] = useState(false);
  const [text, setText] = useState("");

  // 打开时挂载并播放进场动画；每次打开清空输入框
  useEffect(() => {
    if (open) {
      setMounted(true);
      setClosing(false);
      setText("");
    }
  }, [open]);

  const finishClose = useCallback(() => {
    setMounted(false);
    setClosing(false);
    onClose();
  }, [onClose]);

  const close = useCallback(() => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(finishClose, EXIT_MS);
  }, [closing, finishClose]);

  // Esc 取消
  useEffect(() => {
    if (!mounted || closing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mounted, closing, close]);

  if (!mounted) return null;

  const modelName = currentModelName(llm.config) ?? "未选择模型";

  const handleSend = () => {
    if (!llm.ready || llm.generating) return;
    const trimmed = text.trim();
    if (!trimmed) return;
    void llm.chat(trimmed);
    setText("");
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-label="测试模型"
    >
      <button
        type="button"
        tabIndex={-1}
        aria-label="关闭对话框"
        className={cn(
          "absolute inset-0 cursor-default bg-black/20",
          closing ? "animate-out fade-out-0 duration-200" : "animate-in fade-in-0 duration-200",
        )}
        onClick={close}
      />
      <div
        className={cn(
          "relative flex max-h-[85vh] w-full max-w-xl flex-col rounded-xl border border-panel-border bg-panel-background",
          closing
            ? "animate-out fade-out-0 zoom-out-95 duration-200 ease-in"
            : "animate-in fade-in-0 zoom-in-95 duration-200 ease-out",
        )}
      >
        <div className="flex items-start justify-between gap-4 border-b border-divider px-5 py-4">
          <div className="min-w-0">
            <h3 className="text-sm font-semibold text-text-primary">测试 AI 大脑</h3>
            <p className="mt-0.5 truncate text-xs text-text-muted">当前模型：{modelName}</p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 shrink-0"
            onClick={close}
            aria-label="关闭"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>

        <div className="flex-1 space-y-3 overflow-y-auto px-5 py-4">
          {!llm.ready && (
            <p className="text-xs text-text-muted">
              模型未连接，无法测试。请先开启顶部的连接开关。
            </p>
          )}

          <textarea
            className="w-full rounded-md border border-panel-border bg-app-background/60 p-3 text-sm text-text-primary outline-none transition-colors focus:border-primary/50 focus:ring-1 focus:ring-primary/20"
            rows={4}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                handleSend();
              }
            }}
            placeholder="输入测试消息…（Enter 发送，Shift+Enter 换行）"
            aria-label="测试消息"
            disabled={!llm.ready}
          />

          <div className="flex items-center justify-end gap-2">
            <Button
              className="shadow-none"
              onClick={handleSend}
              disabled={!llm.ready || llm.generating || !text.trim()}
            >
              <Send className="h-4 w-4" />
              发送
            </Button>
            {llm.generating && (
              <Button variant="destructive" className="shadow-none" onClick={() => void llm.stop()}>
                <Square className="h-4 w-4" />
                停止
              </Button>
            )}
          </div>

          <div className="rounded-md border border-panel-border bg-app-background/60 p-3">
            {!llm.response && !llm.generating && (
              <p className="text-xs text-text-muted">发送一条消息开始测试。</p>
            )}
            {llm.generating && <p className="mb-1 text-xs text-text-muted">生成中…</p>}
            {llm.response && (
              <p className="whitespace-pre-wrap text-sm text-text-primary">{llm.response}</p>
            )}
          </div>

          {llm.error && (
            <Alert variant="destructive">
              <CircleAlert className="h-4 w-4" />
              <AlertDescription className="whitespace-pre-wrap">{llm.error}</AlertDescription>
            </Alert>
          )}
        </div>
      </div>
    </div>
  );
}
