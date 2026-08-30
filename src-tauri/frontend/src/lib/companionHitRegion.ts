import type { ModelLayout } from "@/components/live2d/Live2dStage";
import type { ModelRect } from "@/components/live2d/modelLayout";
import type { HitRect } from "@/types/tauri";

/**
 * 智能穿透的命中区域装配（纯几何，无 pixi/tauri 依赖）。
 *
 * 前端把「角色画面」换算成窗口内逻辑像素矩形集上报给 Rust
 * （`set_companion_hit_region`），Rust 轮询全局光标做点查动态切换穿透，
 * 坐标契约见 docs/plans/2026-08-28-companion-smart-click-through-design.md §3/§6。
 */

/** 上报去抖：窗口尺寸/布局连续变化时合并，避免 resize 期间高频 invoke。 */
export const HIT_REGION_DEBOUNCE_MS = 150;

/**
 * stage 坐标矩形 → 窗口逻辑坐标矩形。
 *
 * `layout` 来自 Live2dStage 的 `onLayout`（model.position / model.scale），
 * 与 `layoutModel` 的布局公式一致：stage 坐标 = model.position + 局部坐标 × scale；
 * 再统一下移顶部预留条（模型区因 marginTop 下移，stage 原点在窗口 (0, bubbleStrip) 处，
 * `bubbleStrip` 由调用方传 CompanionRoot 的 BUBBLE_STRIP，与后端 COMPANION_BUBBLE_STRIP 一致）。
 *
 * 注意不可沿用 PropsLayer 的 `canvasWidth × scale` 约定：ModelLayout.canvasWidth
 * 是 originalWidth（未乘 sx），而 `computeModelHitRects` 输出的局部坐标已乘 sx/sy。
 */
export function toWindowRects(
  rects: ModelRect[],
  layout: ModelLayout,
  bubbleStrip: number,
): HitRect[] {
  return rects.map((r) => ({
    x: layout.x + r.x * layout.scale,
    y: layout.y + bubbleStrip + r.y * layout.scale,
    width: r.width * layout.scale,
    height: r.height * layout.scale,
  }));
}

/**
 * object-contain 的实际显示区（stage 坐标，居中）。
 *
 * GIF/立绘以 `<img object-contain>` 呈现在容器盒内，容器盒的 letterbox 空白
 * 不该挡点击：由容器尺寸 + 图像宽高比反推实际显示矩形。宽高比非法（≤0/NaN，
 * 图未加载）返回 null，调用方跳过本次上报（保持 Rust 侧旧值/未就绪语义）。
 */
export function gifContainRect(
  box: { width: number; height: number },
  aspectRatio: number,
): ModelRect | null {
  if (!Number.isFinite(aspectRatio) || aspectRatio <= 0 || box.width <= 0 || box.height <= 0) {
    return null;
  }
  const boxRatio = box.width / box.height;
  // 容器比图更宽 → 图按高度撑满，左右 letterbox；否则按宽度撑满，上下 letterbox。
  const width = boxRatio > aspectRatio ? box.height * aspectRatio : box.width;
  const height = boxRatio > aspectRatio ? box.height : box.width / aspectRatio;
  return {
    x: (box.width - width) / 2,
    y: (box.height - height) / 2,
    width,
    height,
  };
}
