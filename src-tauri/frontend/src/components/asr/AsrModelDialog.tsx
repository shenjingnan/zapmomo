import { CircleAlert, Download, Loader2, Trash2 } from "lucide-react";
import { useState } from "react";
import { ModelConfirmDialog } from "@/components/models/ModelConfirmDialog";
import { ModelDialog } from "@/components/models/ModelDialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { ASR_PRESETS, useAsrModelSwitch } from "@/hooks/useAsrModelSwitch";
import { useSmoothProgress } from "@/hooks/useSmoothProgress";
import { formatBytes } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import type { LibraryModel } from "@/types/modelLibrary";
import { asrModelKindLabel } from "./asrMeta";

interface AsrModelDialogProps {
  open: boolean;
  onClose: () => void;
}

/**
 * 选择识别模型弹窗（与 KWS/LLM 选择模型弹窗同款交互）：
 * 内置预设（未安装→下载；已安装→设为当前 / 卸载；当前→标记）。
 * 识别中切换由 useAsrModelSwitch 自动重启识别；语音会话运行中则下次会话生效。
 * 卸载确认框嵌套在此弹窗内。
 */
export function AsrModelDialog({ open, onClose }: AsrModelDialogProps) {
  const { voice } = useRuntime();
  const switcher = useAsrModelSwitch();
  const [confirmModel, setConfirmModel] = useState<LibraryModel | null>(null);
  /** 正在设为当前的预设 id；期间按钮转圈并禁用（识别中切换含重启识别耗时） */
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  const { downloadingId, progress } = switcher;

  const handleSetCurrent = async (id: string) => {
    setSwitchingId(id);
    try {
      await switcher.setCurrent(id);
    } finally {
      setSwitchingId(null);
    }
  };

  // verifying/done 等阶段后端 overallPercent=-1，非 downloading 一律按 100
  const targetPercent =
    progress?.stage === "downloading" ? Math.max(0, Math.min(100, progress.overallPercent)) : 100;
  // 平滑插值：消除高频进度事件造成的进度条抖动
  const percent = useSmoothProgress(targetPercent);

  return (
    <ModelDialog open={open} onClose={onClose} title="选择识别模型" width="lg">
      <p className="text-xs text-text-muted">
        内置识别模型：下载后即可设为当前；正在识别时切换会自动重启识别。
      </p>

      {voice.running && (
        <Alert variant="warning">
          <CircleAlert className="h-4 w-4" />
          <AlertDescription>语音会话运行中：切换将在下次语音会话启动时生效。</AlertDescription>
        </Alert>
      )}

      <div className="space-y-2">
        {ASR_PRESETS.map((p) => {
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
                <p className="flex items-center gap-2 text-sm font-medium text-text-primary">
                  {p.name}
                  <Badge variant="outline" className="shrink-0">
                    {asrModelKindLabel(p.kind)}
                  </Badge>
                </p>
                <p className="mt-0.5 text-xs text-text-muted">
                  {`${formatBytes(p.sizeBytes)} · ${p.tagline}`}
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
                      <Button
                        size="sm"
                        onClick={() => void handleSetCurrent(installed.id)}
                        disabled={busy || switchingId !== null}
                      >
                        {switchingId === installed.id && (
                          <Loader2 className="h-4 w-4 animate-spin" />
                        )}
                        {switchingId === installed.id ? "切换中…" : "设为当前"}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        className="shadow-none text-destructive hover:text-destructive"
                        onClick={() => setConfirmModel(installed)}
                        disabled={switchingId !== null}
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
