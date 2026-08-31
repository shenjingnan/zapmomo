import { open } from "@tauri-apps/plugin-dialog";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import { StoragePromptDialog } from "@/components/storage/StoragePromptDialog";
import { useToast } from "@/components/ui/toast";
import { api, onStorageDirChanged } from "@/lib/tauri";
import type { StoragePrompt } from "@/types/modelLibrary";

export interface StorageGateApi {
  /**
   * 下载/导入前置检查：首次下载/导入前弹「选择模型存储位置」引导窗。
   * 返回 `false` = 用户取消，调用方应静默中止（不提示错误、不置忙碌态）。
   */
  ensureStorageReady: () => Promise<boolean>;
}

/** 默认放行：无 Provider 环境（纯页面测试等）不拦截。 */
const StorageGateContext = createContext<StorageGateApi>({
  ensureStorageReady: async () => true,
});

export function useStorageGate(): StorageGateApi {
  return useContext(StorageGateContext);
}

interface PendingPrompt {
  info: StoragePrompt;
  resolve: (proceed: boolean) => void;
}

/**
 * 存储位置引导 Provider：挂载在 ToastProvider 内（用 useToast）、
 * AppRuntimeProvider 外（下载/导入 hooks 调 useStorageGate）。
 *
 * 完全惰性：挂载不发任何请求，只在首个下载/导入前查一次 `get_storage_prompt`。
 * `readyRef` 做会话内缓存；`inflightRef` 并发去重（同时触发多个下载只弹一次）；
 * `storage-dir-changed`（set_data_dir / 迁移完成）后缓存失效重查。
 */
export function StorageGateProvider({ children }: { children: ReactNode }) {
  const toast = useToast();
  const readyRef = useRef(false);
  const inflightRef = useRef<Promise<boolean> | null>(null);
  const [pending, setPending] = useState<PendingPrompt | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const unlisten = onStorageDirChanged(() => {
      readyRef.current = false;
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const ensureStorageReady = useCallback(async (): Promise<boolean> => {
    if (readyRef.current) return true;
    if (inflightRef.current) return inflightRef.current;
    const p = (async () => {
      try {
        // `!info?.promptRecommended` 裸判（勿改 info.promptRecommended）：
        // invoke mock / 异常返回 undefined 时 fail-open 放行
        const info = await api.getStoragePrompt();
        if (!info?.promptRecommended) {
          readyRef.current = true;
          return true;
        }
        return await new Promise<boolean>((resolve) => setPending({ info, resolve }));
      } catch {
        return true; // 查询失败不阻塞下载（后端仍有空间校验兜底）
      } finally {
        inflightRef.current = null;
      }
    })();
    inflightRef.current = p;
    return p;
  }, []);

  /** 关窗并放行/中止；放行时置会话缓存。 */
  const settle = useCallback(
    (proceed: boolean) => {
      pending?.resolve(proceed);
      setPending(null);
      if (proceed) readyRef.current = true;
    },
    [pending],
  );

  const handleUseDefault = async () => {
    try {
      await api.acknowledgeStoragePrompt();
    } catch (e) {
      // 标记写入失败不影响本次放行，下次会再问一次
      toast.error(String(e));
    }
    settle(true);
  };

  const handlePickDir = async () => {
    const dir = await open({ directory: true, multiple: false, title: "选择模型存储位置" });
    if (typeof dir !== "string" || !dir) return; // 关掉选择器 → 留在引导窗
    setBusy(true);
    try {
      await api.setStorageDir({ path: dir });
      await api.acknowledgeStoragePrompt();
      settle(true);
    } catch (e) {
      // set_data_dir 校验失败（嵌套/被占用等）：留在弹窗内可重选或取消
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <StorageGateContext.Provider value={{ ensureStorageReady }}>
      {children}
      {pending && (
        <StoragePromptDialog
          info={pending.info}
          busy={busy}
          onUseDefault={() => void handleUseDefault()}
          onPickDir={() => void handlePickDir()}
          onCancel={() => settle(false)}
        />
      )}
    </StorageGateContext.Provider>
  );
}
