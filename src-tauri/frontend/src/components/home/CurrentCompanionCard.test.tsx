import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CompanionModelInfo } from "@/types/tauri";
import { CurrentCompanionCard } from "./CurrentCompanionCard";

const { configState } = vi.hoisted(() => ({
  /** get_live2d_config 返回的有效缩放/透明度（模拟「active 伙伴私有 ?? 全局」合并结果）。 */
  configState: { windowScale: 1.0, windowOpacity: 1.0 },
}));

vi.mock("@/lib/tauri", () => ({
  api: {
    getLive2dConfig: vi.fn(async () => ({
      window_scale: configState.windowScale,
      window_opacity: configState.windowOpacity,
    })),
    setCompanionScale: vi.fn(async () => undefined),
    setCompanionOpacity: vi.fn(async () => undefined),
  },
  onCompanionScaleChanged: vi.fn(() => Promise.resolve(() => {})),
  onCompanionOpacityChanged: vi.fn(() => Promise.resolve(() => {})),
  toAssetUrl: vi.fn((p: string) => `asset://localhost/${p}`),
}));

// SharedLive2dStage 依赖 pixi / WebGL，jsdom 无法运行；预览容器量测（ResizeObserver）
// 在 vitest.setup 是空桩、尺寸保持 0，本组件不会真正渲染 stage。
vi.mock("@/components/live2d/SharedLive2dStage", () => ({
  SharedLive2dStage: () => <div data-testid="live2d-stage" />,
}));

function model(id: string, name: string): CompanionModelInfo {
  return {
    id,
    name,
    source_path: `/src/${name}`,
    model_dir: `/zap/.zapmomo/companions/${id}`,
    model_file: `/zap/.zapmomo/companions/${id}/${name}.model3.json`,
    format: "cubism3",
    imported_at: "2026-01-01T00:00:00Z",
    valid: true,
    cover_image: null,
    has_persona: false,
    voice_id: null,
    voice_source: null,
    has_voice: false,
    has_original_voice: false,
    wake_word: null,
    wake_word_effective: "",
    wake_word_ok: true,
    welcome_text: null,
    welcome_text_effective: "",
    welcome_ready: true,
  };
}

const MODEL_A = model("companion-aaaa", "大月下");
const MODEL_B = model("companion-bbbb", "星语");

beforeEach(() => {
  configState.windowScale = 1.0;
  configState.windowOpacity = 1.0;
});

describe("CurrentCompanionCard（尺寸/透明度控制）", () => {
  it("初始显示当前伙伴的有效缩放", async () => {
    configState.windowScale = 0.5;
    render(<CurrentCompanionCard companion={MODEL_A} loading={false} error={null} />);
    expect(await screen.findByText("50%")).toBeInTheDocument();
  });

  it("切换当前伙伴后尺寸滑杆刷新为新伙伴的缩放", async () => {
    configState.windowScale = 0.5;
    const { rerender } = render(
      <CurrentCompanionCard companion={MODEL_A} loading={false} error={null} />,
    );
    expect(await screen.findByText("50%")).toBeInTheDocument();

    // 后端视角：B 的有效缩放 1.5 → 切到 B 后滑杆刷新为 150%。
    configState.windowScale = 1.5;
    rerender(<CurrentCompanionCard companion={MODEL_B} loading={false} error={null} />);
    expect(await screen.findByText("150%")).toBeInTheDocument();
    expect(screen.queryByText("50%")).not.toBeInTheDocument();
  });
});
