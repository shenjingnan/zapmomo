import { useCallback, useEffect, useState } from "react";
import { api, onSpeakerModelDownloadProgress } from "@/lib/tauri";
import type { DownloadProgress } from "@/types/tauri";

export interface SpeakerModelDownloadState {
  downloading: boolean;
  progress: DownloadProgress | null;
  error: string | null;
  download: () => Promise<void>;
}

/**
 * 声纹模型下载：订阅进度事件，`download()` 触发下载，完成后回调 `onSuccess`（刷新配置）。
 * raw 单文件资产：后端只发 downloading/verifying/done 三阶段，无 extracting。
 */
export function useSpeakerModelDownload(onSuccess: () => void): SpeakerModelDownloadState {
  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = onSpeakerModelDownloadProgress(setProgress);
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const download = useCallback(async () => {
    setDownloading(true);
    setError(null);
    setProgress(null);
    try {
      await api.downloadSpeakerModel();
      onSuccess();
    } catch (e) {
      setError(String(e));
    } finally {
      setDownloading(false);
    }
  }, [onSuccess]);

  return { downloading, progress, error, download };
}
