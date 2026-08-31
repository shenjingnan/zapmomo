import { CircleAlert, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";

/** 退出动画时长，需与卡片/遮罩的 duration 一致。 */
const EXIT_MS = 200;

interface KwsTestDialogProps {
  open: boolean;
  onClose: () => void;
  /** 测试目标关键词；缺省回退全局自定义唤醒词（会话级）→ 模型内置词 */
  keywords?: string | null;
  /** 对话框标题（伙伴页按角色测试时显示「测试伙伴唤醒词」等） */
  title?: string;
}

/**
 * 测试唤醒词对话框：只展示实时检测结果（配置字段在主页面，不在此重复）。
 * 打开时若未在监听则自动开始一次测试监听（关键词由调用方指定）；关闭时自动停止
 * 本次测试监听。打开前已监听（顶部开关开启）→ 关闭绝不停止。
 */
export function KwsTestDialog({
  open,
  onClose,
  keywords,
  title = "测试唤醒词",
}: KwsTestDialogProps) {
  const { kws, device, sessionKeywords } = useRuntime();
  const [mounted, setMounted] = useState(open);
  const [closing, setClosing] = useState(false);
  // 本次监听是否由本对话框发起：决定关闭时是否自动停止
  const startedByDialog = useRef(false);
  // 防止打开后对同一状态反复自动 start
  const autoStartHandled = useRef(false);
  // 始终指向最新的 listening 状态（供关闭时的异步等待使用）
  const listeningRef = useRef(kws.listening);
  useEffect(() => {
    listeningRef.current = kws.listening;
  }, [kws.listening]);

  // 打开时挂载并播放进场动画；重置归属追踪
  useEffect(() => {
    if (open) {
      setMounted(true);
      setClosing(false);
      startedByDialog.current = false;
      autoStartHandled.current = false;
    }
  }, [open]);

  // 打开时若未在监听：自动开始一次测试监听（仅一次）
  useEffect(() => {
    if (!open || !mounted || closing || autoStartHandled.current) return;
    if (kws.listening.isListening) {
      autoStartHandled.current = true; // 已在监听：无需再启动
      return;
    }
    if (kws.listening.pending) return; // 等正在进行的操作完成
    autoStartHandled.current = true;
    startedByDialog.current = true;
    void kws.listening.start(device || null, keywords ?? sessionKeywords ?? null);
  }, [
    open,
    mounted,
    closing,
    kws.listening.isListening,
    kws.listening.pending,
    device,
    keywords,
    sessionKeywords,
    kws.listening.start,
  ]);

  // 停止本对话框发起的监听：等 start 在途结束后再停，避免遗留后台监听
  const stopDialogListen = useCallback(async () => {
    while (listeningRef.current.pending) {
      await new Promise((r) => setTimeout(r, 50));
    }
    if (listeningRef.current.isListening) {
      await kws.listening.stop();
    }
  }, [kws.listening.stop]);

  const finishClose = useCallback(() => {
    const mine = startedByDialog.current;
    setMounted(false);
    setClosing(false);
    onClose();
    // 仅停止「本对话框发起」的监听；打开前已有的监听绝不在此停止
    if (mine) void stopDialogListen();
  }, [onClose, stopDialogListen]);

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

  const { listening } = kws;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-label={title}
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
        <div className="flex items-center justify-between gap-4 border-b border-divider px-5 py-4">
          <h3 className="text-sm font-semibold text-text-primary">{title}</h3>
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
          <div className="flex items-center gap-2">
            <span
              className={cn(
                "inline-flex items-center gap-1.5 text-sm font-medium",
                listening.isListening ? "text-emerald-600" : "text-text-muted",
              )}
            >
              <span className="h-1.5 w-1.5 rounded-full bg-current" />
              {listening.isListening ? "正在监听" : "未监听"}
            </span>
          </div>

          <div className="rounded-md border border-panel-border bg-app-background/60">
            <div className="border-b border-divider px-3.5 py-2">
              <p className="text-sm font-medium text-text-primary">最近检测结果</p>
            </div>
            {kws.results.length === 0 ? (
              <p className="px-3.5 py-3 text-sm text-text-muted">尚未检测到唤醒词</p>
            ) : (
              <ul className="max-h-56 overflow-y-auto px-3.5">
                {kws.results.map((r) => (
                  <li
                    key={r.id}
                    className="flex items-center justify-between gap-3 border-b border-divider py-1.5 text-sm last:border-b-0"
                  >
                    <span className="font-medium text-text-primary">“{r.keyword}”</span>
                    <span className="text-xs text-text-muted">{r.at}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>

          {listening.error && (
            <Alert variant="destructive">
              <CircleAlert className="h-4 w-4" />
              <AlertDescription className="whitespace-pre-wrap">{listening.error}</AlertDescription>
            </Alert>
          )}
        </div>

        <div className="border-t border-divider px-5 py-3">
          <p className="text-xs text-text-muted">在本窗口内开启的监听，关闭时自动停止。</p>
        </div>
      </div>
    </div>
  );
}
