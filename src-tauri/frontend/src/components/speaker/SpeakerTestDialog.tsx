import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CircleAlert, Mic, ScanSearch, Upload, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { api } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import type { SpeakerIdentifyResult } from "@/types/tauri";

/** 退出动画时长，需与卡片/遮罩的 duration 一致。 */
const EXIT_MS = 200;
const RECORD_SECONDS = [3, 5, 10] as const;

interface SpeakerTestDialogProps {
  open: boolean;
  onClose: () => void;
}

/** 识别测试对话框：录一段（或选 wav）→ 1:N 识别 → 展示命中/分数表/延迟。 */
export function SpeakerTestDialog({ open, onClose }: SpeakerTestDialogProps) {
  const { speaker, device } = useRuntime();
  const [mounted, setMounted] = useState(open);
  const [closing, setClosing] = useState(false);
  const [wavPath, setWavPath] = useState<string | null>(null);
  const [recordSeconds, setRecordSeconds] = useState(5);
  const [result, setResult] = useState<SpeakerIdentifyResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const recording = speaker.speakers.recording;
  const busy = speaker.speakers.busy;

  useEffect(() => {
    if (open) {
      setMounted(true);
      setClosing(false);
      setWavPath(null);
      setResult(null);
      setRecordSeconds(5);
      setError(null);
    }
  }, [open]);

  const finishClose = useCallback(() => {
    setMounted(false);
    setClosing(false);
    onClose();
    // 录音期间可能自动挂起了语音会话/监听：关闭弹窗时恢复（后端幂等）
    api.speakerResumeMic().catch((e) => setError(String(e)));
  }, [onClose]);

  const close = useCallback(() => {
    if (closing || recording || busy) return;
    setClosing(true);
    window.setTimeout(finishClose, EXIT_MS);
  }, [closing, recording, busy, finishClose]);

  useEffect(() => {
    if (!mounted || closing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mounted, closing, close]);

  if (!mounted) return null;

  const record = async () => {
    setError(null);
    setResult(null);
    try {
      const path = await speaker.speakers.recordSample(recordSeconds, device || null);
      setWavPath(path);
    } catch (e) {
      setError(String(e));
    }
  };

  const pickWav = async () => {
    setError(null);
    setResult(null);
    try {
      const picked = await openDialog({
        multiple: false,
        title: "选择待识别音频",
        filters: [{ name: "WAV", extensions: ["wav"] }],
      });
      if (typeof picked === "string") setWavPath(picked);
    } catch (e) {
      setError(String(e));
    }
  };

  const runIdentify = async () => {
    if (!wavPath) return;
    setError(null);
    try {
      setResult(await speaker.speakers.identifyWav(wavPath));
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-label="识别测试"
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
          <h3 className="text-sm font-semibold text-text-primary">识别测试</h3>
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
          <div className="flex flex-wrap items-center gap-2">
            <Select
              value={String(recordSeconds)}
              onValueChange={(v) => setRecordSeconds(Number(v))}
              disabled={recording}
            >
              <SelectTrigger className="w-24" aria-label="录音时长">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {RECORD_SECONDS.map((s) => (
                  <SelectItem key={s} value={String(s)}>
                    {s} 秒
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button variant="outline" size="sm" onClick={() => void record()} disabled={recording}>
              <Mic className="h-4 w-4" />
              {recording ? "录音中…" : "录一段"}
            </Button>
            <Button variant="outline" size="sm" onClick={() => void pickWav()} disabled={recording}>
              <Upload className="h-4 w-4" />
              选择 wav
            </Button>
            <Button size="sm" onClick={() => void runIdentify()} disabled={!wavPath || busy}>
              <ScanSearch className="h-4 w-4" />
              {busy ? "识别中…" : "开始识别"}
            </Button>
          </div>
          <p className="text-xs text-text-muted">
            录音会自动暂停正在进行的语音会话/监听，关闭弹窗后自动恢复。
          </p>
          {wavPath && (
            <p className="truncate font-mono text-xs text-text-muted" title={wavPath}>
              {wavPath}
            </p>
          )}

          {error && (
            <Alert variant="destructive">
              <CircleAlert className="h-4 w-4" />
              <AlertDescription className="whitespace-pre-wrap">{error}</AlertDescription>
            </Alert>
          )}

          {result && (
            <div className="space-y-2 rounded-md border border-panel-border bg-app-background/60 p-3">
              {result.skipped ? (
                <p className="text-sm text-amber-600">已跳过识别：{result.skipped}</p>
              ) : (
                <p className="flex items-center gap-2 text-sm">
                  <span className="text-text-primary">{result.speaker_id ?? "unknown"}</span>
                  <Badge
                    className={
                      result.matched
                        ? "bg-emerald-100 text-emerald-700"
                        : "bg-amber-100 text-amber-700"
                    }
                  >
                    {result.matched ? "命中" : "未过阈值"}
                  </Badge>
                  <span className="font-mono text-xs text-text-muted">
                    score {result.score?.toFixed(3)} / threshold {result.threshold.toFixed(2)}
                  </span>
                </p>
              )}
              {result.scores.length > 0 && (
                <ul className="space-y-1">
                  {result.scores.map((s) => (
                    <li key={s.speaker_id} className="flex justify-between font-mono text-xs">
                      <span className="text-text-secondary">{s.speaker_id}</span>
                      <span className="text-text-muted">{s.score.toFixed(3)}</span>
                    </li>
                  ))}
                </ul>
              )}
              <p className="font-mono text-xs text-text-muted">
                latency: total {result.latency.total_ms.toFixed(0)}ms (audio{" "}
                {result.latency.audio_duration_ms.toFixed(0)}ms, embedding{" "}
                {result.latency.embedding_ms.toFixed(1)}ms, matching{" "}
                {result.latency.matching_ms.toFixed(1)}ms)
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
