import { describe, expect, it, vi } from "vitest";
import { computeModelBounds, computeModelHitRects, layoutModel } from "./modelLayout";

type FakeRect = { x: number; y: number; width: number; height: number };

/**
 * 假造一个最小 Cubism4 内部模型：originalWidth/Height=100×200、局部宽高=200×400
 * （layout 缩放因子 sx=sy=2），两个 drawable 合并包围盒。
 *
 * `opacities` 可选：提供时挂 coreModel.getDrawableOpacity（按 drawable 顺序取），
 * 不提供时不挂 coreModel（验证「取不到 opacity 视为可见」的兜底）。
 */
function makeModel(rects: Record<string, FakeRect>, opacities?: Record<string, number>) {
  const ids = Object.keys(rects);
  const internalModel: Record<string, unknown> = {
    width: 200,
    height: 400,
    originalWidth: 100,
    originalHeight: 200,
    getDrawableIDs: () => ids,
    getDrawableIndex: (id: string) => ids.indexOf(id),
    getDrawableBounds: (index: number) => rects[ids[index]],
  };
  if (opacities) {
    internalModel.coreModel = {
      getDrawableOpacity: (index: number) => opacities[ids[index]],
    };
  }
  return {
    internalModel,
    scale: { set: vi.fn() },
    anchor: { set: vi.fn() },
    position: { set: vi.fn() },
  };
}

describe("computeModelBounds", () => {
  it("合并所有 drawable 边界并按 layout 缩放因子映射到模型局部坐标", () => {
    // a: (10,20)-(40,60)，b: (50,60)-(70,80) → 原始空间合并 (10,20)-(70,80)，×2 → 120×120
    const model = makeModel({
      a: { x: 10, y: 20, width: 30, height: 40 },
      b: { x: 50, y: 60, width: 20, height: 20 },
    });

    expect(computeModelBounds(model as never)).toEqual({
      cx: 80, // (10+70)/2*2
      cy: 100, // (20+80)/2*2
      width: 120, // (70-10)*2
      height: 120, // (80-20)*2
    });
  });

  it("无 drawable 时返回非法包围盒（NaN/Infinity），供上层跳过布局", () => {
    const model = makeModel({});
    const bounds = computeModelBounds(model as never);
    expect(Number.isFinite(bounds.width)).toBe(false);
    expect(Number.isFinite(bounds.height)).toBe(false);
  });
});

describe("layoutModel", () => {
  it("按包围盒 contain 撑满画布并居中：scale=min(w/bw,h/bh)，position 居中偏移", () => {
    const model = makeModel({
      a: { x: 10, y: 20, width: 30, height: 40 },
      b: { x: 50, y: 60, width: 20, height: 20 },
    });
    // bounds=120×120，画布 240×240 → scale=2；modelScale=0.8 → 1.6
    layoutModel(model as never, 240, 240, 0.8);

    expect(model.scale.set).toHaveBeenCalledWith(1.6);
    expect(model.anchor.set).toHaveBeenCalledWith(0, 0);
    // x = 240/2 - 80*1.6 = -8，y = 240/2 - 100*1.6 = -40
    expect(model.position.set).toHaveBeenCalledWith(-8, -40);
  });

  it("不传 modelScale 默认 1（完整 contain 填充）", () => {
    const model = makeModel({ a: { x: 0, y: 0, width: 100, height: 200 } });
    // bounds=200×400，画布 200×400 → scale=min(1,1)=1
    layoutModel(model as never, 200, 400);

    expect(model.scale.set).toHaveBeenCalledWith(1);
  });

  it("包围盒非法（空 drawable 等）时跳过布局，保持默认状态", () => {
    const model = makeModel({});
    layoutModel(model as never, 240, 240);

    expect(model.scale.set).not.toHaveBeenCalled();
    expect(model.position.set).not.toHaveBeenCalled();
  });
});

describe("computeModelHitRects", () => {
  it("逐 drawable 输出模型局部坐标矩形（×sx/sy）并按面积降序", () => {
    // a 面积 1200、b 面积 400 → a 在前；局部坐标 = 原始坐标 × 2。
    const model = makeModel({
      a: { x: 10, y: 20, width: 30, height: 40 },
      b: { x: 50, y: 60, width: 20, height: 20 },
    });

    expect(computeModelHitRects(model as never)).toEqual([
      { x: 20, y: 40, width: 60, height: 80 },
      { x: 100, y: 120, width: 40, height: 40 },
    ]);
  });

  it("跳过 NaN bounds（moc3 v5 惰性填充 mesh）与零尺寸 drawable", () => {
    const model = makeModel({
      bad: { x: NaN, y: NaN, width: NaN, height: NaN },
      zero: { x: 0, y: 0, width: 0, height: 0 },
      keep: { x: 0, y: 0, width: 10, height: 10 },
    });

    expect(computeModelHitRects(model as never)).toEqual([{ x: 0, y: 0, width: 20, height: 20 }]);
  });

  it("跳过 opacity < 0.05 的隐藏部件（换装/水印不产生幽灵命中区）", () => {
    const model = makeModel(
      {
        hidden: { x: 0, y: 0, width: 500, height: 500 },
        visible: { x: 0, y: 0, width: 10, height: 10 },
      },
      { hidden: 0.01, visible: 0.5 },
    );

    expect(computeModelHitRects(model as never)).toEqual([{ x: 0, y: 0, width: 20, height: 20 }]);
  });

  it("全部 drawable 隐藏时返回空数组", () => {
    const model = makeModel({ a: { x: 0, y: 0, width: 10, height: 10 } }, { a: 0 });

    expect(computeModelHitRects(model as never)).toEqual([]);
  });

  it("取不到 coreModel（测试桩/旧 core）时视为全部可见", () => {
    const model = makeModel({ a: { x: 0, y: 0, width: 10, height: 10 } });

    expect(computeModelHitRects(model as never)).toEqual([{ x: 0, y: 0, width: 20, height: 20 }]);
  });

  it("按面积降序截断到 maxRects", () => {
    const model = makeModel({
      big: { x: 0, y: 0, width: 30, height: 30 },
      mid: { x: 0, y: 0, width: 20, height: 20 },
      small: { x: 0, y: 0, width: 10, height: 10 },
    });

    expect(computeModelHitRects(model as never, 2)).toEqual([
      { x: 0, y: 0, width: 60, height: 60 },
      { x: 0, y: 0, width: 40, height: 40 },
    ]);
  });
});
