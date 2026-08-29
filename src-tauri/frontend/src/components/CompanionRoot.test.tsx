import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CompanionDragMode } from "@/types/tauri";
import { CompanionRoot } from "./CompanionRoot";

const { invokeMock, startDraggingMock, setSizeMock, setPositionMock, configState, listenHandlers } =
  vi.hoisted(() => ({
    invokeMock: vi.fn(),
    startDraggingMock: vi.fn(),
    /** resizeTo 的 setSize 是 config 完全应用（含 setLocked）后的最后一步，作等待信号。 */
    setSizeMock: vi.fn(async () => undefined),
    /** 布局恢复/中心锚定的 setPosition（代码链式 .catch，必须返回 Promise）。 */
    setPositionMock: vi.fn(() => Promise.resolve()),
    /** get_live2d_config 的 locked / drag_mode / 模型字段覆盖值（null = 后端未返回该字段）。 */
    configState: {
      locked: null as boolean | null,
      dragMode: null as CompanionDragMode | null,
      modelFile: null as string | null,
      modelDir: null as string | null,
      format: null as string | null,
    },
    /** 按事件名捕获 listen 回调，供测试主动推送后端事件。 */
    listenHandlers: {} as Record<string, (payload: unknown) => void>,
  }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, cb: (e: { payload: unknown }) => void) => {
    listenHandlers[name] = (payload: unknown) => cb({ payload });
    return Promise.resolve(() => {});
  }),
}));

// CompanionRoot 顶层同时 import 了 LogicalPosition/LogicalSize，缺了模块加载即失败。
// getCurrentWindow 返回共享方法集（setSize 共享才能作为 config 已应用的断言信号）。
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    // setPosition 代码链式 .catch，必须返回 Promise。
    startDragging: startDraggingMock,
    onMoved: vi.fn(() => Promise.resolve(() => {})),
    scaleFactor: vi.fn(async () => 1),
    outerPosition: vi.fn(async () => ({ x: 0, y: 0 })),
    outerSize: vi.fn(async () => ({ width: 360, height: 480 })),
    setSize: setSizeMock,
    setPosition: setPositionMock,
  })),
  LogicalPosition: class {
    constructor(
      public x: number,
      public y: number,
    ) {}
  },
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));

// Live2dStage 依赖 pixi / WebGL，jsdom 无法运行。桩在 modelUrl 从某个旧模型切到
// 新模型时以 3:4 宽高比回调 onModelMetrics，模拟模型加载完成后的尺寸上报；
// 同时回调 onLayout/onModelLoaded（智能穿透命中区域的数据源，fake 模型含 1 个
// drawable (0,0,10,10)、局部缩放因子 2 → 命中 rect (0,0,20,20)）；
// 首次挂载与初次赋 url 不上报（避免与 config 恢复效应竞争 setSize 等待信号）。
vi.mock("@/components/live2d/Live2dStage", async () => {
  const { useEffect, useRef } = await import("react");
  return {
    Live2dStage: (props: {
      modelUrl: string | null;
      onModelMetrics?: (m: { aspectRatio: number }) => void;
      onLayout?: (l: unknown) => void;
      onModelLoaded?: (m: unknown) => void;
    }) => {
      const prevUrl = useRef<string | null>(null);
      useEffect(() => {
        if (prevUrl.current !== null && prevUrl.current !== props.modelUrl) {
          props.onModelMetrics?.({ aspectRatio: 0.75 });
          props.onLayout?.({ x: 5, y: 6, scale: 2, canvasWidth: 100, canvasHeight: 200 });
          props.onModelLoaded?.({
            internalModel: {
              width: 200,
              height: 400,
              originalWidth: 100,
              originalHeight: 200,
              getDrawableIDs: () => ["a"],
              getDrawableIndex: () => 0,
              getDrawableBounds: () => ({ x: 0, y: 0, width: 10, height: 10 }),
            },
          });
        }
        prevUrl.current = props.modelUrl;
      }, [props.modelUrl]);
      return <div data-testid="live2d-stage" />;
    },
  };
});

/** 等 config 读取并完全应用：resizeTo 的 setSize 是 config useEffect（含 setLocked）之后的异步动作。 */
async function waitForConfigApplied() {
  await waitFor(() => expect(setSizeMock).toHaveBeenCalled());
}

