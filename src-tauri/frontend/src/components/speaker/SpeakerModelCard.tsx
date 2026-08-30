import { CircleAlert, Download, FolderOpen } from "lucide-react";
import { useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useRuntime } from "@/providers/RuntimeContext";

/** 基础配置卡：模型状态（就绪/未下载 + 一键下载 + 进度）与档案目录。 */
export function SpeakerModelCard() {
  const { speaker } = useRuntime();
  const { config, error } = speaker.config;
  const { downloading, progress, error: downloadError, download } = speaker.download;
  const [showPath, setShowPath] = useState(false);

  const modelPresent = config?.model_present ?? false;
  const percent =
    progress?.stage === "downloading" ? Math.max(0, Math.min(100, progress.percent)) : 100;
  const busy = downloading || (config?.model_downloading ?? false);
  const message = progress?.stage === "done" ? "模型安装完成" : (progress?.message ?? "下载中…");

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <FolderOpen className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">声纹模型</h2>
            <p className="mt-0.5 text-xs text-text-muted">
              3D-Speaker CAM++（中文 16k，约 27MB，本地运行）
            </p>
          </div>
        </div>
      </div>

      {(error || downloadError) && (
        <div className="space-y-2 px-3.5 pb-2">
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">
              {downloadError ?? `读取配置失败：${error}`}
            </AlertDescription>
          </Alert>
        </div>
      )}

      {config && !modelPresent && !busy && (
        <div className="px-3.5 pb-2">
          <Alert variant="warning">
            <CircleAlert className="h-4 w-4" />
            <AlertTitle>模型未下载</AlertTitle>
            <AlertDescription className="whitespace-pre-wrap">
              点击下方「下载模型」获取声纹模型，下载后即可注册与识别。
            </AlertDescription>
          </Alert>
        </div>
      )}

      <dl>
        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <dt className="shrink-0 text-sm text-text-primary">模型状态</dt>
          <dd className="min-w-0">
            <span className="flex items-center justify-end gap-1.5">
              <Badge
                className={
                  modelPresent ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700"
                }
              >
                {modelPresent ? "已就绪" : busy ? "下载中" : "未下载"}
              </Badge>
              {config?.model_dir && (
                <button
                  type="button"
                  aria-label={showPath ? "隐藏模型目录" : "查看模型目录"}
                  className="truncate font-mono text-xs text-text-muted underline-offset-2 hover:underline"
                  onClick={() => setShowPath((v) => !v)}
                >
                  {showPath ? config.model_dir : "…"}
                </button>
              )}
            </span>
          </dd>
        </div>
        {showPath && config?.speaker_profiles_dir && (
          <div className="flex items-center justify-between gap-3.5 px-3.5 pb-2.5">
            <dt className="shrink-0 text-sm text-text-primary">声纹档案目录</dt>
            <dd className="truncate font-mono text-xs text-text-muted">
              {config.speaker_profiles_dir}
            </dd>
          </div>
        )}
      </dl>

      {busy && (
        <div className="space-y-1.5 px-3.5 pb-3">
          <Progress value={percent} />
          <p className="text-xs text-text-muted">{message}</p>
        </div>
      )}

      <div className="flex items-center justify-end gap-2 border-t border-divider px-3.5 py-2.5">
        <Button size="sm" variant="outline" onClick={() => void download()} disabled={busy}>
          <Download className="h-4 w-4" />
          {modelPresent ? "重新下载" : "下载模型"}
        </Button>
      </div>
    </section>
  );
}
