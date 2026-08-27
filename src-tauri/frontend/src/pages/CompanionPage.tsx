import { open } from "@tauri-apps/plugin-dialog";
import {
  CircleAlert,
  Image as ImageIcon,
  Info,
  Pencil,
  Sparkles,
  Star,
  Trash2,
  Upload,
} from "lucide-react";
import {
  type MouseEvent as ReactMouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { LibraryDialog } from "@/components/library/LibraryDialog";
import type { Live2dCatalog } from "@/components/live2d/previewManager";
import type { SharedLive2dStageHandle } from "@/components/live2d/SharedLive2dStage";
import { SharedLive2dStage } from "@/components/live2d/SharedLive2dStage";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCompanionLibrary } from "@/hooks/useCompanionLibrary";
import { isStaticImageFormat } from "@/lib/companionFormat";
import { api, toAssetUrl } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import type { CompanionDragMode, CompanionModelInfo, CompanionWindowLayer } from "@/types/tauri";

/**
 * 把 Live2D 渲染画布截取为缩小的 PNG 字节数组（供保存为封面）。
 * 等比缩小到最长边 maxSize，避免封面文件过大。
 */
function canvasToPngBytes(canvas: HTMLCanvasElement, maxSize = 256): number[] {
  const scale = Math.min(1, maxSize / Math.max(canvas.width, canvas.height));
  const w = Math.max(1, Math.round(canvas.width * scale));
  const h = Math.max(1, Math.round(canvas.height * scale));
  const out = document.createElement("canvas");
  out.width = w;
  out.height = h;
  const ctx = out.getContext("2d");
  if (!ctx) return [];
  ctx.drawImage(canvas, 0, 0, w, h);
  const base64 = out.toDataURL("image/png").split(",")[1];
  if (!base64) return [];
  const binary = window.atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return Array.from(bytes);
}