beforeEach(() => {
  invokeMock.mockReset();
  startDraggingMock.mockReset();
  setSizeMock.mockReset();
  setPositionMock.mockReset();
  configState.locked = null;
  configState.dragMode = null;
  configState.modelFile = null;
  configState.modelDir = null;
  configState.format = null;
  for (const key of Object.keys(listenHandlers)) delete listenHandlers[key];

  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "get_live2d_config":
        return Promise.resolve({
          model_dir: configState.modelDir,
          model_file: configState.modelFile,
          format: configState.format,
          models_present: configState.modelFile != null,
          window_scale: 1.0,
          window_opacity: 1.0,
          click_through: null,
          window_layer: "front",
          locked: configState.locked,
          drag_mode: configState.dragMode,
          settings_path: "/zap/.zapmomo/settings.toml",
        });
      default:
        // useVoiceSession 等其余命令（is_voice_session_running 等）默认放行。
        return Promise.resolve(undefined);
    }
  });
});

describe("CompanionRoot（位置锁定）", () => {
  it("未锁定时左键按下触发窗口拖动", async () => {
    configState.locked = false;
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(1);
  });

  it("配置恢复为锁定时左键按下不触发拖动，滚轮缩放仍可用", async () => {
    configState.locked = true;
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container);
    expect(startDraggingMock).not.toHaveBeenCalled();

    // cmd/ctrl + 滚轮缩放不受锁定影响。
    fireEvent(container, new WheelEvent("wheel", { ctrlKey: true, deltaY: 100, cancelable: true }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "set_companion_scale",
        expect.objectContaining({ scale: expect.any(Number) }),
      ),
    );
  });

  it("companion-locked-changed 事件实时切换锁定与解锁", async () => {
    configState.locked = false;
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(1);

    // 后端事件：锁定 → 拖动被拦截。
    act(() => listenHandlers["companion-locked-changed"](true));
    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(1);

    // 后端事件：解锁 → 拖动恢复。
    act(() => listenHandlers["companion-locked-changed"](false));
    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(2);
  });

  it("锁定时右键菜单仍可打开（解锁入口保留）", async () => {
    configState.locked = true;
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.contextMenu(container);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "show_companion_menu",
        expect.objectContaining({ x: expect.any(Number), y: expect.any(Number) }),
      ),
    );
  });
});

describe("CompanionRoot（拖拽模式）", () => {
  it("缺省（null）视为 direct：裸左键按下触发窗口拖动", async () => {
    configState.dragMode = null;
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(1);
  });

  it("modifier 模式裸左键按下不触发拖动，按住 cmd 触发", async () => {
    configState.dragMode = "modifier";
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container);
    expect(startDraggingMock).not.toHaveBeenCalled();

    fireEvent.mouseDown(container, { metaKey: true });
    expect(startDraggingMock).toHaveBeenCalledTimes(1);
  });

  it("modifier 模式下 ctrl（Windows/Linux）同样触发拖动", async () => {
    configState.dragMode = "modifier";
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container, { ctrlKey: true });
    expect(startDraggingMock).toHaveBeenCalledTimes(1);
  });

  it("锁定优先于拖拽模式：modifier + 修饰键 + locked 仍不触发", async () => {
    configState.dragMode = "modifier";
    configState.locked = true;
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container, { metaKey: true });
    expect(startDraggingMock).not.toHaveBeenCalled();
  });

  it("companion-drag-mode-changed 事件实时切换拖拽模式", async () => {
    configState.dragMode = "direct";
    render(<CompanionRoot />);
    const container = screen.getByRole("application");
    await waitForConfigApplied();

    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(1);

    // 后端事件：切到 modifier → 裸按被拦截。
    act(() => listenHandlers["companion-drag-mode-changed"]("modifier"));
    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(1);

    // 切回 direct → 拖动恢复。
    act(() => listenHandlers["companion-drag-mode-changed"]("direct"));
    fireEvent.mouseDown(container);
    expect(startDraggingMock).toHaveBeenCalledTimes(2);
  });
});

