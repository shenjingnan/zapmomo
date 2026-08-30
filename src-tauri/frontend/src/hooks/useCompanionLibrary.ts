import { useCallback, useEffect, useState } from "react";
import { useToast } from "@/components/ui/toast";
import { api, onLive2dModelChanged } from "@/lib/tauri";
import type { CompanionLibraryView, CompanionModelInfo } from "@/types/tauri";

export interface CompanionLibraryState {
  /** 伙伴库快照（null = 尚未加载完成）。 */
  library: CompanionLibraryView | null;
  loading: boolean;
  error: string | null;
  refreshing: boolean;
  refresh: () => Promise<void>;
  /** 导入模型目录或 GIF 动图文件；返回导入（或已存在）的伙伴，供页面选中。 */
  importModel: (source: string) => Promise<CompanionModelInfo | null>;
  /** 设为当前使用。 */
  setActive: (id: string) => Promise<void>;
  /** 重命名伙伴（只改展示名）。 */
  rename: (id: string, name: string) => Promise<void>;
  /** 绑定/解绑伙伴音色（voiceId 传 null 解绑，回退目录自带或全局默认；voiceName 仅供 toast 展示）。 */
  setVoice: (id: string, voiceId: string | null, voiceName?: string) => Promise<void>;
  /** 移除伙伴（删除托管文件；若删的是 active 会自动落位/清空）。 */
  remove: (id: string) => Promise<void>;
  /** 在文件管理器中打开伙伴的托管资产目录（音色参考等可自行调整）。 */
  openAssetsDir: (id: string) => Promise<void>;
  /** 保存从 Live2D 渲染画布截取的封面 PNG，保存后返回更新后的库视图。 */
  saveCover: (id: string, png: number[]) => Promise<CompanionLibraryView | null>;
}

/**
 * 伙伴库状态（页面级 hook，对齐 `useModelLibrary`，不进全局 RuntimeContext）。
 *
 * active 的「全局持久化」由后端 `library.json` 承担，桌宠窗口通过
 * `get_live2d_config` / `live2d-model-changed` 读取，无需前端 Context 中转。
 */
export function useCompanionLibrary(): CompanionLibraryState {
  const toast = useToast();
  const [library, setLibrary] = useState<CompanionLibraryView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      setLibrary(await api.listCompanions());
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 托盘 / 右键菜单切换伙伴后同步「使用中」Badge：live2d-model-changed 只在 active
  // 真变化时发出（reconcile_active 幂等早退保证），不会造成刷新风暴。refresh 是稳定
  // useCallback（空依赖），本 effect 只运行一次；卸载时取消订阅。
  useEffect(() => {
    const unlisten = onLive2dModelChanged(() => {
      void refresh();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const importModel = useCallback(
    async (source: string): Promise<CompanionModelInfo | null> => {
      try {
        const result = await api.importCompanion({ source });
        setLibrary(result.library);
        const model = result.library.models.find((m) => m.id === result.model_id) ?? null;
        if (result.already_imported) {
          toast.warning("该伙伴已经导入");
        } else if (model) {
          toast.success(`✓ 已导入「${model.name}」`);
        }
        return model;
      } catch (e) {
        toast.error(String(e));
        return null;
      }
    },
    [toast],
  );

  const setActive = useCallback(
    async (id: string) => {
      try {
        const view = await api.setActiveCompanion({ id });
        setLibrary(view);
        const name = view.models.find((m) => m.id === id)?.name ?? id;
        toast.success(`✓ 「${name}」已设为当前使用`);
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  const rename = useCallback(
    async (id: string, name: string) => {
      try {
        const view = await api.renameCompanion({ id, name });
        setLibrary(view);
        toast.success(`✓ 已重命名为「${name}」`);
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  const remove = useCallback(
    async (id: string) => {
      const target = library?.models.find((m) => m.id === id);
      try {
        const view = await api.removeCompanion({ id });
        setLibrary(view);
        toast.success(`✓ 已移除「${target?.name ?? id}」`);
      } catch (e) {
        toast.error(String(e));
      }
    },
    [library, toast],
  );

  const setVoice = useCallback(
    async (id: string, voiceId: string | null, voiceName?: string) => {
      try {
        const view = await api.setCompanionVoice({ id, voiceId });
        setLibrary(view);
        const name = view.models.find((m) => m.id === id)?.name ?? id;
        toast.success(
          voiceId === null
            ? `✓ 「${name}」已恢复默认音色`
            : `✓ 「${name}」已绑定音色「${voiceName ?? voiceId}」`,
        );
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  const openAssetsDir = useCallback(
    async (id: string) => {
      try {
        await api.openCompanionDir({ id });
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  const saveCover = useCallback(
    async (id: string, png: number[]): Promise<CompanionLibraryView | null> => {
      try {
        const view = await api.saveCoverImage({ id, png });
        setLibrary(view);
        return view;
      } catch (e) {
        toast.error(`保存封面失败：${String(e)}`);
        return null;
      }
    },
    [toast],
  );

  return {
    library,
    loading,
    error,
    refreshing,
    refresh,
    importModel,
    setActive,
    rename,
    setVoice,
    remove,
    openAssetsDir,
    saveCover,
  };
}
