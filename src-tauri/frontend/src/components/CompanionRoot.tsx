import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import type { Live2DModel } from "pixi-live2d-display/cubism4";
import { useCallback, useEffect, useRef, useState } from "react";
import { EventBubble } from "@/components/companion/EventBubble";
import { GifStage } from "@/components/gif/GifStage";
import {
  Live2dStage,
  type Live2dStageHandle,
  type ModelLayout,
} from "@/components/live2d/Live2dStage";
import { PropsLayer } from "@/components/performance/PropsLayer";
import { usePerformance } from "@/components/performance/usePerformance";
import { VoiceStatusDot } from "@/components/voice/VoiceStatusDot";
import { useLive2dConfig } from "@/hooks/useLive2dConfig";
import { useVoiceSession } from "@/hooks/useVoiceSession";
import { isStaticImageFormat } from "@/lib/companionFormat";
import { pickMotionGroup } from "@/lib/dshMotion";
import {
  api,
  onCompanionDragModeChanged,
  onCompanionLayerChanged,
  onCompanionLockedChanged,
  onCompanionOpacityChanged,
  onCompanionScaleChanged,
  onDshSpeak,
  onLive2dModelChanged,
  toAssetUrl,
} from "@/lib/tauri";
import { centeredResizeTarget } from "@/lib/windowResize";
import type { CompanionDragMode, CompanionWindowLayer, PerformancePropsInfo } from "@/types/tauri";

/** 角色窗口基准高度上限（100% 时高度 = min(480, 屏幕可用高度 × 0.6)）。 */
const BASE_HEIGHT = 480;
/** 窗口尺寸下限与初始值。 */
const MIN_WIDTH = 120;
const MIN_HEIGHT = 120;
const INITIAL_WIDTH = 360;
/** 模型宽高比缺省值（3:4），模型加载后更新为真实宽高比。 */
const DEFAULT_ASPECT_RATIO = 3 / 4;
/** 缩放比例范围（25% ~ 200%）。 */
const SCALE_MIN = 0.25;
const SCALE_MAX = 2.0;
/** cmd/ctrl + 滚轮单格缩放步长。 */
const WHEEL_SCALE_STEP = 1.1;
/**
 * 窗口顶部为 dsh 事件 toast 堆叠预留的高度（最前卡片 + 2 层向上 peek）。
 * 模型渲染区整体下移一条，堆叠卡片永不遮挡模型。
 * 需与后端 `COMPANION_BUBBLE_STRIP`（src-tauri/src/lib.rs）保持一致。
 */
const BUBBLE_STRIP = 72;

/**
 * 常驻角色窗口：展示当前伙伴（Live2D 模型 / GIF 动图 / 角色包静态立绘，按 format 分发渲染）。
 * Live2D 仅呼吸/眨眼等自动动画，不跟随鼠标；静态图像由 WebView 原生 <img> 展示。
 *
 * - 启动时读 `get_live2d_config` 恢复持久化的模型、缩放比例与透明度；
 * - 订阅 `live2d-model-changed` / `companion-scale-changed` / `companion-opacity-changed`，
 *   设置窗口切换模型、缩放或调透明度时即时同步（透明度由包裹模型的 wrapper div 的
 *   `style.opacity` 应用，语音状态点不受影响）；
 * - 窗口尺寸由「基准高度 × scale + 顶部气泡预留条」派生（宽度按模型宽高比，不含预留条），
 *   缩放入口为设置面板、cmd/ctrl+滚轮与原生右键菜单（后端弹原生菜单，不受小窗口裁剪）；
 * - 按住左键拖动移动窗口（位置锁定时禁止；修饰键模式下需按住 cmd/ctrl）；右键弹出原生上下文菜单。
 */
