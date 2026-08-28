import { CircleAlert, Download, Trash2 } from "lucide-react";
import { useState } from "react";
import { ModelConfirmDialog } from "@/components/models/ModelConfirmDialog";
import { ModelDialog } from "@/components/models/ModelDialog";
import { ttsModelKindLabel } from "@/components/tts/ttsMeta";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useSmoothProgress } from "@/hooks/useSmoothProgress";
import { TTS_PRESETS, useTtsModelSwitch } from "@/hooks/useTtsModelSwitch";
import { formatBytes } from "@/lib/utils";
import type { LibraryModel } from "@/types/modelLibrary";

interface TtsModelDialogProps {
  open: boolean;
  onClose: () => void;
}

/**
 * 选择合成模型弹窗（与 KWS/ASR/LLM 选择模型弹窗同款交互）：
 * 内置预设（未安装→下载；已安装→设为当前 / 卸载；当前→标记）。
 * TTS 每次合成现场建引擎：切换立即生效（下次合成使用新模型）；
 * 语音会话运行中也静默生效（下次会话用新模型），无需重启提示。
 * 卸载确认框嵌套在此弹窗内。
 */
export function TtsModelDialog({ open, onClose }: TtsModelDialogProps) {
  const switcher = useTtsModelSwitch();
  const [confirmModel, setConfirmModel] = useState<LibraryModel | null>(null);
  const { downloadingId, progress } = switcher;

  // verifying/done 等阶段后端 overallPercent=-1，非 downloading 一律按 100
  const targetPercent =
    progress?.stage === "downloading" ? Math.max(0, Math.min(100, progress.overallPercent)) : 100;
  // 平滑插值：消除高频进度事件造成的进度条抖动
  const percent = useSmoothProgress(targetPercent);

  return (
    <ModelDialog open={open} onClose={onClose} title="选择合成模型" width="lg">
      <p className="text-xs text-text-muted">
        内置语音合成模型：下载后即可设为当前；切换立即生效（下次合成使用新模型）。
      </p>

      <div className="space-y-2">
        {TTS_PRESETS.map((p) => {
          // 仅「完整已安装」视为已安装（list_model_library 对未安装 registry 模型也返回记录）
          const installed =
            (switcher.models ?? []).find(
              (m) => (m.id === p.id || m.repoId === p.id) && m.installState === "installed",
            ) ?? null;
          const busy = downloadingId === p.id;
          return (
            <div
              key={p.id}
              className="flex items-center justify-between gap-3 rounded-lg border border-panel-border px-3 py-2.5"
            >
              <div className="min-w-0">
                <div className="flex items-center gap-2 text-sm font-medium text-text-primary">
                  {p.name}
                  <Badge
                    variant="outline"
                    className="shrink-0 border-violet-500/20 bg-violet-500/10 px-1.5 py-0 text-[10px] text-violet-600"
                  >
                    {ttsModelKindLabel(p.kind)}
                  </Badge>
                </div>
                <p className="mt-0.5 text-xs text-text-muted">
                  {`${p.languages} · ${formatBytes(p.sizeBytes)} · ${p.tagline}`}
                </p>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {installed ? (
                  installed.current ? (
                    <span className="inline-flex items-center gap-1.5 text-xs text-emerald-600">
                      <span className="h-1.5 w-1.5 rounded-full bg-current" />
                      当前模型
                    </span>
                  ) : (
                    <>
                      <Button size="sm" onClick={() => void switcher.setCurrent(installed.id)}>
                        设为当前
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        className="shadow-none text-destructive hover:text-destructive"
                        onClick={() => setConfirmModel(installed)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        卸载
                      </Button>
                    </>
                  )
                ) : (
                  <Button
                    size="sm"
                    onClick={() => void switcher.download(p.id)}
                    disabled={downloadingId !== null}
                    aria-label={`下载${p.name}`}
                  >
                    <Download className="h-4 w-4" />
                    {busy ? "下载中…" : "下载"}
                  </Button>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {progress && (
        <div className="space-y-1">
          <Progress value={percent} />
          <p className="text-xs text-text-muted">{progress.message}</p>
        </div>
      )}

      {switcher.error && (
        <Alert variant="destructive">
          <CircleAlert className="h-4 w-4" />
          <AlertDescription className="whitespace-pre-wrap">
            读取模型列表失败：{switcher.error}
          </AlertDescription>
        </Alert>
      )}

      <ModelConfirmDialog
        open={confirmModel !== null}
        model={confirmModel}
        onClose={() => setConfirmModel(null)}
        onConfirm={(m) => {
          setConfirmModel(null);
          void switcher.remove(m.id);
        }}
      />
    </ModelDialog>
  );
}
