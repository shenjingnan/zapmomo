import { AppWindow } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Switch } from "@/components/ui/switch";
import { api } from "@/lib/tauri";
import type { CompanionDragMode, CompanionWindowLayer } from "@/types/tauri";

/**
 * 设置页「伙伴窗口」区块。
 *
 * 层级 / 点击穿透 / 锁定位置 / 修饰键拖动四个窗口级行为（全局生效，
 * 对所有伙伴一致），从伙伴页迁入。挂载时经 get_live2d_config 恢复，
 * 切换乐观更新，写配置失败时回滚到原值。
 */
export function CompanionWindowSection() {
  const [layer, setLayer] = useState<CompanionWindowLayer>("front");
  const [clickThrough, setClickThrough] = useState(false);
  const [locked, setLocked] = useState(false);
  const [dragMode, setDragMode] = useState<CompanionDragMode>("direct");

  useEffect(() => {
    void api
      .getLive2dConfig()
      .then((cfg) => {
        // 旧后端 / 测试桩可能不返回该字段，兜底为关闭。
        if (cfg.window_layer) setLayer(cfg.window_layer);
        setClickThrough(cfg.click_through ?? false);
        setLocked(cfg.locked ?? false);
        setDragMode(cfg.drag_mode ?? "direct");
      })
      .catch(() => {});
  }, []);

  const handleLayerChange = useCallback((checked: boolean) => {
    const next: CompanionWindowLayer = checked ? "front" : "back";
    setLayer(next);
    void api
      .setCompanionLayer({ layer: next })
      .catch(() => setLayer(next === "front" ? "back" : "front"));
  }, []);

  const handleToggleClickThrough = useCallback((enabled: boolean) => {
    setClickThrough(enabled);
    void api.setCompanionClickThrough({ enabled }).catch(() => setClickThrough(!enabled));
  }, []);

  const handleToggleLocked = useCallback((enabled: boolean) => {
    setLocked(enabled);
    void api.setCompanionLocked({ enabled }).catch(() => setLocked(!enabled));
  }, []);

  const handleToggleDragMode = useCallback((enabled: boolean) => {
    const next: CompanionDragMode = enabled ? "modifier" : "direct";
    setDragMode(next);
    void api
      .setCompanionDragMode({ mode: next })
      .catch(() => setDragMode(next === "modifier" ? "direct" : "modifier"));
  }, []);

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <AppWindow className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">伙伴窗口</h2>
            <p className="mt-0.5 text-xs text-text-muted">窗口行为全局生效，对所有伙伴一致</p>
          </div>
        </div>
      </div>

      <dl className="divide-y divide-divider">
        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <div className="min-w-0">
            <dt className="text-sm text-text-primary">层级</dt>
            <dd className="mt-0.5 text-xs text-text-muted">
              {layer === "front"
                ? "置顶：悬浮在所有窗口之上"
                : "置底：沉到所有窗口之下（点穿，无法拖拽/右键）"}
            </dd>
          </div>
          <Switch
            aria-label="置顶"
            checked={layer === "front"}
            onCheckedChange={handleLayerChange}
          />
        </div>

        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <div className="min-w-0">
            <dt className="text-sm text-text-primary">点击穿透</dt>
            <dd className="mt-0.5 text-xs text-text-muted">
              鼠标点击穿过窗口直达背后内容（可从托盘菜单关闭）
            </dd>
          </div>
          <Switch
            aria-label="点击穿透"
            checked={clickThrough}
            onCheckedChange={handleToggleClickThrough}
          />
        </div>

        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <div className="min-w-0">
            <dt className="text-sm text-text-primary">锁定位置</dt>
            <dd className="mt-0.5 text-xs text-text-muted">
              禁止拖动窗口，滚轮缩放与右键菜单不受影响
            </dd>
          </div>
          <Switch aria-label="锁定位置" checked={locked} onCheckedChange={handleToggleLocked} />
        </div>

        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <div className="min-w-0">
            <dt className="text-sm text-text-primary">修饰键拖动</dt>
            <dd className="mt-0.5 text-xs text-text-muted">需按住 ⌘/Ctrl 才能拖动窗口</dd>
          </div>
          <Switch
            aria-label="修饰键拖动"
            checked={dragMode === "modifier"}
            onCheckedChange={handleToggleDragMode}
          />
        </div>
      </dl>
    </section>
  );
}
