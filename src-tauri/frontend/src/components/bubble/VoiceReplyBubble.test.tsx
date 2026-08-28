import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { VoiceReplyBubble } from "./VoiceReplyBubble";

const { startDraggingMock, invokeMock } = vi.hoisted(() => ({
  startDraggingMock: vi.fn(() => Promise.resolve()),
  invokeMock: vi.fn(() => Promise.resolve(undefined)),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    startDragging: startDraggingMock,
  })),
}));

describe("VoiceReplyBubble（回复气泡）", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("流式文本更新时跟随最新内容", () => {
    const { rerender } = render(<VoiceReplyBubble text="你好" phase="thinking" />);
    expect(screen.getByText("你好")).toBeTruthy();
    rerender(<VoiceReplyBubble text="你好，我是" phase="thinking" />);
    expect(screen.getByText("你好，我是")).toBeTruthy();
  });

  it("无内容时不渲染", () => {
    const { container } = render(<VoiceReplyBubble text="" phase="armed" />);
    expect(container.firstChild).toBeNull();
  });

  it("正常完结（text 清空、phase 仍在 speaking）：定格 5s 后消失", () => {
    const { rerender } = render(<VoiceReplyBubble text="完整回复" phase="speaking" />);
    rerender(<VoiceReplyBubble text="" phase="speaking" />);
    // 定格中仍显示
    expect(screen.getByText("完整回复")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(5100);
    });
    expect(screen.queryByText("完整回复")).toBeNull();
  });

  it("打断（text 清空、phase 已回 armed）：立即消失", () => {
    const { rerender } = render(<VoiceReplyBubble text="被打断的回复" phase="speaking" />);
    rerender(<VoiceReplyBubble text="" phase="armed" />);
    expect(screen.queryByText("被打断的回复")).toBeNull();
  });

  it("定格期内 phase 回 armed 不打断定格（播完回待唤醒场景）", () => {
    const { rerender } = render(<VoiceReplyBubble text="定格我" phase="speaking" />);
    rerender(<VoiceReplyBubble text="" phase="speaking" />);
    // 定格 1s 后 phase 变 armed（播完回待唤醒），气泡应保持
    rerender(<VoiceReplyBubble text="" phase="armed" />);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(screen.getByText("定格我")).toBeTruthy();
    // 总计 5s 后仍按时消失
    act(() => {
      vi.advanceTimersByTime(3100);
    });
    expect(screen.queryByText("定格我")).toBeNull();
  });

  it("新一轮文本到达取消定格淡出，立即展示新内容", () => {
    const { rerender } = render(<VoiceReplyBubble text="旧回复" phase="speaking" />);
    rerender(<VoiceReplyBubble text="" phase="speaking" />);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    rerender(<VoiceReplyBubble text="新回复" phase="thinking" />);
    expect(screen.getByText("新回复")).toBeTruthy();
    // 旧定时器已取消：越过原 5s 点后新内容仍在
    act(() => {
      vi.advanceTimersByTime(4000);
    });
    expect(screen.getByText("新回复")).toBeTruthy();
  });

  it("气泡面 mousedown(button 0) 触发窗口拖动", () => {
    render(<VoiceReplyBubble text="拖我" phase="speaking" />);
    // 事件从内层文本冒泡到外层拖动面
    fireEvent.mouseDown(screen.getByText("拖我"), { button: 0 });
    expect(startDraggingMock).toHaveBeenCalled();
  });

  it("右键 mousedown 不触发拖动", () => {
    render(<VoiceReplyBubble text="拖我" phase="speaking" />);
    fireEvent.mouseDown(screen.getByText("拖我"), { button: 2 });
    expect(startDraggingMock).not.toHaveBeenCalled();
  });

  it("可见性变化经 onVisibleChange 上报（供窗口根组件切换点击穿透）", () => {
    const onVisibleChange = vi.fn();
    const { rerender } = render(
      <VoiceReplyBubble text="" phase="armed" onVisibleChange={onVisibleChange} />,
    );
    expect(onVisibleChange).toHaveBeenLastCalledWith(false);
    rerender(<VoiceReplyBubble text="出现" phase="thinking" onVisibleChange={onVisibleChange} />);
    expect(onVisibleChange).toHaveBeenLastCalledWith(true);
    // 打断立即消失 → 上报不可见
    rerender(<VoiceReplyBubble text="" phase="armed" onVisibleChange={onVisibleChange} />);
    expect(onVisibleChange).toHaveBeenLastCalledWith(false);
  });
});
