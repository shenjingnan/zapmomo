import { open } from "@tauri-apps/plugin-dialog";
import { CircleAlert, FolderOpen, HardDrive, Settings2 } from "lucide-react";
import { useEffect, useState } from "react";
import { DeviceSelect } from "@/components/DeviceSelect";
import { ModelDialog } from "@/components/models/ModelDialog";
import { CompanionWindowSection } from "@/components/settings/CompanionWindowSection";
import { ShortcutsSection } from "@/components/settings/ShortcutsSection";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { useToast } from "@/components/ui/toast";
import { api, onAutostartChanged, onStorageMigrateProgress } from "@/lib/tauri";
import { formatBytes } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import type { StorageInfo, StorageMigrateProgress } from "@/types/modelLibrary";

/**
 * 设置页：通用设置（麦克风来源 / 隐藏 Dock 图标）+ 存储位置（数据目录）。
 * 存储位置支持自定义数据目录（新下载走新目录、存量双根可见）与「迁移已有模型释放空间」。
 */
export function SettingsPage() {
  const {
    devices: { error: devicesError },
  } = useRuntime();
  const toast = useToast();
  const [hideDockIcon, setHideDockIcon] = useState<boolean | null>(null);
  const [autostart, setAutostart] = useState(false);

  // 存储位置（数据目录）
  const [storageInfo, setStorageInfo] = useState<StorageInfo | null>(null);
  const [storageLoading, setStorageLoading] = useState(true);
  const [dirDialogOpen, setDirDialogOpen] = useState(false);
  const [migrateDialogOpen, setMigrateDialogOpen] = useState(false);
  const [migrateProgress, setMigrateProgress] = useState<StorageMigrateProgress | null>(null);

  const refreshStorage = async () => {
    try {
      setStorageInfo(await api.getStorageInfo());
    } catch (e) {
      toast.error(String(e));
    } finally {
      setStorageLoading(false);
    }
  };

  useEffect(() => {
    void api
      .getHideDockIcon()
      .then(setHideDockIcon)
      .catch(() => setHideDockIcon(false));
  }, []);

  // 开机自启动：系统注册状态直读（读失败或旧后端未返回时回退关闭）
  useEffect(() => {
    void api
      .getAutostart()
      .then((v) => setAutostart(Boolean(v)))
      .catch(() => setAutostart(false));
  }, []);

  // 托盘菜单切换自启动后同步开关
  useEffect(() => {
    const unlistenPromise = onAutostartChanged(setAutostart);
    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: 挂载时一次性订阅；refreshStorage/toast 引用仅作调用，不参与依赖
  useEffect(() => {
    void refreshStorage();
    // 迁移进度事件：state 终态时 toast + 刷新，其余更新进度条
    const unlistenPromise = onStorageMigrateProgress((p) => {
      setMigrateProgress(p);
      if (p.state === "done") {
        setMigrateDialogOpen(false);
        void refreshStorage();
        toast.success(
          p.failedItems.length > 0
            ? `迁移完成（${p.failedItems.length} 项失败：${p.failedItems
                .map((f) => f.name)
                .join("、")}）`
            : "迁移完成",
        );
      } else if (p.state === "cancelled") {
        setMigrateDialogOpen(false);
        void refreshStorage();
        toast.warning("已取消迁移");
      } else if (p.state === "failed") {
        setMigrateDialogOpen(false);
        void refreshStorage();
        toast.error("迁移失败");
      }
    });
    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  // 立即应用并持久化；失败时回滚到原值。
  const handleToggle = (hide: boolean) => {
    setHideDockIcon(hide);
    void api.setHideDockIcon({ hide }).catch(() => setHideDockIcon((prev) => !prev));
  };

  // 同款乐观更新；写入系统启动项失败（如组策略禁写）时回滚
  const handleToggleAutostart = (enabled: boolean) => {
    setAutostart(enabled);
    void api.setAutostart({ enabled }).catch(() => setAutostart((prev) => !prev));
  };

  // ---- 存储位置交互 ----

  const changeStorageDir = async () => {
    setDirDialogOpen(false);
    const dir = await open({ directory: true, multiple: false, title: "选择数据目录" });
    if (typeof dir !== "string" || !dir) return;
    try {
      await api.setStorageDir({ path: dir });
      toast.success("数据目录已更新");
      await refreshStorage();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const resetStorageDir = async () => {
    try {
      await api.setStorageDir({ path: null });
      toast.success("已恢复默认目录");
      await refreshStorage();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const startMigration = async () => {
    setMigrateDialogOpen(false);
    try {
      await api.migrateStorage();
    } catch (e) {
      toast.error(String(e));
      await refreshStorage();
    }
  };

  const migrating = migrateProgress !== null;
  const storageBusy = migrating || storageLoading;

  return (
    <div className="space-y-4 pb-4">
      <h1 className="text-2xl font-semibold tracking-tight text-text-primary">设置</h1>

      <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
        <div className="px-3.5 py-2.5">
          <div className="flex items-center gap-2.5">
            <Settings2 className="h-4 w-4 shrink-0 text-text-secondary" />
            <div>
              <h2 className="text-base font-semibold text-text-primary">通用</h2>
              <p className="mt-0.5 text-xs text-text-muted">应用行为与系统集成</p>
            </div>
          </div>
        </div>

        <dl className="divide-y divide-divider">
          <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
            <div className="min-w-0">
              <dt className="text-sm text-text-primary">麦克风来源</dt>
              <dd className="mt-0.5 text-xs text-text-muted">
                用于唤醒词检测与语音识别的输入设备，选择后全局生效并被记忆。
              </dd>
            </div>
            <DeviceSelect />
          </div>

          <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
            <div className="min-w-0">
              <dt className="text-sm text-text-primary">隐藏应用图标</dt>
              <dd className="mt-0.5 text-xs text-text-muted">
                在 Dock / Cmd+Tab 中隐藏应用图标（仅 macOS）
              </dd>
            </div>
            <Switch
              aria-label="隐藏应用图标"
              checked={hideDockIcon ?? false}
              onCheckedChange={handleToggle}
            />
          </div>

          <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
            <div className="min-w-0">
              <dt className="text-sm text-text-primary">开机自启动</dt>
              <dd className="mt-0.5 text-xs text-text-muted">
                登录系统后自动启动 ZapMomo，桌宠静默出现
              </dd>
            </div>
            <Switch
              aria-label="开机自启动"
              checked={autostart}
              onCheckedChange={handleToggleAutostart}
            />
          </div>

          <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
            <div className="min-w-0">
              <dt className="text-sm text-text-primary">重启应用</dt>
              <dd className="mt-0.5 text-xs text-text-muted">关闭并重新启动 ZapMomo</dd>
            </div>
            <Button size="sm" onClick={() => void api.restartApp()}>
              重启
            </Button>
          </div>
        </dl>

        {devicesError && (
          <div className="px-3.5 pb-2">
            <Alert variant="destructive">
              <CircleAlert className="h-4 w-4" />
              <AlertDescription className="whitespace-pre-wrap">{devicesError}</AlertDescription>
            </Alert>
          </div>
        )}
      </section>

      {/* 伙伴窗口（层级 / 点击穿透 / 锁定 / 修饰键拖动，全局生效） */}
      <CompanionWindowSection />

      {/* 存储位置（数据目录） */}
      <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
        <div className="px-3.5 py-2.5">
          <div className="flex items-center gap-2.5">
            <HardDrive className="h-4 w-4 shrink-0 text-text-secondary" />
            <div>
              <h2 className="text-base font-semibold text-text-primary">存储位置</h2>
              <p className="mt-0.5 text-xs text-text-muted">
                自定义模型与伙伴模型的存放目录；切换后新下载走新目录，已有模型保持可用。 settings
                与日志等小文件仍保留在 ~/.zapmomo
              </p>
            </div>
          </div>
        </div>

        <dl className="divide-y divide-divider">
          <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
            <div className="min-w-0">
              <dt className="text-sm text-text-primary">数据目录（模型 / 伙伴）</dt>
              <dd className="mt-0.5 space-y-0.5 break-all text-xs text-text-muted">
                <div>
                  {storageInfo?.modelsDir ?? (storageLoading ? "加载中…" : "~/.zapmomo/models")}
                </div>
                <div>{storageInfo?.companionsDir ?? "~/.zapmomo/companions"}</div>
              </dd>
            </div>
            <div className="flex shrink-0 items-center gap-1.5">
              <Button
                size="sm"
                variant="ghost"
                onClick={() => void api.openStorageDir()}
                disabled={storageBusy}
              >
                <FolderOpen className="h-3.5 w-3.5" />
                打开目录
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => setDirDialogOpen(true)}
                disabled={storageBusy}
              >
                更改
              </Button>
              {storageInfo?.dataDir && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => void resetStorageDir()}
                  disabled={storageBusy}
                >
                  恢复默认
                </Button>
              )}
            </div>
          </div>

          {storageInfo?.migrationAvailable && !migrating && (
            <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
              <div className="min-w-0">
                <dt className="text-sm text-text-primary">迁移已有模型</dt>
                <dd className="mt-0.5 text-xs text-text-muted">
                  旧目录占用{" "}
                  {formatBytes(
                    (storageInfo.legacyModelsBytes ?? 0) + (storageInfo.legacyCompanionsBytes ?? 0),
                  )}
                  ，迁移后释放空间（跨盘复制可能耗时较长）
                </dd>
              </div>
              <Button size="sm" onClick={() => setMigrateDialogOpen(true)}>
                开始迁移
              </Button>
            </div>
          )}

          {migrating && migrateProgress && (
            <div className="space-y-2 px-3.5 py-2.5">
              <div className="flex items-center justify-between text-xs text-text-muted">
                <span>{migrateProgress.message}</span>
                <span>
                  {migrateProgress.itemsDone}/{migrateProgress.itemsTotal} 项 ·{" "}
                  {formatBytes(migrateProgress.bytesDone)}
                  {migrateProgress.bytesTotal > 0
                    ? ` / ${formatBytes(migrateProgress.bytesTotal)}`
                    : ""}
                </span>
              </div>
              <Progress
                value={
                  migrateProgress.bytesTotal > 0
                    ? Math.min(100, (migrateProgress.bytesDone / migrateProgress.bytesTotal) * 100)
                    : migrateProgress.itemsTotal > 0
                      ? (migrateProgress.itemsDone / migrateProgress.itemsTotal) * 100
                      : 0
                }
              />
              <div className="flex justify-end">
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() =>
                    void api.cancelStorageMigration().catch((e) => toast.error(String(e)))
                  }
                >
                  取消迁移
                </Button>
              </div>
            </div>
          )}
        </dl>
      </section>

      {/* 快捷键 */}
      <ShortcutsSection />

      {/* 更改目录确认框 */}
      <ModelDialog
        open={dirDialogOpen}
        onClose={() => setDirDialogOpen(false)}
        title="更改数据目录"
        footer={
          <div className="flex justify-end gap-2">
            <Button size="sm" variant="ghost" onClick={() => setDirDialogOpen(false)}>
              取消
            </Button>
            <Button size="sm" onClick={() => void changeStorageDir()}>
              选择目录
            </Button>
          </div>
        }
      >
        <p className="text-sm text-text-muted">
          切换后新的模型下载 / 导入将进入新目录，已有模型仍保持可用。
          如需释放旧目录空间，可稍后在「存储位置」执行迁移。 settings 与日志等小文件仍保留在
          ~/.zapmomo。
        </p>
      </ModelDialog>

      {/* 迁移确认框 */}
      <ModelDialog
        open={migrateDialogOpen}
        onClose={() => setMigrateDialogOpen(false)}
        title="迁移已有模型"
        footer={
          <div className="flex justify-end gap-2">
            <Button size="sm" variant="ghost" onClick={() => setMigrateDialogOpen(false)}>
              取消
            </Button>
            <Button size="sm" onClick={() => void startMigration()}>
              开始迁移
            </Button>
          </div>
        }
      >
        <p className="text-sm text-text-muted">
          将把旧目录中的模型迁移到当前数据目录。同盘迁移瞬时完成；跨盘复制耗时较长且期间请勿关闭应用。
          迁移可随时取消，已迁移的模型会保留。
        </p>
      </ModelDialog>
    </div>
  );
}
