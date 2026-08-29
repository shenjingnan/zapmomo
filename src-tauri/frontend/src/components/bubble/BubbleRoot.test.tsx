import { LogicalSize, PhysicalPosition } from "@tauri-apps/api/dpi";
import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BubbleRoot } from "./BubbleRoot";

const {
  invokeMock,
  listenMock,
  eventHandlers,
  setIgnoreMock,
  onMovedHandlers,
  winState,
  setSizeMock,
  setPositionMock,
  roInstances,
} = vi.hoisted(() => {
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
    // 窗口几何（物理像素，2x 缩放）：初始 480×180 @ (100, 400)，setSize/setPosition 真实回写
    winState: { x: 100, y: 400, w: 960, h: 360 },
    setSizeMock: vi.fn(async (s: { width: number; height: number; type?: string }) => {
      const logical = s.type === "Logical";
      winState.w = logical ? s.width * 2 : s.width;
      winState.h = logical ? s.height * 2 : s.height;
    }),
    setPositionMock: vi.fn(async (p: { x: number; y: number }) => {
      winState.x = p.x;
      winState.y = p.y;
    }),
    roInstances: [] as { cb: ResizeObserverCallback }[],
  };
});

// jsdom 无 ResizeObserver：stub 捕获实例，测试手动派发 contentRect 变化
class ROStub {
  cb: ResizeObserverCallback;
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb;
    roInstances.push(this);
  }
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", ROStub);

/** 手动触发气泡容器的 ResizeObserver 回调（height 为逻辑像素）。 */
const fireBubbleResize = (height: number) => {
  const ro = roInstances.at(-1);
  if (!ro) throw new Error("ResizeObserver 未挂载");
  act(() => {
    ro.cb(
      [{ contentRect: { width: 448, height } }] as unknown as ResizeObserverEntry[],
      ro as unknown as ResizeObserver,
    );
  });
};

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
    innerSize: vi.fn(() => Promise.resolve({ width: winState.w, height: winState.h })),
    outerPosition: vi.fn(() => Promise.resolve({ x: winState.x, y: winState.y })),
    setSize: setSizeMock,
    setPosition: setPositionMock,
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
    roInstances.length = 0;
    // 窗口几何复位：480×180 @ (100, 400)，物理像素（2x）
    winState.x = 100;
    winState.y = 400;
    winState.w = 960;
    winState.h = 360;
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

  it("打断（state 回 armed）保留内容静置，保持接收鼠标（等用户点击关闭）", async () => {
    render(<BubbleRoot />);
    emit("voice-session-token", { delta: "被打断" });
    expect(screen.getByText("被打断")).toBeTruthy();
    emit("voice-session-state", { running: true, state: "armed" });
    // 打断不清场：内容静置保留，窗口保持接收鼠标（点穿只由点击关闭触发）
    expect(screen.getByText("被打断")).toBeTruthy();
    await waitFor(() => expect(setIgnoreMock).toHaveBeenLastCalledWith(false));
  });

  it("用户句经 transcript is_final 渲染「我：」前缀并恢复接收鼠标", async () => {
    render(<BubbleRoot />);
    await waitFor(() => expect(setIgnoreMock).toHaveBeenCalledWith(true));
    emit("voice-session-transcript", { text: "你好", is_final: true });
    expect(screen.getByText("我：你好")).toBeTruthy();
    await waitFor(() => expect(setIgnoreMock).toHaveBeenCalledWith(false));
  });

  it("同文本连发（turnSeq 递增）仍开启新一轮，顶掉上一轮静置回复", async () => {
    render(<BubbleRoot />);
    emit("voice-session-transcript", { text: "继续", is_final: true });
    emit("voice-session-token", { delta: "好的" });
    emit("voice-session-reply-finished", { reason: "Eos", text: "好的" });
    expect(screen.getByText("好的")).toBeTruthy();
    // 同文本第二轮：仅按 userText 值判不出新轮，靠 turnSeq 自增顶掉静置旧回复
    emit("voice-session-transcript", { text: "继续", is_final: true });
    expect(screen.getByText("我：继续")).toBeTruthy();
    expect(screen.queryByText("好的")).toBeNull();
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

  // ---- 窗口高度随内容自适应（底边锚定向上生长）----

  it("气泡内容增高时窗口高度跟随，底边锚定向上生长", async () => {
    render(<BubbleRoot />);
    emit("voice-session-token", { delta: "你好" });
    expect(screen.getByText("你好")).toBeTruthy();
    // 内容 100 逻辑像素 → 期望窗口高 100 + 12(pt) + 26(阴影扩散) = 138
    fireBubbleResize(100);
    await waitFor(() => expect(setSizeMock).toHaveBeenLastCalledWith(new LogicalSize(480, 138)));
    // dy = 138 - 180 = -42 → y = 400 - (-42 × 2) = 484；底边不变：400+360 === 484+276
    expect(setPositionMock).toHaveBeenLastCalledWith(new PhysicalPosition(100, 484));
  });

  it("内容高度未变化时不重复调整窗口", async () => {
    render(<BubbleRoot />);
    fireBubbleResize(100);
    await waitFor(() => expect(setSizeMock).toHaveBeenLastCalledWith(new LogicalSize(480, 138)));
    setSizeMock.mockClear();
    setPositionMock.mockClear();
    fireBubbleResize(100);
    await act(async () => {});
    expect(setSizeMock).not.toHaveBeenCalled();
    expect(setPositionMock).not.toHaveBeenCalled();
  });

  it("内容清空（点击关闭）后窗口缩回最小高度", async () => {
    render(<BubbleRoot />);
    fireBubbleResize(60); // 60 + 38 = 98
    await waitFor(() => expect(setSizeMock).toHaveBeenLastCalledWith(new LogicalSize(480, 98)));
    fireBubbleResize(0); // 内容消失 → 仅剩上下留白（38），纯透明不可见
    await waitFor(() => expect(setSizeMock).toHaveBeenLastCalledWith(new LogicalSize(480, 38)));
  });
});
