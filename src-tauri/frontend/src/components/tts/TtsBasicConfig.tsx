import { CircleAlert, FolderOpen, Mic, Settings2, Volume2 } from "lucide-react";
import { useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useRuntime } from "@/providers/RuntimeContext";
import { groupKokoroVoices, modelNameFromDir } from "./ttsMeta";

interface TtsBasicConfigProps {
  onTestOpen: () => void;
  onManageVoices: () => void;
  onOpenModelDialog: () => void;
}

/**
 * 基础配置（macOS 设置行）：
 * 当前模型（名称 + 就绪/未下载 Badge + 可展开完整路径）+
 * 底部「选择模型 / 测试语音 / 音色管理」操作按钮。
 */
export function TtsBasicConfig({
  onTestOpen,
  onManageVoices,
  onOpenModelDialog,
}: TtsBasicConfigProps) {
  const { tts } = useRuntime();
  const { config, configError, voices, selectedVoice, setSelectedVoice } = tts;
  const [showPath, setShowPath] = useState(false);

  const modelsPresent = config?.models_present ?? false;
  const enabled = config?.enabled ?? true;
  const modelPath = config?.model_dir ?? "";
  const modelName = modelNameFromDir(modelPath);
  // 音色语义按模型族三分：kokoro 选预置音色（103 个，分组下拉）；
  // vits/matcha/pocket 单说话人或固定音色（禁用占位）；zipvoice/omnivoice 走
  // 参考音频克隆（共享音色库与音色管理入口；omnivoice 无内置音色，未选时走
  // server auto voice）。
  const modelKind = config?.model_type ?? "";
  const kokoro = modelKind === "kokoro";
  const clone = modelKind === "zipvoice" || modelKind === "omnivoice";
  const sidFixed = !!modelKind && !clone && !kokoro;
  const voiceGroups = groupKokoroVoices(voices);

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <Settings2 className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">基础配置</h2>
            <p className="mt-0.5 text-xs text-text-muted">TTS 合成与声音设置</p>
          </div>
        </div>
      </div>

      {configError && (
        <div className="space-y-2 px-3.5 pb-2">
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">
              读取配置失败：{configError}
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
              模型文件缺失（{config.model_dir}）。点击下方「选择模型」重新下载或换用已安装模型。
            </AlertDescription>
          </Alert>
        </div>
      )}

      {config && !enabled && (
        <div className="px-3.5 pb-2">
          <Alert variant="warning">
            <CircleAlert className="h-4 w-4" />
            <AlertTitle>语音合成已关闭</AlertTitle>
            <AlertDescription className="whitespace-pre-wrap">
              语音合成已关闭，可在页面顶部开启后再测试。
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
              {config?.backend === "audiocpp" && (
                <Badge
                  variant="outline"
                  className="shrink-0 border-violet-500/20 bg-violet-500/10 text-violet-600"
                  title="由内置 audio.cpp 引擎（sidecar 进程）驱动"
                >
                  audio.cpp
                </Badge>
              )}
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
            </span>
            {showPath && modelPath && (
              <p className="mt-1 truncate font-mono text-xs text-text-muted" title={modelPath}>
                {modelPath}
              </p>
            )}
          </dd>
        </div>
      </dl>

      {/* 默认音色：kokoro 选预置音色（分组下拉，选即持久化 [tts].voice）；
          zipvoice/omnivoice 走克隆（共享音色库，所有合成默认用该音色）；
          vits/matcha/pocket 音色固定，仅显示禁用占位 */}
      <dl>
        <div className="flex items-center justify-between gap-3.5 border-t border-divider px-3.5 py-2.5">
          <dt className="shrink-0 text-sm text-text-primary">音色</dt>
          <dd className="min-w-0">
            {kokoro ? (
              <Select
                value={selectedVoice}
                onValueChange={(v) => void setSelectedVoice(v)}
                disabled={voices.length === 0}
              >
                <SelectTrigger id="tts-default-voice" aria-label="默认音色" className="h-8 w-48">
                  <SelectValue placeholder="默认（zf_001 · 中文女声）" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="">默认（zf_001 · 中文女声）</SelectItem>
                  {voiceGroups.map((g) => (
                    <SelectGroup key={g.group}>
                      <SelectLabel>{g.label}</SelectLabel>
                      {g.items.map((v) => (
                        <SelectItem key={v.id} value={v.id}>
                          {v.name}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  ))}
                </SelectContent>
              </Select>
            ) : sidFixed ? (
              <Select value="fixed" disabled>
                <SelectTrigger id="tts-default-voice" aria-label="默认音色" className="h-8 w-48">
                  <SelectValue placeholder="默认音色（模型固定）" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="fixed">默认音色（模型固定）</SelectItem>
                </SelectContent>
              </Select>
            ) : (
              <Select
                value={selectedVoice}
                onValueChange={(v) => void setSelectedVoice(v)}
                disabled={voices.length === 0 && modelKind === "zipvoice"}
              >
                <SelectTrigger id="tts-default-voice" aria-label="默认音色" className="h-8 w-48">
                  <SelectValue
                    placeholder={modelKind === "omnivoice" ? "默认（自动音色）" : "默认（内置 leijun）"}
                  />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="">
                    {modelKind === "omnivoice" ? "默认（自动音色）" : "默认（内置 leijun）"}
                  </SelectItem>
                  {voices.map((v) => (
                    <SelectItem key={v.id} value={v.id}>
                      {v.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </dd>
        </div>
      </dl>

      <div className="flex flex-wrap gap-2 border-t border-divider px-3.5 py-2.5">
        <Button onClick={onOpenModelDialog}>
          <FolderOpen className="h-4 w-4" />
          选择模型
        </Button>
        <Button
          variant="secondary"
          className="shadow-none"
          disabled={!modelsPresent}
          onClick={onTestOpen}
        >
          <Volume2 className="h-4 w-4" />
          测试语音
        </Button>
        {!kokoro && !sidFixed && (
          <Button variant="secondary" className="shadow-none" onClick={onManageVoices}>
            <Mic className="h-4 w-4" />
            音色管理
          </Button>
        )}
      </div>
    </section>
  );
}
