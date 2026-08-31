import { CircleAlert, Download, FolderOpen, Repeat2, Settings2 } from "lucide-react";
import { useState } from "react";
import { DeviceSelect } from "@/components/DeviceSelect";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useRuntime } from "@/providers/RuntimeContext";
import { isDefaultKwsModelDir, modelNameFromDir } from "./kwsMeta";

interface KwsBasicConfigProps {
  /** 打开「选择唤醒词模型」弹窗（由 KwsPage 持有弹窗状态） */
  onSwitchOpen: () => void;
}

/**
 * 基础配置（macOS 设置行）：
 * 当前模型（名称 + 就绪/未下载 Badge + 切换模型 + 可展开完整路径）/ 麦克风来源 +
 * 底部「下载模型 / 选择模型」操作按钮。
 *
 * 全局自定义唤醒词输入与「测试唤醒词」入口已移除：唤醒词由伙伴在「伙伴」页
 * 按角色设置并在该处测试（角色词压过全局词，见 `companion::resolve_wake_word`）；
 * `[kws].custom_keywords` 保留为无伙伴接管时的回退（存量配置继续生效，仅无 UI 入口）。
 */
export function KwsBasicConfig({ onSwitchOpen }: KwsBasicConfigProps) {
  const {
    kws,
    devices: { error: devicesError },
  } = useRuntime();
  const { config, error } = kws.config;
  const { downloading, progress, error: downloadError, download } = kws.download;
  const [showPath, setShowPath] = useState(false);

  const modelsPresent = config?.models_present ?? false;
  const modelPath = config?.model_dir ?? "";
  const modelName = modelNameFromDir(modelPath);
  // 当前模型是否为默认 zh-en：只有它才允许用 legacy「下载模型」一键下载
  // （download_kws_model 固定装 zh-en；其他模型缺失时走「选择模型」弹窗）
  const isDefaultModel = isDefaultKwsModelDir(modelPath);

  const percent =
    progress?.stage === "downloading" ? Math.max(0, Math.min(100, progress.percent)) : 100;
  const busy = downloading || (config?.model_downloading ?? false);

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <Settings2 className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">基础配置</h2>
            <p className="mt-0.5 text-xs text-text-muted">KWS 检测与运行设置</p>
          </div>
        </div>
      </div>

      {error && (
        <div className="space-y-2 px-3.5 pb-2">
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">
              读取配置失败：{error}
            </AlertDescription>
          </Alert>
        </div>
      )}

      {config && !config.models_present && (
        <div className="px-3.5 pb-2">
          <Alert variant="warning">
            <CircleAlert className="h-4 w-4" />
            <AlertTitle>模型文件缺失</AlertTitle>
            <AlertDescription className="whitespace-pre-wrap">
              {isDefaultModel
                ? `模型文件缺失（${config.model_dir}）。点击下方「下载模型」按钮下载后即可开始监听。`
                : `当前模型文件缺失（${config.model_dir}）。点击下方「选择模型」换回已安装模型，或在弹窗中重新下载。`}
            </AlertDescription>
          </Alert>
        </div>
      )}

      <dl>
        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <dt className="shrink-0 text-sm text-text-primary">当前模型</dt>
          <dd className="min-w-0">
            <span className="flex items-center justify-end gap-1.5">
              {modelPath && (
                <button
                  type="button"
                  aria-label={showPath ? "隐藏模型路径" : "查看模型路径"}
                  title={modelPath}
                  onClick={() => setShowPath((v) => !v)}
                  className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-accent hover:text-text-primary"
                >
                  <FolderOpen className="h-3.5 w-3.5" />
                </button>
              )}
              <span
                className="truncate text-sm font-medium text-text-primary"
                title={modelName ?? undefined}
              >
                {modelName ?? "未知模型"}
              </span>
              <Badge
                variant="outline"
                className={
                  modelsPresent
                    ? "shrink-0 border-emerald-500/20 bg-emerald-500/10 text-emerald-600"
                    : "shrink-0 border-amber-500/20 bg-amber-500/10 text-amber-600"
                }
              >
                {modelsPresent ? "已就绪" : "未下载"}
              </Badge>
              <Button
                size="sm"
                variant="outline"
                className="shadow-none"
                onClick={onSwitchOpen}
                aria-label="切换唤醒词模型"
              >
                <Repeat2 className="h-3.5 w-3.5" />
                切换模型
              </Button>
            </span>
            {showPath && modelPath && (
              <p className="mt-1 truncate font-mono text-xs text-text-muted" title={modelPath}>
                {modelPath}
              </p>
            )}
          </dd>
        </div>

        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <div className="min-w-0">
            <dt className="text-sm text-text-primary">麦克风来源</dt>
            <dd className="mt-0.5 text-xs text-text-muted">用于检测唤醒词的麦克风输入源</dd>
          </div>
          <DeviceSelect />
        </div>
      </dl>

      {devicesError && (
        <div className="px-3.5 pb-2">
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">{devicesError}</AlertDescription>
          </Alert>
        </div>
      )}

      <div className="flex flex-wrap gap-2 border-t border-divider px-3.5 py-2.5">
        {!modelsPresent &&
          (isDefaultModel ? (
            <Button onClick={download} disabled={busy}>
              <Download className="h-4 w-4" />
              {busy ? "下载中…" : "下载模型"}
            </Button>
          ) : (
            <Button onClick={onSwitchOpen}>
              <Download className="h-4 w-4" />
              选择模型
            </Button>
          ))}
      </div>

      {progress && (
        <div className="space-y-1 px-3.5 pb-3">
          <Progress value={percent} />
          <p className="text-xs text-text-muted">{progress.message}</p>
        </div>
      )}

      {downloadError && (
        <div className="px-3.5 pb-3">
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">{downloadError}</AlertDescription>
          </Alert>
        </div>
      )}
    </section>
  );
}
