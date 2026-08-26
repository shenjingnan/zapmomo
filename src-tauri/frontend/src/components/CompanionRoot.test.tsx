import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CompanionDragMode } from "@/types/tauri";
import { CompanionRoot } from "./CompanionRoot";

const { invokeMock, startDraggingMock, setSizeMock, configState, listenHandlers } = vi.hoisted(
  () => ({
    invokeMock: vi.fn(),
    startDraggingMock: vi.fn(),
    /** resizeTo 的 setSize 是 config 完全应用（含 setLocked）后的最后一步，作等待信号。 */
    setSizeMock: vi.fn(async () => undefined),
    /** get_live2d_config 的 locked / drag_mode / 模型字段覆盖值（null = 后端未返回该字段）。 */
    configState: {
      locked: null as boolean | null,
      dragMode: null as CompanionDragMode | null,
      modelFile: null as string | null,
      format: null as string | null,
    },
    /** 按事件名捕获 listen 回调，供测试主动推送后端事件。 */
    listenHandlers: {} as Record<string, (payload: unknown) => void>,
  }),
);

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
    setPosition: vi.fn(() => Promise.resolve()),
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

// Live2dStage 依赖 pixi / WebGL，jsdom 无法运行。
vi.mock("@/components/live2d/Live2dStage", () => ({
  Live2dStage: () => <div data-testid="live2d-stage" />,
}));

/** 等 config 读取并完全应用：resizeTo 的 setSize 是 config useEffect（含 setLocked）之后的异步动作。 */
async function waitForConfigApplied() {
  await waitFor(() => expect(setSizeMock).toHaveBeenCalled());
}

beforeEach(() => {
  invokeMock.mockReset();
  startDraggingMock.mockReset();
  setSizeMock.mockReset();
  configState.locked = null;
  configState.dragMode = null;
  configState.modelFile = null;
  configState.format = null;
  for (const key of Object.keys(listenHandlers)) delete listenHandlers[key];

  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "get_live2d_config":
        return Promise.resolve({
          model_dir: null,
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
