import { useCallback, useEffect, useRef, useState } from "react";
import { useToast } from "@/components/ui/toast";
import { api, onModelLibraryDownloadProgress } from "@/lib/tauri";
import { useRuntime } from "@/providers/RuntimeContext";
import type { LibraryModel, ModelLibraryProgress, SetCurrentResult } from "@/types/modelLibrary";

/** ASR 切换弹窗的内置预设（id = models/model_registry.json 的 registry id）。 */
export const ASR_PRESETS = [
  {
    id: "asr-streaming-bilingual-zh-en",
    name: "Streaming Zipformer ASR zh-en",
    tagline: "中英双语 · 流式转写（默认）",
    sizeBytes: 511_274_346,
    kind: "zipformer",
  },
  {
    id: "asr-streaming-small-bilingual-zh-en",
    name: "Streaming Zipformer ASR zh-en (small)",
    tagline: "中英双语 · 轻量版",
    sizeBytes: 458_187_351,
    kind: "zipformer",
  },
  {
    id: "asr-streaming-zh-14m",
    name: "Streaming Zipformer ASR zh 14M",
    tagline: "纯中文 · 超轻量 14M",
    sizeBytes: 74_004_050,
    kind: "zipformer",
  },
  {
    id: "asr-streaming-en-20m",
    name: "Streaming Zipformer ASR en 20M",
    tagline: "纯英文 · 轻量 20M · 不支持中文",
    sizeBytes: 127_887_156,
    kind: "zipformer",
  },
  {
    id: "asr-streaming-en-2023-06-21",
    name: "Streaming Zipformer ASR en 2023-06-21",
    tagline: "纯英文 · 不支持中文",
    sizeBytes: 506_956_414,
    kind: "zipformer",
  },
  {
    id: "asr-streaming-en-2023-02-21",
    name: "Streaming Zipformer ASR en 2023-02-21",
    tagline: "纯英文 · 不支持中文",
    sizeBytes: 397_939_030,
    kind: "zipformer",
  },
  {
    id: "asr-paraformer-bilingual-zh-en",
    name: "Streaming Paraformer ASR zh-en",
    tagline: "中英双语 · 流式 · 高准确率 · 包体约 1GB",
    sizeBytes: 1_047_319_737,
    kind: "paraformer",
  },
  {
    id: "asr-paraformer-trilingual-zh-cantonese-en",
    name: "Streaming Paraformer ASR zh-yue-en",
    tagline: "中粤英三语 · 流式 · 包体约 1GB",
    sizeBytes: 1_047_671_211,
    kind: "paraformer",
  },
  {
    id: "asr-sensevoice-zh-en-ja-ko-yue",
    name: "SenseVoice ASR (int8)",
    tagline: "中英日韩粤 · 离线整段转写 · 情绪/事件标签",
    sizeBytes: 163_002_883,
    kind: "sensevoice",
  },
  {
    id: "asr-whisper-tiny",
    name: "Whisper ASR tiny",
    tagline: "多语言 · 离线整段转写 · 轻量",
    sizeBytes: 116_204_861,
    kind: "whisper",
  },
  {
    id: "asr-whisper-base",
    name: "Whisper ASR base",
    tagline: "多语言 · 离线整段转写 · 更高准确率",
    sizeBytes: 207_557_382,
    kind: "whisper",
  },
  {
    id: "asr-qwen3-0.6b",
    name: "Qwen3-ASR 0.6B (int8)",
    tagline: "29 语言自动识别 · 离线整段转写 · 支持热词 · 包体约 840MB",
    sizeBytes: 878_702_423,
    kind: "qwen3_asr",
  },
  {
    id: "asr-qwen3-0.6b-audiocpp",
    name: "Qwen3-ASR 0.6B (audio.cpp)",
    tagline: "29 语言自动识别 · Metal 加速 · 不支持热词 · 包体约 1.1GB",
    sizeBytes: 1_151_272_416,
    kind: "qwen3_asr",
  },
] as const;

