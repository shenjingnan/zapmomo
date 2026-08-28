import type { Live2DModel } from "pixi-live2d-display/cubism4";

/** 角色真实包围盒（模型局部坐标），用于居中 + 等比缩放。 */
export interface ModelBounds {
  cx: number;
  cy: number;
  width: number;
  height: number;
}

/** 单个矩形（模型局部 / stage 坐标），几何形状与协议类型 `HitRect` 相同。 */
export interface ModelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 智能穿透上报的部件数上限：按面积降序截断，限制 payload 与 Rust 点查成本。 */
export const MAX_HIT_RECTS = 32;

/** drawable 透明度低于该值视为隐藏（换装/水印部件不产生幽灵命中区）。 */
export const DRAWABLE_OPACITY_MIN = 0.05;

/**
 * 遍历所有 drawable，合并边界得到角色真实最小包围盒（AABB）。
 *
 * `getDrawableBounds` 返回原始画布空间（originalWidth×originalHeight），
 * 乘以 layout 缩放因子（internalModel.width / originalWidth）映射到模型局部坐标。
 *
 * 跳过顶点未填充的 drawable（bounds 含 undefined/NaN）：Cubism 5（moc3 v5）模型中
 * 存在初始隐藏、core 惰性填充顶点的 mesh（如水印类部件），其 bounds 为
 * `{x: undefined, y: undefined, width: NaN, height: NaN}`；不跳过会让合并包围盒
 * 变成 NaN，`layoutModel` 判定非法后跳过布局，模型卡在 2500×2500 画布原点，
 * 窗口内只露出画布角落一小块（表现为「模型不显示」）。
 */
export function computeModelBounds(model: Live2DModel): ModelBounds {
  const im = model.internalModel;
  const sx = im.width / im.originalWidth;
  const sy = im.height / im.originalHeight;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const id of im.getDrawableIDs()) {
    const b = im.getDrawableBounds(im.getDrawableIndex(id));
    if (
      !Number.isFinite(b.x) ||
      !Number.isFinite(b.y) ||
      !Number.isFinite(b.width) ||
      !Number.isFinite(b.height)
    ) {
      continue;
    }
    minX = Math.min(minX, b.x);
    minY = Math.min(minY, b.y);
    maxX = Math.max(maxX, b.x + b.width);
    maxY = Math.max(maxY, b.y + b.height);
  }
  return {
    cx: ((minX + maxX) / 2) * sx,
    cy: ((minY + maxY) / 2) * sy,
    width: (maxX - minX) * sx,
    height: (maxY - minY) * sy,
  };
}

/**
 * 让角色真实包围盒在画布内 contain 撑满并居中（而非基于画布尺寸）。
 * `modelScale` 额外乘一个等比系数（<1 缩小），用于概览等场景让模型小一圈。
 * 若包围盒非法（空 drawable 等），跳过布局，保持模型默认状态。
 */
export function layoutModel(model: Live2DModel, width: number, height: number, modelScale = 1) {
  const b = computeModelBounds(model);
  if (!Number.isFinite(b.width) || !Number.isFinite(b.height) || b.width <= 0 || b.height <= 0) {
    return;
  }
  const scale = Math.min(width / b.width, height / b.height) * modelScale;
  model.scale.set(scale);
  model.anchor.set(0, 0);
  model.position.set(width / 2 - b.cx * scale, height / 2 - b.cy * scale);
}

/**
 * 逐 drawable 输出可见包围盒（模型局部坐标，已乘 layout 缩放因子 sx/sy），
 * 供智能穿透把角色画面映射为窗口内的命中矩形集（见 SMART_CLICK_THROUGH_DESIGN.md）。
 *
 * 复用 [`computeModelBounds`] 的 NaN 防线（moc3 v5 惰性填充 mesh）；另按
 * `coreModel.getDrawableOpacity` 过滤隐藏部件——不用 `DrawableFlags.IsVisible`，
 * 眨眼等瞬时隐藏会把眼睛的 rect 抖掉；取不到 opacity（测试桩/旧 core）视为可见。
 *
 * 结果按面积降序截断到 `maxRects`：保留视觉上最大的部件，限制上报体积。
 */
export function computeModelHitRects(model: Live2DModel, maxRects = MAX_HIT_RECTS): ModelRect[] {
  const im = model.internalModel;
  const sx = im.width / im.originalWidth;
  const sy = im.height / im.originalHeight;
  const rects: ModelRect[] = [];
  for (const id of im.getDrawableIDs()) {
    const index = im.getDrawableIndex(id);
    const b = im.getDrawableBounds(index);
    if (
      !Number.isFinite(b.x) ||
      !Number.isFinite(b.y) ||
      !Number.isFinite(b.width) ||
      !Number.isFinite(b.height)
    ) {
      continue;
    }
    if (b.width <= 0 || b.height <= 0) {
      continue;
    }
    // coreModel 静态类型是 object（跨运行时基类）；窄化出本次需要的可选方法，
    // 取不到（测试桩/旧 core）视为可见。
    const core = im.coreModel as { getDrawableOpacity?(index: number): number } | undefined;
    const opacity = core?.getDrawableOpacity?.(index) ?? 1;
    if (opacity < DRAWABLE_OPACITY_MIN) {
      continue;
    }
    rects.push({ x: b.x * sx, y: b.y * sy, width: b.width * sx, height: b.height * sy });
  }
  rects.sort((a, b) => b.width * b.height - a.width * a.height);
  return rects.length > maxRects ? rects.slice(0, maxRects) : rects;
}