describe("CompanionRoot（GIF 伙伴分发）", () => {
  it("config format=gif 时渲染 GifStage 而非 Live2dStage", async () => {
    configState.modelFile = "/zap/companions/x/dance.gif";
    configState.format = "gif";
    render(<CompanionRoot />);
    await waitForConfigApplied();

    expect(screen.getByTestId("gif-stage")).toBeInTheDocument();
    expect(screen.queryByTestId("live2d-stage")).not.toBeInTheDocument();
    // GIF 的 img 渲染在 GifStage 内。
    const img = screen.getByRole("img");
    expect(img).toHaveAttribute("src", expect.stringContaining("dance.gif"));
  });

  it("config format=cubism3 时仍渲染 Live2dStage", async () => {
    configState.modelFile = "/zap/companions/x/x.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();

    expect(screen.getByTestId("live2d-stage")).toBeInTheDocument();
    expect(screen.queryByTestId("gif-stage")).not.toBeInTheDocument();
  });

  it("config format=character 时渲染 GifStage（静态立绘）而非 Live2dStage", async () => {
    configState.modelFile = "/zap/companions/x/character.png";
    configState.format = "character";
    render(<CompanionRoot />);
    await waitForConfigApplied();

    expect(screen.getByTestId("gif-stage")).toBeInTheDocument();
    expect(screen.queryByTestId("live2d-stage")).not.toBeInTheDocument();
    const img = screen.getByRole("img");
    expect(img).toHaveAttribute("src", expect.stringContaining("character.png"));
  });

  it("live2d-model-changed 事件切到角色包（format=character）走 GifStage", async () => {
    configState.modelFile = "/zap/companions/x/x.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();
    expect(screen.getByTestId("live2d-stage")).toBeInTheDocument();

    act(() =>
      listenHandlers["live2d-model-changed"]({
        model_dir: "/zap/companions/c",
        model_file: "/zap/companions/c/character.png",
        format: "character",
        props: null,
      }),
    );
    expect(screen.getByTestId("gif-stage")).toBeInTheDocument();
    expect(screen.queryByTestId("live2d-stage")).not.toBeInTheDocument();
  });

  it("live2d-model-changed 事件可在 Live2D 与 GIF 之间切换", async () => {
    configState.modelFile = "/zap/companions/x/x.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();
    expect(screen.getByTestId("live2d-stage")).toBeInTheDocument();

    // 后端事件：切到 GIF 伙伴。
    act(() =>
      listenHandlers["live2d-model-changed"]({
        model_dir: "/zap/companions/g",
        model_file: "/zap/companions/g/dance.gif",
        format: "gif",
        props: null,
      }),
    );
    expect(screen.getByTestId("gif-stage")).toBeInTheDocument();
    expect(screen.queryByTestId("live2d-stage")).not.toBeInTheDocument();

    // 再切回 Live2D。
    act(() =>
      listenHandlers["live2d-model-changed"]({
        model_dir: "/zap/companions/x",
        model_file: "/zap/companions/x/x.model3.json",
        format: "cubism3",
        props: null,
      }),
    );
    expect(screen.getByTestId("live2d-stage")).toBeInTheDocument();
    expect(screen.queryByTestId("gif-stage")).not.toBeInTheDocument();
  });

  it("清屏事件（空 model_file）移除 GIF 展示", async () => {
    configState.modelFile = "/zap/companions/g/dance.gif";
    configState.format = "gif";
    render(<CompanionRoot />);
    await waitForConfigApplied();
    expect(screen.getByTestId("gif-stage")).toBeInTheDocument();

    act(() =>
      listenHandlers["live2d-model-changed"]({
        model_dir: null,
        model_file: null,
        format: null,
        props: null,
      }),
    );
    expect(screen.queryByTestId("gif-stage")).not.toBeInTheDocument();
  });
});

/** 与组件 computeSize 同公式：由宽高比与 scale 推导期望窗口尺寸。 */
function expectedSize(ratio: number, scale: number) {
  const availW = window.screen.availWidth;
  const availH = window.screen.availHeight;
  const baseH = Math.min(480, availH * 0.6);
  const modelH = Math.round(baseH * scale);
  let height = modelH + 72;
  let width = Math.round(modelH * ratio);
  height = Math.max(120, Math.min(height, Math.floor(availH * 0.9)));
  width = Math.max(120, Math.min(width, Math.floor(availW * 0.9)));
  return { width, height };
}

