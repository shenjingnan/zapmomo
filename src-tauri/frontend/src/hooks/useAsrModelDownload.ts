import { useCallback, useEffect, useState } from "react";
import { api, onAsrDownloadProgress } from "@/lib/tauri";
import { useStorageGate } from "@/providers/StorageGateProvider";
import type { DownloadProgress } from "@/types/tauri";

export interface AsrModelDownloadState {
  downloading: boolean;
  progress: DownloadProgress | null;
  error: string | null;
  download: () => Promise<void>;
}

/**
 * ASR 模型下载：订阅进度事件，`download()` 触发下载，完成后回调 `onSuccess`（刷新配置）。
 */
export function useAsrModelDownload(onSuccess: () => void): AsrModelDownloadState {
  const gate = useStorageGate();
  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = onAsrDownloadProgress(setProgress);
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const download = useCallback(async () => {
    // 首次下载引导（选存储位置）；用户取消则静默中止
    if (!(await gate.ensureStorageReady())) return;
    setDownloading(true);
    setError(null);
    setProgress(null);
    try {
      await api.downloadAsrModel();
      onSuccess();
    } catch (e) {
      setError(String(e));
    } finally {
      setDownloading(false);
    }
  }, [gate, onSuccess]);

  return { downloading, progress, error, download };
}
