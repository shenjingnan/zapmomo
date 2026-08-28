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

  it("正常完结：定格 5s（期间不透明），随后淡出并于 5.5s 移除", () => {
    const { rerender, container } = render(<VoiceReplyBubble text="完整回复" phase="speaking" />);
    rerender(<VoiceReplyBubble text="" phase="speaking" />);
    // 定格中仍显示
    expect(screen.getByText("完整回复")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(4000);
    });
    expect(container.querySelector(".transition-opacity")?.className).toContain("opacity-100");
    // 5s 进入淡出（DOM 仍在，透明度过渡到 0）
    act(() => {
      vi.advanceTimersByTime(1200);
    });
    expect(screen.getByText("完整回复")).toBeTruthy();
    expect(container.querySelector(".transition-opacity")?.className).toContain("opacity-0");
    // 5.5s 移除
    act(() => {
      vi.advanceTimersByTime(400);
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
    // 总计 5.6s（定格 5s + 淡出 0.5s）后消失
    act(() => {
      vi.advanceTimersByTime(3600);
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

  // ---- 插播通道（announcement，dsh 播报与回复共用同一个气泡）----

  it("插播在无流式文本时展示，走同一套定格→淡出→移除", () => {
    const { container } = render(
      <VoiceReplyBubble text="" phase="speaking" announcement="开工啦" />,
    );
    expect(screen.getByText("开工啦")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(4900);
    });
    expect(screen.getByText("开工啦")).toBeTruthy();
    expect(container.querySelector(".transition-opacity")?.className).toContain("opacity-100");
    act(() => {
      vi.advanceTimersByTime(700);
    });
    expect(screen.queryByText("开工啦")).toBeNull();
  });

  it("插播被流式回复压制，回复完结后（新鲜期内）补展示", () => {
    const { rerender } = render(<VoiceReplyBubble text="" phase="armed" />);
    rerender(<VoiceReplyBubble text="长回复" phase="thinking" announcement="插播台词" />);
    expect(screen.getByText("长回复")).toBeTruthy();
    expect(screen.queryByText("插播台词")).toBeNull();
    rerender(<VoiceReplyBubble text="" phase="speaking" announcement="插播台词" />);
    expect(screen.getByText("插播台词")).toBeTruthy();
    expect(screen.queryByText("长回复")).toBeNull();
  });

  it("插播被压制超过新鲜期（5s）→ 回复完结后不再补展示，回复照常定格", () => {
    const { rerender } = render(
      <VoiceReplyBubble text="长回复" phase="thinking" announcement="过期插播" />,
    );
    expect(screen.getByText("长回复")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(6000);
    });
    rerender(<VoiceReplyBubble text="" phase="speaking" announcement="过期插播" />);
    expect(screen.queryByText("过期插播")).toBeNull();
    expect(screen.getByText("长回复")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(5600);
    });
    expect(screen.queryByText("长回复")).toBeNull();
  });

  it("插播到达时替换定格中的旧回复（最新发言胜出）", () => {
    const { rerender } = render(<VoiceReplyBubble text="旧回复" phase="speaking" />);
    rerender(<VoiceReplyBubble text="" phase="speaking" />);
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    rerender(<VoiceReplyBubble text="" phase="speaking" announcement="插播台词" />);
    expect(screen.getByText("插播台词")).toBeTruthy();
    expect(screen.queryByText("旧回复")).toBeNull();
  });

  it("插播不随会话打断消失（phase 回 armed 不影响，按自身定时消失）", () => {
    const { rerender } = render(
      <VoiceReplyBubble text="" phase="speaking" announcement="插播台词" />,
    );
    rerender(<VoiceReplyBubble text="" phase="armed" announcement="插播台词" />);
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(screen.getByText("插播台词")).toBeTruthy();
  });

  it("流式回复开始时立即顶掉展示中的插播", () => {
    const { rerender } = render(<VoiceReplyBubble text="" phase="armed" announcement="插播台词" />);
    expect(screen.getByText("插播台词")).toBeTruthy();
    rerender(<VoiceReplyBubble text="新回复" phase="thinking" announcement="插播台词" />);
    expect(screen.getByText("新回复")).toBeTruthy();
    expect(screen.queryByText("插播台词")).toBeNull();
  });

  it("插播展示同样经 onVisibleChange 上报", () => {
    const onVisibleChange = vi.fn();
    render(
      <VoiceReplyBubble
        text=""
        phase="armed"
        announcement="插播台词"
        onVisibleChange={onVisibleChange}
      />,
    );
    expect(onVisibleChange).toHaveBeenLastCalledWith(true);
  });
});
