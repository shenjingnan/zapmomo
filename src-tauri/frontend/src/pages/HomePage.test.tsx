import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ToastProvider } from "@/components/ui/toast";
import type { RuntimeState } from "@/providers/RuntimeContext";
import type { CompanionLibraryView, CompanionModelInfo, VoiceSessionPhase } from "@/types/tauri";
import { HomePage } from "./HomePage";

const { invokeMock, state } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  // 可变 runtime 快照：单个用例按需替换 kws/asr/llm/tts 切片（贴近真实 RuntimeState）。
  state: { runtime: null as RuntimeState | null },
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

// 概览页读取全局 runtime；直接 mock context 模块比挂载完整 App 轻得多。
vi.mock("@/providers/RuntimeContext", () => ({
  useRuntime: () => state.runtime,
}));

// SharedLive2dStage 依赖 pixi / WebGL，jsdom 无法运行；且 ResizeObserver 在 jsdom 是空桩、
// 预览容器尺寸保持 0，组件不会真正渲染 stage，这里 mock 掉避免模块加载副作用。
vi.mock("@/components/live2d/SharedLive2dStage", () => ({
  SharedLive2dStage: () => <div data-testid="live2d-stage" />,
}));

function model(id: string, name: string, valid = true): CompanionModelInfo {
  return {
    id,
    name,
    source_path: `/src/${name}`,
    model_dir: `/zap/.zapmomo/companions/${id}`,
    model_file: `/zap/.zapmomo/companions/${id}/${name}.model3.json`,
    format: "cubism3",
    imported_at: "2026-01-01T00:00:00Z",
    valid,
    cover_image: null,
    has_persona: false,
    voice_id: null,
    voice_source: null,
    has_voice: false,
  };
}

const MODEL_A = model("companion-aaaa", "大月下");

/** 可变伙伴库快照（模拟后端 list_companions）。 */
let library: CompanionLibraryView;

// ---- runtime 切片工厂：只填概览推导真正读取的字段 ----

function makeKws(o?: {
  enabled?: boolean;
  modelsPresent?: boolean;
  isListening?: boolean;
  error?: string | null;
}) {
  return {
    config: {
      config: { enabled: o?.enabled ?? false, models_present: o?.modelsPresent ?? false },
      error: null,
    },
    listening: { isListening: o?.isListening ?? false, pending: false, error: o?.error ?? null },
  };
}

function makeAsr(o?: {
  enabled?: boolean;
  modelsPresent?: boolean;
  isListening?: boolean;
  pending?: boolean;
  error?: string | null;
}) {
  return {
    config: {
      config: { enabled: o?.enabled ?? false, models_present: o?.modelsPresent ?? false },
      error: null,
    },
    listening: {
      isListening: o?.isListening ?? false,
      pending: o?.pending ?? false,
      error: o?.error ?? null,
    },
  };
}

function makeLlm(o?: {
  modelsPresent?: boolean;
  ready?: boolean;
  loading?: boolean;
  generating?: boolean;
  error?: string | null;
}) {
  return {
    // 远程连接语义：modelsPresent 映射为「已填写 API 地址 + 模型名」
    config: o?.modelsPresent
      ? { base_url: "https://open.bigmodel.cn/api/paas/v4", model: "glm-4.7-flash" }
      : { base_url: null, model: null },
    ready: o?.ready ?? false,
    loading: o?.loading ?? false,
    generating: o?.generating ?? false,
    error: o?.error ?? null,
  };
}

function makeTts(o?: {
  modelsPresent?: boolean;
  enabled?: boolean;
  synthesizing?: boolean;
  configError?: string | null;
}) {
  return {
    config: {
      enabled: o?.enabled ?? true,
      models_present: o?.modelsPresent ?? false,
    },
    configError: o?.configError ?? null,
    synthesizing: o?.synthesizing ?? false,
  };
}

function makeVoice(o?: { running?: boolean; phase?: VoiceSessionPhase; error?: string | null }) {
  return {
    running: o?.running ?? false,
    phase: o?.phase ?? "idle",
    error: o?.error ?? null,
  };
}

function makeSpeaker(o?: { error?: string | null; modelPresent?: boolean; enabled?: boolean }) {
  return {
    config: {
      config: {
        enabled: o?.enabled ?? false,
        model_present: o?.modelPresent ?? true,
      },
      error: o?.error ?? null,
      refresh: vi.fn(),
    },
  };
}

function makeRuntime(
  overrides?: Partial<{
    kws: ReturnType<typeof makeKws>;
    asr: ReturnType<typeof makeAsr>;
    llm: ReturnType<typeof makeLlm>;
    tts: ReturnType<typeof makeTts>;
    speaker: ReturnType<typeof makeSpeaker>;
    voice: ReturnType<typeof makeVoice>;
  }>,
): RuntimeState {
  return {
    kws: overrides?.kws ?? makeKws(),
    asr: overrides?.asr ?? makeAsr(),
    llm: overrides?.llm ?? makeLlm(),
    tts: overrides?.tts ?? makeTts(),
    speaker: overrides?.speaker ?? makeSpeaker(),
    voice: overrides?.voice ?? makeVoice(),
  } as unknown as RuntimeState;
}

function renderHome() {
  return render(
    <ToastProvider>
      <MemoryRouter>
        <HomePage />
      </MemoryRouter>
    </ToastProvider>,
  );
}

