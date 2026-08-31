import { useEffect, useState } from "react";
import { api, onListenStarted, onListenStopped } from "@/lib/tauri";

export interface ListeningState {
  isListening: boolean;
  /** start/stop 在途标志：消除 command 已发到 isListening 落盘之间的重复点击窗口 */
  pending: boolean;
  error: string | null;
  start: (device: string | null, keywords: string | null) => Promise<void>;
  stop: () => Promise<void>;
}

/**
 * 监听状态管理：初始化时回读后端状态，订阅 `kws-stopped` 事件；
 * start/stop 包装对应 command 并同步 UI 状态与错误。
 */
export function useListening(): ListeningState {
  const [isListening, setIsListening] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .isListening()
      .then(setIsListening)
      .catch(() => {});

    const unlisten = onListenStopped((payload) => {
      setIsListening(false);
      // 干净停止（error 空）同时清掉陈旧错误：错误态只在真正出错时存续，
      // 否则一次历史错误会永远挂在模型页状态上误导（仅下次启动成功才清）
      setError(payload.error ?? null);
    });
    const unlistenStarted = onListenStarted(() => {
      setIsListening(true);
      setError(null);
    });

    return () => {
      unlisten.then((fn) => fn());
      unlistenStarted.then((fn) => fn());
    };
  }, []);

  const start = async (device: string | null, keywords: string | null) => {
    setPending(true);
    setError(null);
    try {
      await api.startListen({ device, keywords });
      setIsListening(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  };

  const stop = async () => {
    setPending(true);
    try {
      await api.stopListen();
      setIsListening(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  };

  return { isListening, pending, error, start, stop };
}