/** 左侧列表项：placeholder 缩略图 + 名称 + 重命名/移除 + active Badge + selected 高亮。 */
function CompanionListItem({
  model,
  selected,
  isActive,
  onSelect,
  onRename,
  onRequestRemove,
}: {
  model: CompanionModelInfo;
  selected: boolean;
  isActive: boolean;
  onSelect: () => void;
  onRename: (id: string, name: string) => void;
  onRequestRemove: (model: CompanionModelInfo) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(model.name);
  const inputRef = useRef<HTMLInputElement>(null);
  /** 防止 Enter 提交与 onBlur 提交重复触发（blur 在 setEditing 后可能还会跑一次）。 */
  const doneRef = useRef(false);

  // 进入编辑态：选中全部文本，方便直接覆盖。
  useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const startEdit = useCallback(
    (e: ReactMouseEvent) => {
      e.stopPropagation();
      setDraft(model.name);
      doneRef.current = false;
      setEditing(true);
    },
    [model.name],
  );

  const commitEdit = useCallback(() => {
    if (doneRef.current) return;
    doneRef.current = true;
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed && trimmed !== model.name) {
      onRename(model.id, trimmed);
    }
  }, [draft, model.id, model.name, onRename]);

  const cancelEdit = useCallback(() => {
    doneRef.current = true;
    setEditing(false);
  }, []);

  return (
    <div
      data-testid={`companion-item-${model.id}`}
      className={cn(
        "group relative flex items-center gap-1 rounded-lg border px-3 py-2 transition-colors",
        selected ? "border-primary/60 bg-primary/5" : "border-transparent hover:bg-muted/60",
      )}
    >
      <button
        type="button"
        onClick={onSelect}
        className="flex min-w-0 flex-1 items-center gap-3 text-left"
      >
        {/* 缩略图：优先模型封面图，无则占位图标（封面加载失败时隐藏，露出占位） */}
        <span
          className={cn(
            "relative flex h-10 w-10 shrink-0 items-center justify-center overflow-hidden rounded-md bg-muted",
            selected ? "text-primary" : "text-muted-foreground",
          )}
        >
          <Sparkles className="h-5 w-5" />
          {model.cover_image && (
            <img
              src={toAssetUrl(model.cover_image)}
              alt=""
              className="absolute inset-0 h-full w-full object-cover"
              onError={(e) => {
                e.currentTarget.style.display = "none";
              }}
            />
          )}
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate text-sm font-medium text-text-primary">{model.name}</span>
            {isActive && (
              <Badge
                variant="outline"
                className="shrink-0 border-emerald-200 bg-emerald-50 text-emerald-700"
              >
                使用中
              </Badge>
            )}
          </span>
          {!model.valid && <span className="block text-xs text-destructive">模型不可用</span>}
          {/* 角色包能力标记：人设（character.md）/ 音色（voice/ 克隆参考） */}
          {(model.has_persona || model.has_voice) && (
            <span className="mt-0.5 flex gap-1">
              {model.has_persona && (
                <Badge variant="outline" className="px-1 py-0 text-[10px] text-muted-foreground">
                  人设
                </Badge>
              )}
              {model.has_voice && (
                <Badge variant="outline" className="px-1 py-0 text-[10px] text-muted-foreground">
                  音色
                </Badge>
              )}
            </span>
          )}
        </span>
      </button>

      {!editing && (
        <button
          type="button"
          aria-label={`重命名「${model.name}」`}
          onClick={startEdit}
          className="shrink-0 rounded p-1 text-muted-foreground opacity-60 transition-opacity group-hover:opacity-100 hover:text-text-primary focus:opacity-100"
        >
          <Pencil className="h-3.5 w-3.5" />
        </button>
      )}

      {!editing && (
        <button
          type="button"
          aria-label={`移除「${model.name}」`}
          onClick={() => onRequestRemove(model)}
          disabled={isActive}
          title={isActive ? "请先切换其他伙伴为使用中再移除" : undefined}
          className={cn(
            "shrink-0 rounded p-1 transition-opacity",
            isActive
              ? "cursor-not-allowed text-muted-foreground/40 opacity-40"
              : "text-muted-foreground opacity-60 group-hover:opacity-100 hover:text-red-600 focus:opacity-100",
          )}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      )}

      {/* 编辑态覆盖整行：避免嵌套可交互元素 */}
      {editing && (
        <div className="absolute inset-0 z-10 flex items-center rounded-lg border border-primary/60 bg-panel-background px-3">
          <Input
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitEdit();
              else if (e.key === "Escape") cancelEdit();
            }}
            onBlur={commitEdit}
            autoFocus
            className="h-8 min-w-0 flex-1"
          />
        </div>
      )}
    </div>
  );
}