beforeEach(() => {
  invokeMock.mockReset();
  library = { models: [], active_model_id: null };
  state.runtime = makeRuntime();

  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_companions":
        return Promise.resolve(library);
      case "get_live2d_config":
        return Promise.resolve({
          model_dir: null,
          model_file: null,
          format: null,
          models_present: false,
          window_scale: 1.0,
          window_opacity: 1.0,
          settings_path: "/zap/.zapmomo/settings.toml",
        });
      default:
        return Promise.resolve(undefined);
    }
  });
});

describe("HomePage 概览", () => {
  it("渲染当前伙伴：顶部名称、使用中徽标与尺寸/透明度滑块", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    renderHome();

    expect(await screen.findByText("使用中")).toBeInTheDocument();
    expect(screen.getByText(MODEL_A.name)).toBeInTheDocument();
    // 初始从 get_live2d_config 读到 window_scale=1.0 / window_opacity=1.0 → 都是 100%。
    expect(await screen.findAllByText("100%")).toHaveLength(2);
    expect(screen.getAllByRole("slider")).toHaveLength(2);
    // /chat 仍是占位页：概览不提供「开始对话」入口。
    expect(screen.queryByRole("button", { name: "开始对话" })).not.toBeInTheDocument();
  });

  it("拖动桌宠尺寸滑块调用 set_companion_scale", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderHome();

    await screen.findByText("使用中");
    const slider = screen.getByRole("slider", { name: "尺寸" });
    slider.focus();
    await user.keyboard("{ArrowRight}");

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_scale", {
        scale: expect.any(Number),
      });
    });
  });

  it("拖动透明度滑块调用 set_companion_opacity", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderHome();

    await screen.findByText("使用中");
    const slider = screen.getByRole("slider", { name: "透明度" });
    slider.focus();
    await user.keyboard("{ArrowLeft}"); // 100 → 95

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_opacity", {
        opacity: expect.any(Number),
      });
    });
  });

  it("空库：卡内空态，无滑块", async () => {
    renderHome();

    expect(await screen.findByText("尚未选择桌面伙伴")).toBeInTheDocument();
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();
  });

  it("LLM：已填写连接配置但未连接 → 未连接（配置存在 ≠ 已连接）", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    state.runtime = makeRuntime({ llm: makeLlm({ modelsPresent: true, ready: false }) });
    renderHome();

    const capabilities = await screen.findByLabelText("AI 能力");
    expect(await within(capabilities).findByText("未连接")).toBeInTheDocument();
  });

  it("LLM 出错时能力卡显示异常", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    state.runtime = makeRuntime({
      llm: makeLlm({ modelsPresent: true, ready: false, error: "engine boom" }),
    });
    renderHome();

    const capabilities = await screen.findByLabelText("AI 能力");
    expect(await within(capabilities).findByText("异常")).toBeInTheDocument();
  });

  it("KWS：enabled 且模型在但未监听 → 已就绪（启动自动监听失败会静默降级）", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    state.runtime = makeRuntime({ kws: makeKws({ enabled: true, modelsPresent: true }) });
    renderHome();

    const capabilities = await screen.findByLabelText("AI 能力");
    expect(await within(capabilities).findByText("已就绪")).toBeInTheDocument();
  });

  it("KWS：模型在但未启用 → 未启用；TTS 主动关闭 → 已关闭", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    state.runtime = makeRuntime({
      kws: makeKws({ enabled: false, modelsPresent: true }),
      tts: makeTts({ modelsPresent: true, enabled: false }),
      // 声纹默认未启用：会与断言的「未启用」文案撞车，这里显式启用
      speaker: makeSpeaker({ enabled: true, modelPresent: true }),
    });
    renderHome();

    const capabilities = await screen.findByLabelText("AI 能力");
    expect(await within(capabilities).findByText("未启用")).toBeInTheDocument();
    expect(within(capabilities).getByText("已关闭")).toBeInTheDocument();
  });

  it("ASR：enabled 且模型在但未识别 → 已就绪（读取持久化 enabled）", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    state.runtime = makeRuntime({ asr: makeAsr({ enabled: true, modelsPresent: true }) });
    renderHome();

    const capabilities = await screen.findByLabelText("AI 能力");
    expect(await within(capabilities).findByText("已就绪")).toBeInTheDocument();
  });

  it("ASR：模型在但未启用 → 未启用", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    state.runtime = makeRuntime({
      asr: makeAsr({ enabled: false, modelsPresent: true }),
      // 声纹默认未启用：会与断言的「未启用」文案撞车，这里显式启用
      speaker: makeSpeaker({ enabled: true, modelPresent: true }),
    });
    renderHome();

    const capabilities = await screen.findByLabelText("AI 能力");
    expect(await within(capabilities).findByText("未启用")).toBeInTheDocument();
  });

  it("伙伴模型不可用：预览提示文案 + 名称旁「模型不可用」", async () => {
    const broken = model("companion-broken", "莉娅", false);
    library = { models: [broken], active_model_id: broken.id };
    renderHome();

    expect(await screen.findByText("无法加载该 Live2D 模型")).toBeInTheDocument();
    expect(screen.getByText("模型不可用")).toBeInTheDocument();
  });

  it("AI 能力卡是纯展示：区域内没有任何导航链接；页头为中性副标题", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    renderHome();

    const capabilities = await screen.findByLabelText("AI 能力");
    expect(within(capabilities).queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText("查看你的桌面伙伴与 AI 能力状态")).toBeInTheDocument();
  });
});
