import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BubbleRoot } from "./BubbleRoot";

const { invokeMock, listenMock, eventHandlers, setIgnoreMock, onMovedHandlers } = vi.hoisted(() => {
  const handlers: Record<string, (payload: unknown) => void> = {};
  return {
    invokeMock: vi.fn(),
    listenMock: vi.fn((event: string, cb: (e: { payload: unknown }) => void) => {
      handlers[event] = (payload) => cb({ payload });
      return Promise.resolve(() => {});
    }),
    eventHandlers: handlers,
    setIgnoreMock: vi.fn(() => Promise.resolve()),
    onMovedHandlers: {
      current: undefined as undefined | ((e: { payload: { x: number; y: number } }) => void),
    },
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    setIgnoreCursorEvents: setIgnoreMock,
    startDragging: vi.fn(() => Promise.resolve()),
    onMoved: vi.fn((cb: (e: { payload: { x: number; y: number } }) => void) => {
      onMovedHandlers.current = cb;
      return Promise.resolve(() => {});
    }),
    scaleFactor: vi.fn(() => Promise.resolve(2)),
  })),
}));

function emit(event: string, payload: unknown) {
  act(() => {
    eventHandlers[event]?.(payload);
  });
}

describe("BubbleRoot（气泡窗口根组件）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // useVoiceSession / getLive2dConfig 等所有命令默认放行
    invokeMock.mockResolvedValue(undefined);
    setIgnoreMock.mockClear();
  });

  it("挂载即空闲点穿（setIgnoreCursorEvents(true)）", async () => {
    render(<BubbleRoot />);
    await waitFor(() => expect(setIgnoreMock).toHaveBeenCalledWith(true));
    expect(setIgnoreMock).not.toHaveBeenCalledWith(false);
  });

  it("收到流式 token 后渲染气泡并恢复接收鼠标（可拖动）", async () => {
    render(<BubbleRoot />);
    await waitFor(() => expect(setIgnoreMock).toHaveBeenCalledWith(true));
    emit("voice-session-token", { delta: "你好呀" });
    expect(screen.getByText("你好呀")).toBeTruthy();
    await waitFor(() => expect(setIgnoreMock).toHaveBeenCalledWith(false));
  });

  it("打断（state 回 armed）清空内容并恢复点穿", async () => {
    render(<BubbleRoot />);
    emit("voice-session-token", { delta: "被打断" });
    expect(screen.getByText("被打断")).toBeTruthy();
    emit("voice-session-state", { running: true, state: "armed" });
    expect(screen.queryByText("被打断")).toBeNull();
    await waitFor(() => expect(setIgnoreMock).toHaveBeenLastCalledWith(true));
  });

  it("拖动停止后以逻辑像素回写位置（save_bubble_position）", async () => {
    render(<BubbleRoot />);
    await waitFor(() => expect(onMovedHandlers.current).toBeTruthy());
    // 物理像素（2x 缩放）→ 逻辑 (100, 200)
    act(() => {
      onMovedHandlers.current?.({ payload: { x: 200, y: 400 } });
    });
    await waitFor(
      () => expect(invokeMock).toHaveBeenCalledWith("save_bubble_position", { x: 100, y: 200 }),
      { timeout: 2000 },
    );
  });

  it("角色置底（layer=back）时不渲染气泡内容", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "get_live2d_config"
        ? Promise.resolve({ window_layer: "back" })
        : Promise.resolve(undefined),
    );
    render(<BubbleRoot />);
    // 等 layer 回读落定（初始渲染先按 front，回读后才切 back）再推事件
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_live2d_config"));
    await act(async () => {});
    emit("voice-session-token", { delta: "不应出现" });
    expect(screen.queryByText("不应出现")).toBeNull();
    expect(setIgnoreMock).not.toHaveBeenCalledWith(false);
  });

  // ---- 统一聊天气泡：dsh（DeepSeek Harness）播报与回复共用同一个气泡窗口 ----

  it("dsh-speak 播报渲染进气泡并恢复接收鼠标（与回复同一气泡）", async () => {
    render(<BubbleRoot />);
    await waitFor(() => expect(setIgnoreMock).toHaveBeenCalledWith(true));
    emit("dsh-speak", { text: "开工啦", event: { kind: "task-started" } });
    expect(screen.getByText("开工啦")).toBeTruthy();
    await waitFor(() => expect(setIgnoreMock).toHaveBeenCalledWith(false));
  });

  it("插播不顶掉正在流式输出的回复，回复完结后补展示插播", async () => {
    render(<BubbleRoot />);
    emit("voice-session-token", { delta: "回复中" });
    emit("dsh-speak", { text: "插播台词", event: {} });
    expect(screen.getByText("回复中")).toBeTruthy();
    expect(screen.queryByText("插播台词")).toBeNull();
    emit("voice-session-reply-finished", { text: "回复中" });
    expect(screen.getByText("插播台词")).toBeTruthy();
  });

  it("角色置底（layer=back）时 dsh 播报同样不渲染", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "get_live2d_config"
        ? Promise.resolve({ window_layer: "back" })
        : Promise.resolve(undefined),
    );
    render(<BubbleRoot />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_live2d_config"));
    await act(async () => {});
    emit("dsh-speak", { text: "置底不播", event: {} });
    expect(screen.queryByText("置底不播")).toBeNull();
  });
});