/** 动作/表情预览面板：手写下划线 Tab（对齐 ModelDetailPane 先例，不引 Tabs 依赖）。 */
function MotionCatalogPanel({
  catalog,
  onPlayMotion,
  onApplyExpression,
  onResetExpression,
}: {
  catalog: Live2dCatalog;
  onPlayMotion: (group: string, index: number) => void;
  onApplyExpression: (index: number) => void;
  onResetExpression: () => void;
}) {
  const [tab, setTab] = useState<"motions" | "expressions">("motions");
  const [playingKey, setPlayingKey] = useState<string | null>(null);
  const [appliedExpression, setAppliedExpression] = useState<number | null>(null);

  const hasMotions = catalog.motionGroups.length > 0;
  const hasExpressions = catalog.expressions.length > 0;
  const activeTab = tab === "motions" && !hasMotions ? "expressions" : tab;

  const handlePlay = (group: string, index: number) => {
    setPlayingKey(`${group}/${index}`);
    // 播放结束（或失败）后恢复按钮；首次播放含懒加载延迟。
    void Promise.resolve(onPlayMotion(group, index)).finally(() => setPlayingKey(null));
  };

  const empty = !hasMotions && !hasExpressions;
  return (
    <div className="mt-3 shrink-0 border-t border-panel-border pt-3" data-testid="motion-catalog">
      {empty ? (
        <p className="text-xs text-muted-foreground">此模型未提供动作或表情</p>
      ) : (
        <>
          {hasMotions && hasExpressions && (
            <div role="tablist" aria-label="预览类型" className="mb-2 flex gap-4">
              {(["motions", "expressions"] as const).map((t) => (
                <button
                  key={t}
                  type="button"
                  role="tab"
                  aria-selected={activeTab === t}
                  onClick={() => setTab(t)}
                  className={cn(
                    "border-b-2 pb-1 text-sm transition-colors",
                    activeTab === t
                      ? "border-blue-500 text-text-primary"
                      : "border-transparent text-muted-foreground hover:text-text-primary",
                  )}
                >
                  {t === "motions" ? "动作" : "表情"}
                </button>
              ))}
            </div>
          )}
          <div className="flex max-h-40 flex-wrap content-start gap-2 overflow-y-auto">
            {activeTab === "motions"
              ? catalog.motionGroups.map(({ group, motions }) => (
                  <div key={group} className="w-full">
                    {catalog.motionGroups.length > 1 && (
                      <p className="mb-1 text-xs font-medium text-muted-foreground">{group}</p>
                    )}
                    <div className="flex flex-wrap gap-2">
                      {motions.map((m) => {
                        const key = `${group}/${m.index}`;
                        return (
                          <Button
                            key={key}
                            variant="outline"
                            size="sm"
                            aria-label={`播放动作 ${m.name}`}
                            disabled={playingKey !== null}
                            onClick={() => handlePlay(group, m.index)}
                          >
                            {playingKey === key ? "播放中…" : m.name}
                          </Button>
                        );
                      })}
                    </div>
                  </div>
                ))
              : catalog.expressions.map((e) => (
                  <Button
                    key={e.index}
                    variant={appliedExpression === e.index ? "default" : "outline"}
                    size="sm"
                    aria-label={`应用表情 ${e.name}`}
                    onClick={() => {
                      onApplyExpression(e.index);
                      setAppliedExpression(e.index);
                    }}
                  >
                    {e.name}
                  </Button>
                ))}
            {activeTab === "expressions" && appliedExpression != null && (
              <Button
                variant="ghost"
                size="sm"
                aria-label="重置表情"
                onClick={() => {
                  onResetExpression();
                  setAppliedExpression(null);
                }}
              >
                重置表情
              </Button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

/**
 * 伙伴页：伙伴模型管理器。
 *
 * - 左侧：我的伙伴列表（selected = 蓝色高亮；active = 名字旁绿色「使用中」Badge）；
 * - 右侧：Live2D 预览；非当前使用时显示「设为当前使用」。
 *
 * 状态区分：`selectedCompanionId`（页面 local state，仅切换预览，不动桌宠）与
 * `activeModelId`（后端 `library.json` 持久化，真正驱动桌宠窗口）。
 */
export function CompanionPage() {
  const { library, loading, error, importModel, setActive, rename, remove, saveCover } =
    useCompanionLibrary();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [stageError, setStageError] = useState<string | null>(null);
  const [previewSize, setPreviewSize] = useState({ width: 0, height: 0 });
  const [removeTarget, setRemoveTarget] = useState<CompanionModelInfo | null>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const stageHandleRef = useRef<SharedLive2dStageHandle>(null);
  const [catalog, setCatalog] = useState<Live2dCatalog | null>(null);
  /** 最近一次待移除目标：关闭动画期间 removeTarget 已置空，用它兜底保持正文不闪空。 */
  const lastRemoveTarget = useRef<CompanionModelInfo | null>(null);

  useEffect(() => {
    if (removeTarget) lastRemoveTarget.current = removeTarget;
  }, [removeTarget]);

  const selected = useMemo(
    () => library?.models.find((m) => m.id === selectedId) ?? null,
    [library, selectedId],
  );
  const isActive = selected != null && selected.id === library?.active_model_id;

  // selected 校正：切换模型 / 库变化后，selected 不存在时落到 active(valid) → 首个 valid → null。
  useEffect(() => {
    if (!library) return;
    if (selectedId && library.models.some((m) => m.id === selectedId)) {
      return;
    }
    const active = library.models.find((m) => m.id === library.active_model_id && m.valid);
    const fallback = active ?? library.models.find((m) => m.valid) ?? null;
    setSelectedId(fallback?.id ?? null);
  }, [library, selectedId]);

  const selectModel = useCallback((id: string) => {
    // 切换选中模型时重置渲染错误并清空旧目录（不能放在 effect 里随 selectedId 清：
    // 首次校正 selectedId 会与 onModelCatalog 竞争，把新目录覆盖成 null）。
    setStageError(null);
    setCatalog(null);
    setSelectedId(id);
  }, []);

  // 量测预览容器尺寸，交给 SharedLive2dStage（PIXI 需要非 0 尺寸）。
  useEffect(() => {
    const el = previewRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect) {
        setPreviewSize({
          width: Math.round(rect.width),
          height: Math.round(rect.height),
        });
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const handleStageError = useCallback((e: Error) => {
    setStageError(e.message);
  }, []);

  // 桌宠尺寸（缩放百分比，25%~200%，伙伴私有）与透明度（20%~100%，全局）：
  // 写入后通知桌宠窗口即时生效；切换 active 伙伴时重读以刷新滑杆显示值。
  // 点击穿透（窗口级行为，与选中哪个伙伴无关）：开启后桌宠窗口对所有鼠标事件透明。
  // 显示层级（置顶/置底，窗口级）：写入 settings 并通知桌宠窗口即时生效。
  // 拖拽模式（窗口级）：modifier = 需按住 ⌘/Ctrl 才能拖动，direct = 直接拖动。
  const [percent, setPercent] = useState(100);
  const [opacityPercent, setOpacityPercent] = useState(100);
  const [clickThrough, setClickThrough] = useState(false);
  const [layer, setLayer] = useState<CompanionWindowLayer>("front");
  const [locked, setLocked] = useState(false);
  const [dragMode, setDragMode] = useState<CompanionDragMode>("direct");
  // biome-ignore lint/correctness/useExhaustiveDependencies: library?.active_model_id 是伙伴切换触发器（缩放为伙伴私有，需重读刷新滑杆）
  useEffect(() => {
    void api
      .getLive2dConfig()
      .then((cfg) => {
        if (cfg.window_scale != null) setPercent(Math.round(cfg.window_scale * 100));
        if (cfg.window_opacity != null) setOpacityPercent(Math.round(cfg.window_opacity * 100));
        // 旧后端 / 测试桩可能不返回该字段，兜底为关闭。
        setClickThrough(cfg.click_through ?? false);
        if (cfg.window_layer) setLayer(cfg.window_layer);
        setLocked(cfg.locked ?? false);
        setDragMode(cfg.drag_mode ?? "direct");
      })
      .catch(() => {});
  }, [library?.active_model_id]);
  const handleScaleChange = useCallback((value: number) => {
    const clamped = Math.max(25, Math.min(200, Math.round(value)));
    setPercent(clamped);
    void api.setCompanionScale({ scale: clamped / 100 });
  }, []);
  const handleOpacityChange = useCallback((value: number) => {
    const clamped = Math.max(20, Math.min(100, Math.round(value)));
    setOpacityPercent(clamped);
    void api.setCompanionOpacity({ opacity: clamped / 100 });
  }, []);
  const handleToggleClickThrough = useCallback((enabled: boolean) => {
    setClickThrough(enabled);
    void api.setCompanionClickThrough({ enabled });
  }, []);
  const handleLayerChange = useCallback((checked: boolean) => {
    const next: CompanionWindowLayer = checked ? "front" : "back";
    setLayer(next);
    void api.setCompanionLayer({ layer: next });
  }, []);
  const handleToggleLocked = useCallback((enabled: boolean) => {
    setLocked(enabled);
    void api.setCompanionLocked({ enabled });
  }, []);
  const handleToggleDragMode = useCallback((enabled: boolean) => {
    const next: CompanionDragMode = enabled ? "modifier" : "direct";
    setDragMode(next);
    void api.setCompanionDragMode({ mode: next });
  }, []);

  // 通用导入收尾：清错误、调导入命令并选中新伙伴。
  const importAndSelect = useCallback(
    async (source: string) => {
      setStageError(null);
      const model = await importModel(source);
      if (model) {
        setSelectedId(model.id);
      }
    },
    [importModel],
  );

  const handleImportDir = useCallback(async () => {
    const dir = await open({ directory: true, title: "选择 Live2D 模型或角色包目录" });
    if (typeof dir === "string") await importAndSelect(dir);
  }, [importAndSelect]);

  const handleImportGif = useCallback(async () => {
    const file = await open({
      title: "选择 GIF 动图",
      filters: [{ name: "GIF 动图", extensions: ["gif"] }],
    });
    if (typeof file === "string") await importAndSelect(file);
  }, [importAndSelect]);

  const handleRemoveConfirm = useCallback(() => {
    if (!removeTarget) return;
    void remove(removeTarget.id);
    setRemoveTarget(null);
  }, [remove, removeTarget]);

  // 每个伙伴在本次会话里只尝试生成一次封面（无封面时才生成）。
  const coverAttempted = useRef(new Set<string>());
  const handleModelReady = useCallback(
    (canvas: HTMLCanvasElement) => {
      if (!selected || selected.cover_image || coverAttempted.current.has(selected.id)) return;
      coverAttempted.current.add(selected.id);
      // 等一帧确保 PIXI ticker 已把模型画到画布，再截取保存。
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          const png = canvasToPngBytes(canvas);
          if (png.length > 0) {
            void saveCover(selected.id, png);
          }
        });
      });
    },
    [selected, saveCover],
  );

  const isGif = isStaticImageFormat(selected?.format);
  const previewUrl = selected ? toAssetUrl(selected.model_file) : null;
  // 静态图像伙伴（GIF/角色包立绘）不走 PIXI 预览（无 canvas/动作目录），单独 img 分支。
  const showStage = !!selected?.valid && !isGif && previewSize.width > 0 && previewSize.height > 0;

  return (
    <div className="flex h-full flex-col gap-4">
      {/* 顶部：页面标题 */}
      <div>
        <h1 className="text-xl font-semibold tracking-tight text-text-primary">伙伴</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">导入并管理你的桌面伙伴</p>
      </div>

      <div className="flex min-h-0 flex-1 gap-4">
        {/* 左侧：我的伙伴（边框/阴影对齐模型库界面的面板样式） */}
        <Card className="flex w-[460px] shrink-0 flex-col border-panel-border shadow-none">
          <CardHeader className="flex-row items-center justify-between gap-3 space-y-0">
            <div>
              <CardTitle className="flex items-center gap-2 text-base font-semibold text-text-primary">
                <Sparkles className="h-4 w-4 text-muted-foreground" />
                我的伙伴
              </CardTitle>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <Button size="sm" onClick={handleImportDir} disabled={loading}>
                <Upload className="h-4 w-4" />
                导入模型 / 角色包
              </Button>
              <Button size="sm" variant="outline" onClick={handleImportGif} disabled={loading}>
                <ImageIcon className="h-4 w-4" />
                导入 GIF
              </Button>
            </div>
          </CardHeader>
          <CardContent className="flex min-h-0 flex-1 flex-col gap-3">
            {error && (
              <Alert variant="destructive">
                <CircleAlert className="h-4 w-4" />
                <AlertDescription className="whitespace-pre-wrap">{error}</AlertDescription>
              </Alert>
            )}

            {!library && loading ? (
              <p className="py-6 text-center text-sm text-muted-foreground">加载中…</p>
            ) : library && library.models.length === 0 ? (
              <div className="flex flex-1 flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-panel-border p-6 text-center">
                <Sparkles className="h-6 w-6 text-muted-foreground/50" />
                <p className="text-sm font-medium text-text-primary">还没有伙伴</p>
                <p className="text-xs text-muted-foreground">
                  导入一个 Live2D 模型或角色包，让它成为你的桌面伙伴。
                </p>
              </div>
            ) : (
              <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-0.5">
                {library?.models.map((model) => (
                  <CompanionListItem
                    key={model.id}
                    model={model}
                    selected={model.id === selectedId}
                    isActive={model.id === library.active_model_id}
                    onSelect={() => selectModel(model.id)}
                    onRename={(id, name) => void rename(id, name)}
                    onRequestRemove={setRemoveTarget}
                  />
                ))}
              </div>
            )}
          </CardContent>
        </Card>

        {/* 右侧：预览（边框/阴影对齐模型库界面的面板样式） */}
        <Card className="flex min-w-0 flex-1 flex-col border-panel-border shadow-none">
          <CardHeader className="flex-col items-start gap-3 space-y-0">
            <CardTitle className="text-base font-semibold">
              {selected ? selected.name : "暂无伙伴"}
            </CardTitle>
            {/* 桌宠尺寸/透明度：调整窗口缩放与模型透明度，同步到桌宠窗口（尺寸在上，透明度在下）。
                点击穿透/显示层级是窗口级行为，未选中伙伴也可切换（窗口仍在，遮挡桌面）。 */}
            <div className="flex w-full flex-col gap-2 text-sm text-muted-foreground">
              {selected && (
                <>
                  <div className="flex w-full items-center gap-2">
                    <span className="w-20 shrink-0">尺寸</span>
                    <Slider
                      aria-label="尺寸"
                      value={[percent]}
                      min={25}
                      max={200}
                      step={5}
                      onValueChange={([v]) => handleScaleChange(v)}
                      className="flex-1"
                    />
                    <span className="w-10 shrink-0 text-right tabular-nums">{percent}%</span>
                  </div>
                  <div className="flex w-full items-center gap-2">
                    <span className="w-20 shrink-0">透明度</span>
                    <Slider
                      aria-label="透明度"
                      value={[opacityPercent]}
                      min={20}
                      max={100}
                      step={5}
                      onValueChange={([v]) => handleOpacityChange(v)}
                      className="flex-1"
                    />
                    <span className="w-10 shrink-0 text-right tabular-nums">{opacityPercent}%</span>
                  </div>
                </>
              )}
              {/* 显示层级：置顶 = 悬浮浮层（默认，现状）；置底 = 沉到窗口之下并点穿（窗口级） */}
              <div className="flex w-full items-center gap-2">
                <span className="w-20 shrink-0">层级</span>
                <Switch
                  aria-label="置顶"
                  checked={layer === "front"}
                  onCheckedChange={handleLayerChange}
                />
                <span className="min-w-0 flex-1 text-xs text-muted-foreground">
                  {layer === "front"
                    ? "置顶：悬浮在所有窗口之上"
                    : "置底：沉到所有窗口之下（点穿，无法拖拽/右键）"}
                </span>
              </div>
              {/* 点击穿透（窗口级）：说明收进 Info icon 的 tooltip，对齐「锁定位置」的展示方式 */}
              <div className="flex w-full items-center gap-2">
                <span className="w-20 shrink-0">点击穿透</span>
                <Switch
                  aria-label="点击穿透"
                  checked={clickThrough}
                  onCheckedChange={handleToggleClickThrough}
                />
                <Tooltip>
                  <TooltipTrigger
                    aria-label="点击穿透说明"
                    className="rounded p-0.5 text-muted-foreground/70 transition-colors hover:text-text-primary focus-visible:outline-none"
                  >
                    <Info className="h-3.5 w-3.5" />
                  </TooltipTrigger>
                  <TooltipContent className="max-w-xs">
                    开启后鼠标点击穿过模型直达背后内容；拖动、滚轮缩放与右键菜单将失效，可随时在此或托盘菜单关闭
                  </TooltipContent>
                </Tooltip>
              </div>
              {/* 位置锁定（窗口级）：说明收进 Info icon 的 tooltip，避免占满整行 */}
              <div className="flex w-full items-center gap-2">
                <span className="w-20 shrink-0">锁定位置</span>
                <Switch
                  aria-label="锁定位置"
                  checked={locked}
                  onCheckedChange={handleToggleLocked}
                />
                <Tooltip>
                  <TooltipTrigger
                    aria-label="锁定位置说明"
                    className="rounded p-0.5 text-muted-foreground/70 transition-colors hover:text-text-primary focus-visible:outline-none"
                  >
                    <Info className="h-3.5 w-3.5" />
                  </TooltipTrigger>
                  <TooltipContent className="max-w-xs">
                    开启后禁止拖动窗口，滚轮缩放与右键菜单不受影响
                  </TooltipContent>
                </Tooltip>
              </div>
              {/* 拖拽模式（窗口级）：modifier = 需按住 cmd/Ctrl 才能拖动，与锁定正交（锁定优先），说明收进 Info icon 的 tooltip */}
              <div className="flex w-full items-center gap-2">
                <span className="w-20 shrink-0">修饰键拖动</span>
                <Switch
                  aria-label="修饰键拖动"
                  checked={dragMode === "modifier"}
                  onCheckedChange={handleToggleDragMode}
                />
                <Tooltip>
                  <TooltipTrigger
                    aria-label="修饰键拖动说明"
                    className="rounded p-0.5 text-muted-foreground/70 transition-colors hover:text-text-primary focus-visible:outline-none"
                  >
                    <Info className="h-3.5 w-3.5" />
                  </TooltipTrigger>
                  <TooltipContent className="max-w-xs">
                    开启后需按住 ⌘/Ctrl 才能拖动窗口，滚轮缩放与右键菜单不受影响
                  </TooltipContent>
                </Tooltip>
              </div>
            </div>
          </CardHeader>
          <CardContent className="flex min-h-0 flex-1 flex-col">
            {/* 已是当前使用时不显示 CTA（左侧「使用中」徽标已标识） */}
            {selected && !isActive && (
              <div className="mb-4">
                <Button onClick={() => void setActive(selected.id)} disabled={!selected.valid}>
                  <Star className="h-4 w-4" />
                  设为当前使用
                </Button>
              </div>
            )}

            <div ref={previewRef} className="relative min-h-0 flex-1 overflow-hidden">
              {selected?.valid && isStaticImageFormat(selected.format) && (
                <img
                  src={previewUrl ?? undefined}
                  alt={selected.name}
                  draggable={false}
                  className="pointer-events-none absolute inset-0 h-full w-full select-none object-contain"
                />
              )}
              {showStage && previewUrl && (
                <SharedLive2dStage
                  modelUrl={previewUrl}
                  width={previewSize.width}
                  height={previewSize.height}
                  onError={handleStageError}
                  onModelReady={handleModelReady}
                  onModelCatalog={setCatalog}
                  ref={stageHandleRef}
                  className="h-full w-full"
                />
              )}
              {selected && !selected.valid && (
                <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
                  无法加载该 Live2D 模型
                </div>
              )}
              {!selected && (
                <div className="flex h-full flex-col items-center justify-center gap-1 text-center text-sm text-muted-foreground">
                  <Sparkles className="h-6 w-6 text-muted-foreground/50" />
                  暂无伙伴
                  <span className="text-xs">导入模型后可以在这里预览。</span>
                </div>
              )}
            </div>

            {showStage && catalog && (
              <MotionCatalogPanel
                catalog={catalog}
                onPlayMotion={(group, index) =>
                  void stageHandleRef.current?.playMotion(group, index)
                }
                onApplyExpression={(index) => void stageHandleRef.current?.applyExpression(index)}
                onResetExpression={() => stageHandleRef.current?.resetExpression()}
              />
            )}

            {stageError && (
              <Alert variant="destructive" className="mt-3">
                <CircleAlert className="h-4 w-4" />
                <AlertDescription className="whitespace-pre-wrap">
                  无法加载该 Live2D 模型：{stageError}
                </AlertDescription>
              </Alert>
            )}
          </CardContent>
        </Card>
      </div>

      {/* 移除伙伴确认（样式对齐模型库 ModelConfirmDialog） */}
      <LibraryDialog
        open={removeTarget != null}
        onClose={() => setRemoveTarget(null)}
        title="移除伙伴"
        width="md"
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setRemoveTarget(null)}>
              取消
            </Button>
            <Button variant="destructive" onClick={handleRemoveConfirm}>
              移除
            </Button>
          </div>
        }
      >
        {(removeTarget ?? lastRemoveTarget.current) && (
          <div className="space-y-1">
            <p className="text-sm text-text-primary">
              确定要移除{" "}
              <span className="font-semibold">
                {(removeTarget ?? lastRemoveTarget.current)?.name}
              </span>{" "}
              吗？
            </p>
            <p className="text-sm text-text-secondary">
              移除后，其保存在应用中的模型文件也会被删除。
            </p>
          </div>
        )}
      </LibraryDialog>
    </div>
  );
}
