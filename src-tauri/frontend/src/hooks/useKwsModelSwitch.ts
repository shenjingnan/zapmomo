import { useCallback, useEffect, useRef, useState } from "react";
import { useToast } from "@/components/ui/toast";
import { api, onModelLibraryDownloadProgress } from "@/lib/tauri";
import { useRuntime } from "@/providers/RuntimeContext";
import { useStorageGate } from "@/providers/StorageGateProvider";
import type { LibraryModel, ModelLibraryProgress, SetCurrentResult } from "@/types/modelLibrary";

/** 默认（legacy 一键下载按钮所装）的 zh-en 模型 registry id。 */
export const KWS_DEFAULT_PRESET_ID = "kws-zipformer-zh-en-3m";

/** KWS 切换弹窗的内置预设（id = models/model_registry.json 的 registry id）。 */
export const KWS_PRESETS = [
  {
    id: "kws-zipformer-zh-en-3m",
    name: "Zipformer KWS zh-en 3M",
    tagline: "中英混合 · 支持中英文唤醒词",
    sizeBytes: 32_885_699,
  },
] as const;

export interface KwsModelSwitchState {
  /** `list_model_library` 快照（含安装 / current 状态）；null = 尚未加载 */
  models: LibraryModel[] | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  /** 下载 registry 模型（model-library-download-progress 进度） */
  download: (id: string) => Promise<void>;
  downloadingId: string | null;
  progress: ModelLibraryProgress | null;
  /** 设为当前模型；监听中切换后自动 stop → start 重启监听使新模型立即生效 */
  setCurrent: (id: string) => Promise<void>;
  /** 卸载（managed 删文件；当前/运行中模型后端会拒绝） */
  remove: (id: string) => Promise<void>;
}

/**
 * KWS 模型切换状态：从后端模型列表过滤 KWS 条目，提供下载 / 设为当前 / 卸载。
 * 数据用 `list_model_library`（后端模型列表真相源，含 install_state + current）。
 */
export function useKwsModelSwitch(): KwsModelSwitchState {
  const runtime = useRuntime();
  const toast = useToast();
  const gate = useStorageGate();
  const [models, setModels] = useState<LibraryModel[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [downloadingId, setDownloadingId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ModelLibraryProgress | null>(null);
  /** 下载终态（done/cancelled/failed）：await 返回时事件可能尚未到达，用 ref 透传 */
  const terminalStage = useRef<string | null>(null);

  // setCurrent 的 await 期间 runtime 可能变化（重启监听需读最新 device/keywords/isListening）
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
      // 首次下载引导（选存储位置）；用户取消则静默中止（不置忙碌态）
      if (!(await gate.ensureStorageReady())) return;
      setDownloadingId(id);
      setProgress(null);
      terminalStage.current = null;
      try {
        await api.downloadLibraryModel({ id });
        const stage = terminalStage.current;
        if (stage === "cancelled") {
          toast.warning("已取消下载");
        } else {
          const name = KWS_PRESETS.find((p) => p.id === id)?.name ?? id;
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
    [gate, toast, refresh],
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
      await Promise.allSettled([runtimeRef.current.kws.config.refresh(), refresh()]);
      // 后端只写配置（restart_required）：监听中切换由前端重启监听使新模型立即生效
      // （与高级参数保存后的重启同款模式）
      const kws = runtimeRef.current.kws;
      if (res.runtimeAction === "restart_required" && kws.listening.isListening) {
        await kws.listening.stop();
        await kws.listening.start(
          runtimeRef.current.device || null,
          runtimeRef.current.sessionKeywords || null,
        );
        if (kws.listening.error) {
          toast.error(`模型已切换，但重启监听失败：${kws.listening.error}`);
        } else {
          toast.success("已切换模型并重启监听");
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
      await Promise.allSettled([runtimeRef.current.kws.config.refresh(), refresh()]);
    },
    [toast, refresh],
  );

  return { models, loading, error, refresh, download, downloadingId, progress, setCurrent, remove };
}