export function CompanionRoot() {
  const containerRef = useRef<HTMLDivElement>(null);
  const { config } = useLive2dConfig();
  // 桌宠窗口无 RuntimeContext：hook 自包含，与设置窗口订阅同一批后端 voice 事件。
  const voice = useVoiceSession();
  // 展示目标（静态图像伙伴（GIF/角色包立绘）渲染 GifStage，Live2D 伙伴渲染 Live2dStage；url null = 清屏）。
  const [stage, setStage] = useState<{ url: string | null; isGif: boolean }>({
    url: null,
    isGif: false,
  });
  const [aspectRatio, setAspectRatio] = useState(DEFAULT_ASPECT_RATIO);
  const [scale, setScale] = useState(1.0);
  const [opacity, setOpacity] = useState(1.0);
  // 显示层级：置底（back）为纯背景装饰（点穿、不可拖拽/右键/滚轮），置顶（front）为现状浮层。
  const [layer, setLayer] = useState<CompanionWindowLayer>("front");
  // 位置锁定：禁止拖动窗口（滚轮缩放与右键菜单保留，右键菜单是解锁入口）。
  const [locked, setLocked] = useState(false);
  // 拖拽模式：modifier = 需按住 cmd/ctrl 才能拖动（缺省 direct = 直接拖动）。
  const [dragMode, setDragMode] = useState<CompanionDragMode>("direct");
  const [size, setSize] = useState({ width: INITIAL_WIDTH, height: BASE_HEIGHT + BUBBLE_STRIP });
  // 表演（BongoCat 兼容模拟键鼠）：道具资源、模型布局与引擎。
  const stageRef = useRef<Live2dStageHandle | null>(null);
  const [props, setProps] = useState<PerformancePropsInfo | null>(null);
  const [modelLayout, setModelLayout] = useState<ModelLayout | null>(null);
  const { pressedKeys } = usePerformance(props, stageRef);

  // Live2D 模型句柄：dsh 事件触发动作用（模型缺对应组时静默跳过）。
  const modelRef = useRef<Live2DModel | null>(null);

  // 用 ref 保存最新值，供异步回调（滚轮/事件/模型加载）读取，避免闭包过期。
  const aspectRatioRef = useRef(aspectRatio);
  aspectRatioRef.current = aspectRatio;
  const scaleRef = useRef(scale);
  scaleRef.current = scale;

  /**
   * 由「基准高度 × scale × 宽高比」计算窗口尺寸（逻辑像素），并 clamp 到屏幕可用区域。
   * 宽度只由模型区（基准高度 × scale）派生；总高度额外加上顶部气泡预留条。
   */
  const computeSize = useCallback((ratio: number, s: number) => {
    const availW = window.screen.availWidth;
    const availH = window.screen.availHeight;
    const baseH = Math.min(BASE_HEIGHT, availH * 0.6);
    const modelH = Math.round(baseH * s);
    let height = modelH + BUBBLE_STRIP;
    let width = Math.round(modelH * ratio);
    height = Math.max(MIN_HEIGHT, Math.min(height, Math.floor(availH * 0.9)));
    width = Math.max(MIN_WIDTH, Math.min(width, Math.floor(availW * 0.9)));
    return { width, height };
  }, []);

  /** 统一设置窗口尺寸并同步本地 state。
   *  anchor="center"：以窗口中心为锚点（用户缩放/外部改比例时角色保持原位）；
   *  anchor="topleft"：锚定左上角（启动恢复阶段使用——记忆的坐标就是最终窗口的左上角，
   *  若用中心锚点，初始尺寸与最终尺寸的差异会让窗口每次启动都偏移并随记忆保存累积）。 */
  const resizeTo = useCallback(
    async (ratio: number, s: number, anchor: "center" | "topleft" = "center") => {
      const win = getCurrentWindow();
      const { width, height } = computeSize(ratio, s);
      if (anchor === "topleft") {
        // setSize 默认固定左上角，正是恢复语义；不动位置就不会触发 onMoved 回写。
        await win.setSize(new LogicalSize(width, height));
        setSize({ width, height });
        return;
      }
      // setSize 默认固定左上角（向右下生长），会使居中的角色表现为从左上角缩放。
      // 读取当前物理位置/尺寸并换算，把左上角移到「保持窗口中心不变」的位置。
      const factor = await win.scaleFactor();
      const pos = await win.outerPosition();
      const cur = await win.outerSize();
      const target = centeredResizeTarget(
        { x: pos.x, y: pos.y, width: cur.width, height: cur.height },
        Math.round(width * factor),
        Math.round(height * factor),
      );
      // 同时发送尺寸+位置（不逐个 await）：避免「先变尺寸、再瞬移归位」的中间态，减少缩放抖动。
      const sizeOp = win.setSize(new LogicalSize(width, height));
      const posOp = win
        .setPosition(
          new LogicalPosition(Math.round(target.x / factor), Math.round(target.y / factor)),
        )
        .catch((e) => {
          // 中心锚定失败（如权限缺失）时降级为默认锚定；尺寸已生效，仍要更新布局避免模型被裁。
          console.warn("中心锚定 setPosition 失败，已降级为默认锚定:", e);
        });
      await Promise.allSettled([sizeOp, posOp]);
      setSize({ width, height });
    },
    [computeSize],
  );

  /** 用户缩放：更新 scale、resize 并持久化比例。 */
  const applyScale = useCallback(
    async (s: number) => {
      const clamped = Math.max(SCALE_MIN, Math.min(SCALE_MAX, s));
      scaleRef.current = clamped; // 立即同步，连续滚轮基于最新值计算下一步
      setScale(clamped);
      await resizeTo(aspectRatioRef.current, clamped);
      await api.setCompanionScale({ scale: clamped });
    },
    [resizeTo],
  );
  const applyScaleRef = useRef(applyScale);
  applyScaleRef.current = applyScale;

  // 启动时恢复持久化的模型（顺带重放行 asset 协议 scope）与 BongoCat 道具资源。
  useEffect(() => {
    if (config?.models_present && config.model_file) {
      setStage({ url: toAssetUrl(config.model_file), isGif: isStaticImageFormat(config.format) });
    }
    setProps(config?.props ?? null);
  }, [config]);

  // 恢复持久化的缩放比例与透明度，并据此 resize 一次（确保前端 state 与后端建窗尺寸一致）。
  // 启动恢复用左上角锚定：后端已把窗口放在记忆坐标（即最终窗口左上角），
  // 中心锚点会因初始/最终尺寸差把窗口推偏，且偏移会被 onMoved 回写、下次启动再偏。
  useEffect(() => {
    if (!config) return;
    const s = config.window_scale ?? 1.0;
    setScale(s);
    setOpacity(config.window_opacity ?? 1.0);
    if (config.window_layer) setLayer(config.window_layer);
    // 旧后端 / 测试桩可能不返回该字段，兜底为未锁定。
    setLocked(config.locked ?? false);
    setDragMode(config.drag_mode ?? "direct");
    void resizeTo(aspectRatioRef.current, s, "topleft");
  }, [config, resizeTo]);

  useEffect(() => {
    const unlisten = onLive2dModelChanged((info) => {
      // 空 model_file = 清屏（active 伙伴被移除等场景）。
      setStage(
        info.model_file && info.format
          ? { url: toAssetUrl(info.model_file), isGif: isStaticImageFormat(info.format) }
          : { url: null, isGif: false },
      );
      setProps(info.props ?? null);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 设置面板改比例时同步（只 resize，不再写回后端避免循环）。
  useEffect(() => {
    const unlisten = onCompanionScaleChanged((s) => {
      // 本窗口滚轮缩放后也会收到这条回显（applyScale → setCompanionScale），值相同则跳过，
      // 避免每次滚轮触发两轮 resize/重布局造成抖动。
      if (s === scaleRef.current) return;
      setScale(s);
      void resizeTo(aspectRatioRef.current, s);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [resizeTo]);

  // 设置面板/菜单改透明度时同步（纯视觉：只更新渲染层 opacity，不涉及窗口尺寸）。
  useEffect(() => {
    const unlisten = onCompanionOpacityChanged((v) => {
      setOpacity(v);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 设置面板切显示层级时同步（置底：隐藏状态点、关闭交互；置顶：恢复）。
  useEffect(() => {
    const unlisten = onCompanionLayerChanged((l) => {
      setLayer(l);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 设置面板/菜单锁定位置时同步（只拦截拖动，不影响缩放与右键）。
  useEffect(() => {
    const unlisten = onCompanionLockedChanged((v) => {
      setLocked(v);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 设置面板切换拖拽模式时同步（只影响 mousedown 拖动条件，不影响缩放与右键）。
  useEffect(() => {
    const unlisten = onCompanionDragModeChanged((m) => {
      setDragMode(m);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 模型加载后更新真实宽高比，并用当前 scale 重算尺寸（不持久化 scale）。
  // 启动阶段（首次回调）同样用左上角锚定，理由同 config 恢复 effect。
  const startupAnchorRef = useRef(true);
  const handleModelMetrics = useCallback(
    async (metrics: { aspectRatio: number }) => {
      const ratio =
        Number.isFinite(metrics.aspectRatio) && metrics.aspectRatio > 0
          ? metrics.aspectRatio
          : DEFAULT_ASPECT_RATIO;
      setAspectRatio(ratio);
      const anchor = startupAnchorRef.current ? "topleft" : "center";
      startupAnchorRef.current = false;
      await resizeTo(ratio, scaleRef.current, anchor);
    },
    [resizeTo],
  );

  // GIF 伙伴无 Live2D 句柄；切到 GIF 时清空，防 dsh 动作触发已卸载的模型。
  useEffect(() => {
    if (stage.isGif) modelRef.current = null;
  }, [stage.isGif]);

  // dsh 任务事件：气泡由 EventBubble 渲染，这里联动触发模型动作。
  useEffect(() => {
    const unlisten = onDshSpeak(({ event }) => {
      const model = modelRef.current;
      if (!model) return;
      // motionManager 类型上非空，但运行时缺组/初始化异常时可能缺失，防御跳过。
      if (!model.internalModel.motionManager) return;
      const groups = Object.keys(
        (model.internalModel.motionManager.definitions ?? {}) as Record<string, unknown>,
      );
      const group = pickMotionGroup(groups, event.type);
      if (!group) return;
      // FORCE 优先级（3）：打断 idle/在播动作，同 previewManager 的 startMotion 语义
      void model.internalModel.motionManager.startMotion(group, 0, 3);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 监听窗口移动：拖动停止（debounce）后把逻辑像素坐标写回 settings，供下次启动恢复。
  useEffect(() => {
    const win = getCurrentWindow();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unlisten = win.onMoved(({ payload }) => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        void (async () => {
          const factor = await win.scaleFactor();
          const x = Math.round(payload.x / factor);
          const y = Math.round(payload.y / factor);
          await api.saveCompanionPosition({ x, y });
        })();
      }, 300);
    });
    return () => {
      clearTimeout(timer);
      void unlisten.then((fn) => fn());
    };
  }, []);

  // cmd/ctrl + 滚轮：连续缩放（节流约 60ms，阻止默认滚动）。
  // 置底（back）为点穿背景，不挂滚轮监听（原生层本已吞掉鼠标事件，这里是防御）。
  useEffect(() => {
    if (layer === "back") return;
    const el = containerRef.current;
    if (!el) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onWheel = (e: WheelEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      e.preventDefault();
      if (timer) return;
      // 步长随滚轮实际位移变化：鼠标一格 deltaY≈100 → 1.1×；小幅位移 = 微调，缩放更平滑。
      const next = scaleRef.current * WHEEL_SCALE_STEP ** (e.deltaY / 100);
      if (next < SCALE_MIN || next > SCALE_MAX) return;
      timer = setTimeout(() => {
        timer = undefined;
      }, 60);
      void applyScaleRef.current(next);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      el.removeEventListener("wheel", onWheel);
      if (timer) clearTimeout(timer);
    };
  }, [layer]);

  return (
    <div
      ref={containerRef}
      role="application"
      className="relative h-screen w-screen select-none overflow-hidden bg-transparent"
      onMouseDown={(e) => {
        if (e.button !== 0 || layer === "back" || locked) return;
        if (dragMode === "modifier" && !(e.metaKey || e.ctrlKey)) return;
        void getCurrentWindow().startDragging();
      }}
      onContextMenu={(e) => {
        if (layer === "back") return;
        e.preventDefault();
        void api.showCompanionMenu({ x: e.clientX, y: e.clientY });
      }}
    >
      {/* 透明度只作用于模型本身，语音状态点保持不透明 */}
      <div style={{ opacity }}>
        {/* 顶部预留 BUBBLE_STRIP 给事件 deck；内层 relative 让 PropsLayer 的画布
            坐标映射（absolute left/top = layout.x/y）锚定到下移后的舞台原点。 */}
        <div className="relative" style={{ marginTop: BUBBLE_STRIP }}>
          {stage.isGif ? (
            <GifStage
              url={stage.url}
              width={size.width}
              height={size.height - BUBBLE_STRIP}
              onModelMetrics={handleModelMetrics}
            />
          ) : (
            <Live2dStage
              ref={stageRef}
              modelUrl={stage.url}
              width={size.width}
              height={size.height - BUBBLE_STRIP}
              onModelMetrics={handleModelMetrics}
              onLayout={setModelLayout}
              onModelLoaded={(m) => {
                modelRef.current = m;
              }}
            />
          )}
          {/* BongoCat 道具层：键盘背景 + 爪子按键贴图（仅 BongoCat 伙伴有 props） */}
          <PropsLayer
            layout={modelLayout}
            backgroundUrl={props?.background ? toAssetUrl(props.background) : null}
            pressedKeys={pressedKeys}
          />
        </div>
      </div>
      {/* dsh 任务事件气泡（pointer-events-none，不挡拖动/右键） */}
      <EventBubble />
      {/* AI 伙伴回复气泡已迁移至独立 bubble 窗口（components/bubble/BubbleRoot） */}
      {/* 置底为纯背景装饰，不显示语音状态点 */}
      {layer === "front" && (
        <span className="absolute right-2 top-2">
          <VoiceStatusDot phase={voice.phase} running={voice.running} />
        </span>
      )}
    </div>
  );
}