describe("CompanionRoot（伙伴私有布局）", () => {
  beforeEach(() => {
    // jsdom screen 默认 0×0，computeSize 会全部 clamp 到 120 下限；钉住屏幕尺寸
    // 让不同 scale 推导出可区分的期望尺寸（1280×800 下 1.0 → 360×552，0.5 → 180×312）。
    Object.defineProperty(window.screen, "availWidth", { value: 1280, configurable: true });
    Object.defineProperty(window.screen, "availHeight", { value: 800, configurable: true });
  });

  function pushModelChanged(payload: Record<string, unknown>) {
    act(() => listenHandlers["live2d-model-changed"](payload));
  }

  it("切换到有私有布局的伙伴：按其 scale 调整尺寸并恢复其位置", async () => {
    configState.modelFile = "/zap/companions/a/a.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();

    setSizeMock.mockClear();
    setPositionMock.mockClear();
    pushModelChanged({
      model_dir: "/zap/companions/b",
      model_file: "/zap/companions/b/b.model3.json",
      format: "cubism3",
      props: null,
      window_scale: 0.5,
      window_position: { x: 100, y: 200 },
    });

    const { width, height } = expectedSize(0.75, 0.5);
    await waitFor(() =>
      expect(setSizeMock).toHaveBeenCalledWith(expect.objectContaining({ width, height })),
    );
    await waitFor(() =>
      expect(setPositionMock).toHaveBeenCalledWith(expect.objectContaining({ x: 100, y: 200 })),
    );
  });

  it("切换到无私有布局的伙伴：沿用当前窗口尺寸，不回退全局默认", async () => {
    configState.modelFile = "/zap/companions/a/a.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();

    // 先切到有私有 scale（0.5）的伙伴 B，让当前窗口处于 0.5 状态。
    pushModelChanged({
      model_dir: "/zap/companions/b",
      model_file: "/zap/companions/b/b.model3.json",
      format: "cubism3",
      props: null,
      window_scale: 0.5,
      window_position: { x: 100, y: 200 },
    });
    const sizeB = expectedSize(0.75, 0.5);
    await waitFor(() => expect(setSizeMock).toHaveBeenCalledWith(expect.objectContaining(sizeB)));

    // 再切到从未配置的伙伴 C：沿用当前状态（尺寸仍是 0.5 推导值），不回退全局 1.0。
    setSizeMock.mockClear();
    pushModelChanged({
      model_dir: "/zap/companions/c",
      model_file: "/zap/companions/c/c.model3.json",
      format: "cubism3",
      props: null,
      window_scale: null,
      window_position: null,
    });
    await waitFor(() => expect(setSizeMock).toHaveBeenCalledWith(expect.objectContaining(sizeB)));
    const sizeGlobal = expectedSize(0.75, 1.0);
    expect(setSizeMock).not.toHaveBeenCalledWith(expect.objectContaining(sizeGlobal));
  });
});

describe("CompanionRoot（智能穿透上报）", () => {
  beforeEach(() => {
    Object.defineProperty(window.screen, "availWidth", { value: 1280, configurable: true });
    Object.defineProperty(window.screen, "availHeight", { value: 800, configurable: true });
  });

  function pushModelChanged(payload: Record<string, unknown>) {
    act(() => listenHandlers["live2d-model-changed"](payload));
  }

  function hitRegionCalls() {
    return invokeMock.mock.calls.filter((c) => c[0] === "set_companion_hit_region");
  }

  it("模型加载后上报逐 drawable 命中区域（窗口逻辑坐标，含顶部预留条偏移）", async () => {
    configState.modelFile = "/zap/companions/a/a.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();
    pushModelChanged({
      model_dir: "/zap/companions/b",
      model_file: "/zap/companions/b/b.model3.json",
      format: "cubism3",
      props: null,
    });
    // 桩在 url 变化时回调 onLayout/onModelLoaded：fake 命中 rect (0,0,20,20)（局部 ×2），
    // 经 layout {x:5,y:6,scale:2} + 顶部预留条 72 → 窗口坐标 (5, 78, 40, 40)。
    await waitFor(() => expect(hitRegionCalls()).not.toHaveLength(0));
    const [, payload] = hitRegionCalls().at(-1) ?? [];
    expect(payload).toEqual({
      rects: [{ x: 5, y: 78, width: 40, height: 40 }],
    });
  });

  it("启动未加载不上报（后端 fail-open 判可交互，加载失败仍可拖动/右键）", async () => {
    render(<CompanionRoot />);
    await waitForConfigApplied();
    // 越过 150ms 上报去抖窗口。
    await act(() => new Promise((r) => setTimeout(r, 220)));
    expect(hitRegionCalls()).toHaveLength(0);
  });

  it("曾加载后清屏上报空数组（空窗口明确穿透）", async () => {
    configState.modelFile = "/zap/companions/a/a.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();
    pushModelChanged({
      model_dir: "/zap/companions/b",
      model_file: "/zap/companions/b/b.model3.json",
      format: "cubism3",
      props: null,
    });
    await waitFor(() => expect(hitRegionCalls()).not.toHaveLength(0));
    pushModelChanged({ model_dir: null, model_file: null, format: null, props: null });
    await waitFor(() => {
      const [, payload] = hitRegionCalls().at(-1) ?? [];
      expect(payload).toEqual({ rects: [] });
    });
  });

  it("GIF 伙伴上报 object-contain 实际显示区（等比时铺满，y 含预留条偏移）", async () => {
    configState.modelFile = "/zap/companions/a/a.model3.json";
    configState.format = "cubism3";
    render(<CompanionRoot />);
    await waitForConfigApplied();
    pushModelChanged({
      model_dir: "/zap/companions/g",
      model_file: "/zap/companions/g/g.gif",
      format: "gif",
      props: null,
    });
    // jsdom 不触发 img onLoad：宽高比保持默认 3:4，与 360×480 舞台盒等比 → 铺满。
    await waitFor(() => expect(hitRegionCalls()).not.toHaveLength(0));
    const [, gifPayload] = hitRegionCalls().at(-1) ?? [];
    expect(gifPayload).toEqual({
      rects: [{ x: 0, y: 72, width: 360, height: 480 }],
    });
  });
});

