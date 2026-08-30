import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CircleAlert, Mic, Trash2, Upload, UserRoundPlus, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useToast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";

/** 退出动画时长，需与卡片/遮罩的 duration 一致。 */
const EXIT_MS = 200;
/** 在线录音可选时长（秒）。 */
const RECORD_SECONDS = [3, 5, 10] as const;
/** speaker_id 合法性（与后端 profiles::validate_speaker_id 对齐）。 */
const SPEAKER_ID_RE = /^[A-Za-z0-9_-]+$/;

interface SpeakerEnrollDialogProps {
  open: boolean;
  onClose: () => void;
}

/**
 * 注册说话人对话框：输入 id → 录多段样本（或选择 wav 文件）→ 完成注册。
 * 成功后后端清理本次录音的临时 wav；注册结果对运行中的语音会话即时生效。
 */
export function SpeakerEnrollDialog({ open, onClose }: SpeakerEnrollDialogProps) {
  const { speaker, anyListening, device } = useRuntime();
  const toast = useToast();
  const [mounted, setMounted] = useState(open);
  const [closing, setClosing] = useState(false);
  const [speakerId, setSpeakerId] = useState("");
  /** 待注册样本（自增 id 作 React key，路径可能重复） */
  const [samples, setSamples] = useState<{ id: number; path: string }[]>([]);
  const [sampleSeq, setSampleSeq] = useState(0);
  const [recordSeconds, setRecordSeconds] = useState(5);
  const [enrolling, setEnrolling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 录音/文件选择进行中的本地态（录音态以 runtime speakers.recording 为准）
  const recording = speaker.speakers.recording;

  useEffect(() => {
    if (open) {
      setMounted(true);
      setClosing(false);
      setSpeakerId("");
      setSamples([]);
      setSampleSeq(0);
      setRecordSeconds(5);
      setError(null);
    }
  }, [open]);

  const finishClose = useCallback(() => {
    setMounted(false);
    setClosing(false);
    onClose();
  }, [onClose]);

  const close = useCallback(() => {
    if (closing || recording || speaker.speakers.busy) return;
    setClosing(true);
    window.setTimeout(finishClose, EXIT_MS);
  }, [closing, recording, speaker.speakers.busy, finishClose]);

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
    try {
      const path = await speaker.speakers.recordSample(recordSeconds, device || null);
      setSamples((prev) => [...prev, { id: sampleSeq + 1, path }]);
      setSampleSeq((n) => n + 1);
    } catch (e) {
      setError(String(e));
    }
  };

  const pickWavs = async () => {
    setError(null);
    try {
      const picked = await openDialog({
        multiple: true,
        title: "选择注册音频（可多选）",
        filters: [{ name: "WAV", extensions: ["wav"] }],
      });
      if (Array.isArray(picked) && picked.length > 0) {
        const paths = picked.filter((p): p is string => typeof p === "string");
        setSamples((prev) => {
          let seq = sampleSeq;
          const added = paths.map((path) => ({ id: ++seq, path }));
          setSampleSeq(seq);
          return [...prev, ...added];
        });
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const idValid = SPEAKER_ID_RE.test(speakerId.trim());
  const canEnroll = idValid && samples.length > 0 && !enrolling && !recording;

  const handleEnroll = async () => {
    setEnrolling(true);
    setError(null);
    try {
      const summary = await speaker.speakers.enroll(
        speakerId.trim(),
        samples.map((s) => s.path),
      );
      toast.success(
        `已注册 ${summary.speaker_id}：${summary.sample_count} 段样本（embedding 维度 ${summary.dim}）`,
      );
      finishClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setEnrolling(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-label="注册说话人"
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
          <h3 className="text-sm font-semibold text-text-primary">注册说话人</h3>
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
          {/* 说话人 id */}
          <div className="space-y-1.5">
            <label className="text-sm text-text-primary" htmlFor="speaker-id">
              说话人 ID
            </label>
            <Input
              id="speaker-id"
              value={speakerId}
              onChange={(e) => setSpeakerId(e.target.value)}
              placeholder="例如：owner（仅英文字母、数字、下划线、连字符）"
            />
            {speakerId.trim() !== "" && !idValid && (
              <p className="text-xs text-amber-600">
                仅允许英文字母、数字、下划线（_）和连字符（-）
              </p>
            )}
          </div>

          {/* 样本来源：录音或选择文件，可多次追加 */}
          <div className="space-y-1.5">
            <p className="text-sm text-text-primary">注册音频（建议 2 段以上，覆盖不同语气）</p>
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
              <Button
                variant="outline"
                size="sm"
                onClick={() => void record()}
                disabled={recording || anyListening}
              >
                <Mic className="h-4 w-4" />
                {recording ? "录音中…" : "录制一段"}
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void pickWavs()}
                disabled={recording}
              >
                <Upload className="h-4 w-4" />
                选择 wav 文件
              </Button>
            </div>
            {anyListening && !recording && (
              <p className="text-xs text-text-muted">语音会话/监听进行中，录音暂不可用。</p>
            )}
          </div>

          {/* 已添加样本列表 */}
          {samples.length > 0 && (
            <ul className="divide-y divide-divider rounded-md border border-panel-border bg-app-background/60">
              {samples.map((s, idx) => (
                <li key={s.id} className="flex items-center justify-between gap-3 px-3 py-2">
                  <span className="truncate font-mono text-xs text-text-muted" title={s.path}>
                    样本 {idx + 1}：{s.path}
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 shrink-0 text-text-muted hover:bg-destructive hover:text-destructive-foreground"
                    onClick={() => setSamples((prev) => prev.filter((it) => it.id !== s.id))}
                    aria-label={`移除样本 ${idx + 1}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </li>
              ))}
            </ul>
          )}

          {error && (
            <Alert variant="destructive">
              <CircleAlert className="h-4 w-4" />
              <AlertDescription className="whitespace-pre-wrap">{error}</AlertDescription>
            </Alert>
          )}

          <div className="flex items-center justify-between gap-2 border-t border-divider pt-3">
            <p className="text-xs text-text-muted">
              注册后即可在语音会话中识别（运行中的会话即时生效）
            </p>
            <Button size="sm" onClick={() => void handleEnroll()} disabled={!canEnroll}>
              <UserRoundPlus className="h-4 w-4" />
              {enrolling ? "注册中…" : "完成注册"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
