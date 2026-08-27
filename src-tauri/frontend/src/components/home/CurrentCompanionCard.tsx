import { Sparkles } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { SharedLive2dStage } from "@/components/live2d/SharedLive2dStage";
import { Badge } from "@/components/ui/badge";
import { Slider } from "@/components/ui/slider";
import { isStaticImageFormat } from "@/lib/companionFormat";
import { api, onCompanionOpacityChanged, onCompanionScaleChanged, toAssetUrl } from "@/lib/tauri";
import type { CompanionModelInfo } from "@/types/tauri";

interface CurrentCompanionCardProps {
  /** 当前使用中的伙伴（active_model_id 对应项），无则为 null */
  companion: CompanionModelInfo | null;
  loading: boolean;
  error: string | null;
}

/**
 * 概览页「当前伙伴」卡片：顶部名称/使用中徽标与尺寸/透明度控制 + Live2D 实时预览。
 *
 * - 预览复用共享舞台 `SharedLive2dStage`（与伙伴页同一 PIXI 实例，ResizeObserver 量测容器尺寸），
 *   渲染失败时按伙伴重试两次（启动瞬间 GPU 繁忙导致的瞬时失败可自愈），
 *   仍失败才回退 `cover_image` 静态封面，无封面则提示文案；
 * - 尺寸/透明度与伙伴页共用同一持久化状态（settings.toml [live2d].window_scale /
 *   window_opacity），写入后桌宠窗口即时生效；其它入口（滚轮/菜单）改动时经事件同步显示值。
 */
