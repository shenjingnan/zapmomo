import { describe, expect, it } from "vitest";
import type { ModelLayout } from "@/components/live2d/Live2dStage";
import { gifContainRect, toWindowRects } from "./companionHitRegion";

const layout: ModelLayout = { x: 10, y: 20, scale: 2, canvasWidth: 100, canvasHeight: 200 };

describe("toWindowRects", () => {
  it("stage 坐标 = layout.position + 局部坐标 × scale，再下移顶部预留条", () => {
    expect(toWindowRects([{ x: 1, y: 2, width: 3, height: 4 }], layout, 72)).toEqual([
      { x: 12, y: 96, width: 6, height: 8 },
    ]);
  });

  it("多矩形逐一映射，bubbleStrip 可变（与后端 COMPANION_BUBBLE_STRIP 对齐）", () => {
    const rects = [
      { x: 0, y: 0, width: 10, height: 10 },
      { x: 5, y: 5, width: 5, height: 5 },
    ];
    expect(toWindowRects(rects, layout, 0)).toEqual([
      { x: 10, y: 20, width: 20, height: 20 },
      { x: 20, y: 30, width: 10, height: 10 },
    ]);
  });
});

describe("gifContainRect", () => {
  it("宽图（比例大于容器）：按宽度撑满，上下 letterbox", () => {
    expect(gifContainRect({ width: 100, height: 100 }, 2)).toEqual({
      x: 0,
      y: 25,
      width: 100,
      height: 50,
    });
  });

  it("高图（比例小于容器）：按高度撑满，左右 letterbox", () => {
    expect(gifContainRect({ width: 100, height: 100 }, 0.5)).toEqual({
      x: 25,
      y: 0,
      width: 50,
      height: 100,
    });
  });

  it("与容器同比例时铺满整盒", () => {
    expect(gifContainRect({ width: 100, height: 50 }, 2)).toEqual({
      x: 0,
      y: 0,
      width: 100,
      height: 50,
    });
  });

  it("非法宽高比（0/负数/NaN）或空容器返回 null（图未加载不上报）", () => {
    expect(gifContainRect({ width: 100, height: 100 }, 0)).toBeNull();
    expect(gifContainRect({ width: 100, height: 100 }, -1)).toBeNull();
    expect(gifContainRect({ width: 100, height: 100 }, NaN)).toBeNull();
    expect(gifContainRect({ width: 0, height: 100 }, 1)).toBeNull();
    expect(gifContainRect({ width: 100, height: 0 }, 1)).toBeNull();
  });
});