export interface AsrModelSwitchState {
  /** `list_model_library` 快照（含安装 / current 状态）；null = 尚未加载 */
  models: LibraryModel[] | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  /** 下载 registry 模型（model-library-download-progress 进度） */
  download: (id: string) => Promise<void>;
  downloadingId: string | null;
  progress: ModelLibraryProgress | null;
  /** 设为当前模型；识别中切换后自动 stop → start 重启识别使新模型立即生效 */
  setCurrent: (id: string) => Promise<void>;
  /** 卸载（managed 删文件；当前/运行中模型后端会拒绝） */
  remove: (id: string) => Promise<void>;
}

/**
 * ASR 模型切换状态：从模型库列表过滤 ASR 条目，提供下载 / 设为当前 / 卸载。
 * 数据用 `list_model_library`（与模型库页同一后端真相源，含 install_state + current）。
 */
export function useAsrModelSwitch(): AsrModelSwitchState {
  const runtime = useRuntime();
  const toast = useToast();
  const [models, setModels] = useState<LibraryModel[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [downloadingId, setDownloadingId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ModelLibraryProgress | null>(null);
  /** 下载终态（done/cancelled/failed）：await 返回时事件可能尚未到达，用 ref 透传 */
  const terminalStage = useRef<string | null>(null);

  // setCurrent 的 await 期间 runtime 可能变化（重启识别需读最新 device/isListening）
  const runtimeRef = useRef(runtime);
  runtimeRef.current = runtime;

  const refresh = useCallback(async () => {
    try {
      setModels(await api.listModelLibrary());
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const unlisten = onModelLibraryDownloadProgress((p) => {
      setProgress(p);
      if (p.stage === "done" || p.stage === "cancelled" || p.stage === "failed") {
        terminalStage.current = p.stage;
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const download = useCallback(
    async (id: string) => {
      setDownloadingId(id);
      setProgress(null);
      terminalStage.current = null;
      try {
        await api.downloadLibraryModel({ id });
        const stage = terminalStage.current;
        if (stage === "cancelled") {
          toast.warning("已取消下载");
        } else {
          const name = ASR_PRESETS.find((p) => p.id === id)?.name ?? id;
          toast.success(`✓ ${name} 下载完成`);
        }
      } catch (e) {
        toast.error(`模型下载失败：${String(e)}`);
      } finally {
        setDownloadingId(null);
        setProgress(null);
        terminalStage.current = null;
        await refresh();
      }
    },
    [toast, refresh],
  );

  const setCurrent = useCallback(
    async (id: string) => {
      let res: SetCurrentResult;
      try {
        res = await api.setCurrentModel({ id });
      } catch (e) {
        toast.error(String(e));
        return;
      }
      await Promise.allSettled([runtimeRef.current.asr.config.refresh(), refresh()]);
      // 后端只写配置（restart_required）：识别中切换由前端重启识别使新模型立即生效
      // （与高级参数保存后的重启同款模式）
      const asr = runtimeRef.current.asr;
      if (res.runtimeAction === "restart_required" && asr.listening.isListening) {
        await asr.listening.stop();
        await asr.listening.start(runtimeRef.current.device || null);
        if (asr.listening.error) {
          toast.error(`模型已切换，但重启识别失败：${asr.listening.error}`);
        } else {
          toast.success("已切换模型并重启识别");
        }
      } else {
        toast.success(res.message);
      }
    },
    [toast, refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      try {
        await api.deleteModel({ id });
        toast.success("✓ 模型已卸载");
      } catch (e) {
        toast.error(String(e));
        return;
      }
      await Promise.allSettled([runtimeRef.current.asr.config.refresh(), refresh()]);
    },
    [toast, refresh],
  );

  return { models, loading, error, refresh, download, downloadingId, progress, setCurrent, remove };
}
