import { useCallback, useEffect, useRef, useState } from "react";
import { useToast } from "@/components/ui/toast";
import { api, onModelLibraryDownloadProgress } from "@/lib/tauri";
import { useRuntime } from "@/providers/RuntimeContext";
import type { LibraryModel, ModelLibraryProgress, SetCurrentResult } from "@/types/modelLibrary";

/** TTS 切换弹窗的内置预设（id = models/model_registry.json 的 registry id）。 */
export const TTS_PRESETS = [
  {
    id: "tts-zipvoice-distill-int8",
    name: "ZipVoice TTS zh-en",
    kind: "zipvoice",
    languages: "中英",
    tagline: "零样本声音克隆 · 中英双语 · 含声码器",
    sizeBytes: 163_320_194,
  },
  {
    id: "tts-vits-melo-zh-en",
    name: "VITS Melo TTS zh-en",
    kind: "vits",
    languages: "中英",
    tagline: "单说话人 · 中英双语 · 44.1kHz",
    sizeBytes: 167_006_755,
  },
  {
    id: "tts-matcha-zh-baker",
    name: "Matcha Icefall zh-baker",
    kind: "matcha",
    languages: "中文",
    tagline: "中文女声 · 22.05kHz",
    sizeBytes: 75_463_442,
  },
  {
    id: "tts-kokoro-int8-multi-lang-v1-1",
    name: "Kokoro TTS 中英 int8",
    kind: "kokoro",
    languages: "中英",
    tagline: "103 音色 · int8 量化 · 24kHz",
    sizeBytes: 147_031_220,
  },
  {
    id: "tts-kokoro-multi-lang-v1-1",
    name: "Kokoro TTS 中英",
    kind: "kokoro",
    languages: "中英",
    tagline: "103 音色 · 高音质 fp32 · 24kHz",
    sizeBytes: 364_816_464,
  },
  {
    id: "tts-pocket-english-audiocpp",
    name: "PocketTTS English",
    kind: "pocket",
    languages: "英文",
    tagline: "audio.cpp 引擎 · 固定音色 alba · 24kHz",
    sizeBytes: 127_856_704,
  },
  {
    id: "tts-omnivoice-q8-audiocpp",
    name: "OmniVoice 多语种克隆",
    kind: "omnivoice",
    languages: "多语种",
    tagline: "audio.cpp 引擎 · 声音克隆 · 600+ 语种 · 24kHz · 仅 Apple Silicon（Metal）",
    sizeBytes: 1_350_288_416,
  },
  {
    id: "tts-voxcpm2-q8-audiocpp",
    name: "VoxCPM2 高保真克隆",
    kind: "voxcpm2",
    languages: "多语种",
    tagline: "audio.cpp 引擎 · 帧级流式 · 48kHz 录音室级 · 30 语种 · 仅 Apple Silicon（Metal）· 建议 16GB+ 内存",
    sizeBytes: 2_955_000_480,
  },
] as const;

export interface TtsModelSwitchState {
  /** `list_model_library` 快照（含安装 / current 状态）；null = 尚未加载 */
  models: LibraryModel[] | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  /** 下载 registry 模型（model-library-download-progress 进度） */
  download: (id: string) => Promise<void>;
  downloadingId: string | null;
  progress: ModelLibraryProgress | null;
  /** 设为当前模型；TTS 每次合成现场建引擎，写完配置即生效（下次合成用新模型） */
  setCurrent: (id: string) => Promise<void>;
  /** 卸载（managed 删文件；当前/下载中模型后端会拒绝） */
  remove: (id: string) => Promise<void>;
}

/**
 * TTS 模型切换状态：从模型库列表过滤 TTS 条目，提供下载 / 设为当前 / 卸载。
 * 数据用 `list_model_library`（与模型库页同一后端真相源，含 install_state + current）。
 * 与 ASR 版差异：TTS 无监听概念，切换不需要重启任何 runtime。
 */
export function useTtsModelSwitch(): TtsModelSwitchState {
  const runtime = useRuntime();
  const toast = useToast();
  const [models, setModels] = useState<LibraryModel[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [downloadingId, setDownloadingId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ModelLibraryProgress | null>(null);
  /** 下载终态（done/cancelled/failed）：await 返回时事件可能尚未到达，用 ref 透传 */
  const terminalStage = useRef<string | null>(null);

  // setCurrent 的 await 期间 runtime 可能变化（刷新配置需读最新 tts 切片）
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
          const name = TTS_PRESETS.find((p) => p.id === id)?.name ?? id;
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
      // 刷新 TTS 配置（当前模型名/就绪状态）与模型库列表；后端只写配置即生效
      await Promise.allSettled([runtimeRef.current.tts.refreshConfig(), refresh()]);
      toast.success(res.message);
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
      await Promise.allSettled([runtimeRef.current.tts.refreshConfig(), refresh()]);
    },
    [toast, refresh],
  );

  return { models, loading, error, refresh, download, downloadingId, progress, setCurrent, remove };
}
