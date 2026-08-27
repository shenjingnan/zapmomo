import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ChatboxBar } from "./ChatboxBar";

const { invokeMock, startDraggingMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  startDraggingMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock("@tauri-apps/api/dpi", () => ({
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
  PhysicalPosition: class {
    constructor(
      public x: number,
      public y: number,
    ) {}
  },
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    startDragging: startDraggingMock,
    onMoved: vi.fn(() => Promise.resolve(() => {})),
    onFocusChanged: vi.fn(() => Promise.resolve(() => {})),
    scaleFactor: vi.fn(() => Promise.resolve(2)),
    // 物理像素（2x 缩放）：对应逻辑 520x96 基准窗口 → 单行时跳过重排
    innerSize: vi.fn(() => Promise.resolve({ width: 1040, height: 192 })),
    outerPosition: vi.fn(() => Promise.resolve({ x: 100, y: 100 })),
    setSize: vi.fn(() => Promise.resolve()),
    setPosition: vi.fn(() => Promise.resolve()),
  })),
}));

function inputBox() {
  return screen.getByLabelText("消息输入框") as HTMLInputElement;
}

describe("ChatboxBar（文字输入条）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeMock.mockResolvedValue(undefined);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("Enter 发送：调用 send_voice_text 并清空输入框", async () => {
    render(<ChatboxBar />);
    fireEvent.change(inputBox(), { target: { value: "你好呀" } });
    fireEvent.keyDown(inputBox(), { key: "Enter" });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("send_voice_text", { text: "你好呀" }),
    );
    await waitFor(() => expect(inputBox().value).toBe(""));
  });

  it("纯空白消息不发送", () => {
    render(<ChatboxBar />);
    fireEvent.change(inputBox(), { target: { value: "   " } });
    fireEvent.keyDown(inputBox(), { key: "Enter" });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("发送内容两端空白被裁剪", async () => {
    render(<ChatboxBar />);
    fireEvent.change(inputBox(), { target: { value: "  在吗  " } });
    fireEvent.keyDown(inputBox(), { key: "Enter" });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("send_voice_text", { text: "在吗" }),
    );
  });

  it("发送失败显示错误提示，3 秒后自动消失", async () => {
    vi.useFakeTimers();
    invokeMock.mockRejectedValue("语音互动未运行");
    render(<ChatboxBar />);
    fireEvent.change(inputBox(), { target: { value: "你好" } });
    fireEvent.keyDown(inputBox(), { key: "Enter" });
    await vi.waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("语音互动未运行"),
    );
    // 输入框保留文本（发送失败不清空）
    expect(inputBox().value).toBe("你好");
    // 3s 后错误消失（act 包裹让 React 落定定时器触发的状态更新）
    await act(async () => {
      vi.advanceTimersByTime(3100);
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("Esc 关闭输入条窗口（hide_chatbox）", () => {
    render(<ChatboxBar />);
    fireEvent.keyDown(inputBox(), { key: "Escape" });
    expect(invokeMock).toHaveBeenCalledWith("hide_chatbox");
  });

  it("Shift+Enter 换行不发送", () => {
    render(<ChatboxBar />);
    fireEvent.change(inputBox(), { target: { value: "第一行" } });
    fireEvent.keyDown(inputBox(), { key: "Enter", shiftKey: true });
    expect(invokeMock).not.toHaveBeenCalled();
    // 输入内容保留，可继续编辑第二行
    expect(inputBox().value).toBe("第一行");
  });

  it("点击发送按钮等价于 Enter", async () => {
    render(<ChatboxBar />);
    fireEvent.change(inputBox(), { target: { value: "按钮发送" } });
    fireEvent.click(screen.getByLabelText("发送"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("send_voice_text", { text: "按钮发送" }),
    );
  });

  it("拖拽把手 mousedown 触发窗口拖动", () => {
    render(<ChatboxBar />);
    fireEvent.mouseDown(screen.getByLabelText("拖动输入条"), { button: 0 });
    expect(startDraggingMock).toHaveBeenCalled();
  });

  it("空消息时发送按钮禁用", () => {
    render(<ChatboxBar />);
    expect((screen.getByLabelText("发送") as HTMLButtonElement).disabled).toBe(true);
  });
});