export function CurrentCompanionCard({ companion, loading, error }: CurrentCompanionCardProps) {
  // Live2D 预览：量测容器尺寸交给 SharedLive2dStage（PIXI 需要非 0 尺寸）。
  const previewRef = useRef<HTMLDivElement>(null);
  const [previewSize, setPreviewSize] = useState({ width: 0, height: 0 });
  // Live2D 挂载尝试（伙伴 id + 次数）：伙伴 id 变化自然重置；失败后隔 1.2s 递增
  // 触发重挂载重试，超过上限才回退静态封面（避免瞬时失败被永久记死）。
  const MAX_LIVE2D_RETRIES = 2;
  const [live2dAttempt, setLive2dAttempt] = useState<{ id: string; n: number } | null>(null);
  const attemptFor = live2dAttempt && live2dAttempt.id === companion?.id ? live2dAttempt.n : 0;
  const retryExhausted = attemptFor > MAX_LIVE2D_RETRIES;

  useEffect(() => {
    const el = previewRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect) {
        setPreviewSize({ width: Math.round(rect.width), height: Math.round(rect.height) });
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const handleStageError = useCallback(() => {
    if (!companion) return;
    // 短暂退避后递增尝试次数：reloadKey 变化 → 共享舞台强制销毁重载模型。
    window.setTimeout(() => {
      setLive2dAttempt((prev) => ({
        id: companion.id,
        n: (prev?.id === companion.id ? prev.n : 0) + 1,
      }));
    }, 1200);
  }, [companion]);

  // 桌宠尺寸（缩放百分比，25%~200%）与透明度（20%~100%）：初始从持久化配置读取。
  const [percent, setPercent] = useState(100);
  const [opacityPercent, setOpacityPercent] = useState(100);
  useEffect(() => {
    void api
      .getLive2dConfig()
      .then((cfg) => {
        if (cfg.window_scale != null) setPercent(Math.round(cfg.window_scale * 100));
        if (cfg.window_opacity != null) setOpacityPercent(Math.round(cfg.window_opacity * 100));
      })
      .catch(() => {});
  }, []);

  // 桌宠窗口滚轮 / 右键菜单改尺寸时同步显示值（自己写入触发的同值事件是 no-op）。
  useEffect(() => {
    const unlisten = onCompanionScaleChanged((scale) => {
      setPercent(Math.round(scale * 100));
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 右键菜单 / 托盘改透明度时同步显示值。
  useEffect(() => {
    const unlisten = onCompanionOpacityChanged((opacity) => {
      setOpacityPercent(Math.round(opacity * 100));
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

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

  // 预览分支：静态图像（GIF/角色包立绘）原生 img > Live2D 实时渲染 > 重试耗尽回退封面/文案 > 空态 / 加载中 / 模型不可用
  const isGif = isStaticImageFormat(companion?.format);
  const stageReady =
    companion?.valid === true && !retryExhausted && previewSize.width > 0 && previewSize.height > 0;
  let preview: ReactNode;
  if (companion?.valid && isGif) {
    // 静态图像伙伴（GIF / 角色包立绘）不走 PIXI，直接原生 img 展示。
    preview = (
      <img
        src={toAssetUrl(companion.model_file)}
        alt={companion.name}
        draggable={false}
        className="h-full w-full select-none object-contain drop-shadow-md"
      />
    );
  } else if (companion == null) {
    preview = loading ? (
      <p className="text-sm text-text-muted">加载中…</p>
    ) : (
      <div className="flex flex-col items-center gap-1 text-center text-sm text-muted-foreground">
        <Sparkles className="h-8 w-8 text-muted-foreground/50" />
        尚未选择桌面伙伴
        <span className="text-xs">在伙伴页导入 Live2D 模型开始使用。</span>
        {error && (
          <span className="mt-1 max-w-[240px] truncate text-xs text-destructive" title={error}>
            {error}
          </span>
        )}
      </div>
    );
  } else if (!companion.valid) {
    preview = <p className="text-sm text-muted-foreground">无法加载该 Live2D 模型</p>;
  } else if (retryExhausted) {
    preview = companion.cover_image ? (
      <img
        src={toAssetUrl(companion.cover_image)}
        alt={companion.name}
        className="h-full w-full object-contain drop-shadow-md"
      />
    ) : (
      <p className="text-sm text-muted-foreground">无法加载该 Live2D 模型</p>
    );
  } else {
    preview = stageReady ? (
      // reloadKey 等价原 React key 语义：伙伴变化或失败重试时强制销毁重载模型。
      <SharedLive2dStage
        reloadKey={`${companion.id}-${attemptFor}`}
        modelUrl={toAssetUrl(companion.model_file)}
        width={previewSize.width}
        height={previewSize.height}
        modelScale={0.8}
        onError={handleStageError}
        className="h-full w-full"
      />
    ) : null;
  }

  return (
    <section
      aria-label="当前伙伴"
      className="flex min-w-0 min-h-0 flex-col rounded-[16px] border border-panel-border bg-panel-background"
    >
      <div className="px-5 pt-4">
        <h2 className="text-base font-semibold text-text-primary">当前伙伴</h2>
        {companion != null && (
          <div className="mt-1 flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-lg font-semibold text-text-primary">{companion.name}</span>
              <Badge
                variant="outline"
                className="shrink-0 border-emerald-200 bg-emerald-50 text-emerald-700"
              >
                使用中
              </Badge>
              {!companion.valid && <span className="text-xs text-destructive">模型不可用</span>}
            </div>

            {/* 尺寸/透明度：与伙伴页同一控制，写入后桌宠窗口即时生效（尺寸在上，透明度在下） */}
            <div className="flex flex-col gap-2 text-sm text-muted-foreground">
              <div className="flex items-center gap-2">
                <span className="w-12 shrink-0">尺寸</span>
                <Slider
                  aria-label="尺寸"
                  value={[percent]}
                  min={25}
                  max={200}
                  step={5}
                  onValueChange={([v]) => handleScaleChange(v)}
                  className="w-28"
                />
                <span className="w-10 shrink-0 text-right tabular-nums">{percent}%</span>
              </div>
              <div className="flex items-center gap-2">
                <span className="w-12 shrink-0">透明度</span>
                <Slider
                  aria-label="透明度"
                  value={[opacityPercent]}
                  min={20}
                  max={100}
                  step={5}
                  onValueChange={([v]) => handleOpacityChange(v)}
                  className="w-28"
                />
                <span className="w-10 shrink-0 text-right tabular-nums">{opacityPercent}%</span>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* 预览区：Live2D 实时渲染，居中留白呼吸 */}
      <div
        ref={previewRef}
        className="flex min-h-0 flex-1 items-center justify-center overflow-hidden px-5 pb-5 pt-3"
      >
        {preview}
      </div>
    </section>
  );
}
