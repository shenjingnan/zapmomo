import type { ModelLayout } from "@/components/live2d/Live2dStage";

/** 一个按键贴图（爪子按在某键上的预渲染图）。 */
export interface PressedKey {
  key: string;
  url: string;
  hand: "left" | "right";
}

interface PropsLayerProps {
  /** 模型画布→屏幕映射（模型未加载为 null，此时不渲染）。 */
  layout: ModelLayout | null;
  /** 键盘背景图 asset:// URL（props 存在即常显，同 BongoCat 语义）。 */
  backgroundUrl: string | null;
  /** 正在按下的按键贴图。 */
  pressedKeys: PressedKey[];
}

/**
 * BongoCat 道具层：键盘背景 + 爪子按键贴图，叠在 Live2D canvas 之上。
 *
 * 所有贴图都是「整画布尺寸」的预渲染图（BongoCat 约定），铺满画布映射盒即可对齐
 * 模型。盒子尺寸来自 `layout`（画布→屏幕映射），对任意窗口宽高比都正确；`pointer-events:
 * none` 不挡拖拽/右键/滚轮。
 */
export function PropsLayer({ layout, backgroundUrl, pressedKeys }: PropsLayerProps) {
  if (!layout) {
    return null;
  }
  return (
    <div
      className="pointer-events-none absolute select-none overflow-hidden"
      style={{
        left: layout.x,
        top: layout.y,
        width: layout.canvasWidth * layout.scale,
        height: layout.canvasHeight * layout.scale,
      }}
    >
      {backgroundUrl && (
        <img src={backgroundUrl} alt="" draggable={false} className="h-full w-full" />
      )}
      {pressedKeys.map((p) => (
        <img
          key={p.key}
          src={p.url}
          alt=""
          draggable={false}
          className="absolute inset-0 h-full w-full"
        />
      ))}
    </div>
  );
}