describe("CompanionRoot（角色形象切换）", () => {
  function pushSpriteEvent(payload: Record<string, unknown>) {
    act(() => listenHandlers["companion-sprite-changed"](payload));
  }

  function pushModelChanged(payload: Record<string, unknown>) {
    act(() => listenHandlers["live2d-model-changed"](payload));
  }

  function currentImgSrc() {
    return screen.getByRole("img").getAttribute("src");
  }

  beforeEach(() => {
    // 角色包伙伴：config 恢复自带托管目录，sprite 事件路径以此为归属校验基准。
    configState.modelFile = "/zap/companions/c/character.png";
    configState.modelDir = "/zap/companions/c";
    configState.format = "character";
  });

  it("收到 sprite 事件（路径在当前伙伴目录内）→ 切换 img src", async () => {
    render(<CompanionRoot />);
    await waitForConfigApplied();
    expect(currentImgSrc()).toContain("character.png");

    pushSpriteEvent({
      companion_id: "companion-c",
      name: "happy",
      path: "/zap/companions/c/sprites/happy.png",
    });
    expect(currentImgSrc()).toContain("sprites/happy.png");
  });

  it("default 事件（character.png 路径）→ 恢复默认立绘", async () => {
    render(<CompanionRoot />);
    await waitForConfigApplied();

    pushSpriteEvent({
      companion_id: "companion-c",
      name: "happy",
      path: "/zap/companions/c/sprites/happy.png",
    });
    expect(currentImgSrc()).toContain("sprites/happy.png");

    pushSpriteEvent({
      companion_id: "companion-c",
      name: "default",
      path: "/zap/companions/c/character.png",
    });
    expect(currentImgSrc()).toContain("character.png");
  });

  it("路径不在当前伙伴托管目录内 → 忽略（防切伙伴竞态下的陈旧事件）", async () => {
    render(<CompanionRoot />);
    await waitForConfigApplied();

    pushSpriteEvent({
      companion_id: "companion-other",
      name: "happy",
      path: "/zap/companions/OTHER/sprites/happy.png",
    });
    expect(currentImgSrc()).toContain("character.png");
  });

  it("切换伙伴 → sprite 覆盖重置回新伙伴默认立绘，旧伙伴的 sprite 事件被忽略", async () => {
    render(<CompanionRoot />);
    await waitForConfigApplied();
    pushSpriteEvent({
      companion_id: "companion-c",
      name: "happy",
      path: "/zap/companions/c/sprites/happy.png",
    });
    expect(currentImgSrc()).toContain("sprites/happy.png");

    // 切到伙伴 B（角色包）：覆盖失效，回 B 的默认立绘。
    pushModelChanged({
      model_dir: "/zap/companions/b",
      model_file: "/zap/companions/b/character.png",
      format: "character",
      props: null,
    });
    expect(currentImgSrc()).toContain("companions/b/character.png");
    expect(currentImgSrc()).not.toContain("happy");

    // 旧伙伴 A 的 sprite 事件迟到：路径不在 B 目录内 → 忽略。
    pushSpriteEvent({
      companion_id: "companion-c",
      name: "sad",
      path: "/zap/companions/c/sprites/sad.png",
    });
    expect(currentImgSrc()).toContain("companions/b/character.png");

    // B 自己的 sprite 事件正常生效。
    pushSpriteEvent({
      companion_id: "companion-b",
      name: "angry",
      path: "/zap/companions/b/sprites/angry.png",
    });
    expect(currentImgSrc()).toContain("companions/b/sprites/angry.png");
  });
});
