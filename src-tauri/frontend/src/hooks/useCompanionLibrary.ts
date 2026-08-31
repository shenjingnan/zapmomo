import { useCallback, useEffect, useState } from "react";
import { useToast } from "@/components/ui/toast";
import { api, onLive2dModelChanged } from "@/lib/tauri";
import type {
  CompanionLibraryView,
  CompanionModelInfo,
  ImportCompanionResult,
} from "@/types/tauri";

export interface CompanionLibraryState {
  /** 伙伴库快照（null = 尚未加载完成）。 */
  library: CompanionLibraryView | null;
  loading: boolean;
  error: string | null;
  refreshing: boolean;
  refresh: () => Promise<void>;
  /** 导入模型目录或 GIF 动图文件；返回导入（或已存在）的伙伴，供页面选中。 */
  importModel: (source: string) => Promise<CompanionModelInfo | null>;
  /** 从 .zip 导入角色包；返回导入（或已存在）的伙伴，供页面选中。 */
  importZip: (source: string) => Promise<CompanionModelInfo | null>;
  /** 导出角色包为 .zip（dest 为 save 对话框返回的完整路径）；返回是否成功。 */
  exportPack: (id: string, name: string, dest: string) => Promise<boolean>;
  /** 上传自定义音色覆盖当前生效音色（作者原版自动备份）；返回是否成功（成功才关对话框）。 */
  uploadVoice: (id: string, wavPath: string, referenceText: string) => Promise<boolean>;
  /** 恢复角色自带音色（删除当前上传版本，不可逆）。 */
  restoreVoice: (id: string) => Promise<void>;
  /** 设为当前使用。 */
  setActive: (id: string) => Promise<void>;
  /** 重命名伙伴（只改展示名）。 */
  rename: (id: string, name: string) => Promise<void>;
  /** 设置伙伴自定义唤醒词（wakeWord 传 null 恢复跟随角色名）。 */
  setWakeWord: (id: string, wakeWord: string | null) => Promise<void>;
  /** 设置伙伴自定义欢迎语（text 传 null 恢复默认模板；后台重生成预合成语音）。 */
  setWelcomeText: (id: string, text: string | null) => Promise<void>;
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

  /** 导入结果统一消费：更新库快照 + toast + 返回导入（或已存在）的伙伴。 */
  const applyImportResult = useCallback(
    async (result: ImportCompanionResult): Promise<CompanionModelInfo | null> => {
      setLibrary(result.library);
      const model = result.library.models.find((m) => m.id === result.model_id) ?? null;
      if (result.already_imported) {
        toast.warning("该伙伴已经导入");
      } else if (model) {
        toast.success(`✓ 已导入「${model.name}」`);
      }
      return model;
    },
    [toast],
  );

  const importModel = useCallback(
    async (source: string): Promise<CompanionModelInfo | null> => {
      try {
        const result = await api.importCompanion({ source });
        return await applyImportResult(result);
      } catch (e) {
        toast.error(String(e));
        return null;
      }
    },
    [toast, applyImportResult],
  );

  const importZip = useCallback(
    async (source: string): Promise<CompanionModelInfo | null> => {
      try {
        const result = await api.importCompanionZip({ source });
        return await applyImportResult(result);
      } catch (e) {
        toast.error(String(e));
        return null;
      }
    },
    [toast, applyImportResult],
  );

  const exportPack = useCallback(
    async (id: string, name: string, dest: string): Promise<boolean> => {
      try {
        const result = await api.exportCompanionPack({ id, dest });
        toast.success(`✓ 已导出「${name}」（${result.files} 个文件）`);
        return true;
      } catch (e) {
        toast.error(String(e));
        return false;
      }
    },
    [toast],
  );

  const uploadVoice = useCallback(
    async (id: string, wavPath: string, referenceText: string): Promise<boolean> => {
      try {
        const view = await api.uploadCompanionVoice({ id, wavPath, referenceText });
        setLibrary(view);
        const name = view.models.find((m) => m.id === id)?.name ?? id;
        toast.success(`✓ 「${name}」音色已更新（作者原版已备份）`);
        return true;
      } catch (e) {
        toast.error(String(e));
        return false;
      }
    },
    [toast],
  );

  const restoreVoice = useCallback(
    async (id: string) => {
      try {
        const view = await api.restoreCompanionVoice({ id });
        setLibrary(view);
        const name = view.models.find((m) => m.id === id)?.name ?? id;
        toast.success(`✓ 「${name}」已恢复角色自带音色`);
      } catch (e) {
        toast.error(String(e));
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

  const setWakeWord = useCallback(
    async (id: string, wakeWord: string | null) => {
      try {
        const view = await api.setCompanionWakeWord({ id, wakeWord });
        setLibrary(view);
        const name = view.models.find((m) => m.id === id)?.name ?? id;
        toast.success(
          wakeWord === null
            ? `✓ 「${name}」已恢复跟随角色名`
            : `✓ 「${name}」唤醒词已设为「${wakeWord}」`,
        );
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  const setWelcomeText = useCallback(
    async (id: string, text: string | null) => {
      try {
        const view = await api.setCompanionWelcomeText({ id, text });
        setLibrary(view);
        const name = view.models.find((m) => m.id === id)?.name ?? id;
        toast.success(
          text === null
            ? `✓ 「${name}」已恢复默认欢迎语，正在生成语音…`
            : `✓ 「${name}」欢迎语已保存，正在生成语音…`,
        );
      } catch (e) {
        toast.error(String(e));
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
    importZip,
    exportPack,
    uploadVoice,
    restoreVoice,
    setActive,
    rename,
    setWakeWord,
    setWelcomeText,
    remove,
    openAssetsDir,
    saveCover,
  };
}
