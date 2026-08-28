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

// jsdom 无 Pointer Capture API：组件在 pointerdown 里 setPointerCapture，stub 掉
HTMLElement.prototype.setPointerCapture = vi.fn();
HTMLElement.prototype.releasePointerCapture = vi.fn();

/** 左键按下（默认原点 100,100）。 */
const press = (el: Element, x = 100, y = 100) =>
  fireEvent.pointerDown(el, { button: 0, pointerId: 1, clientX: x, clientY: y });
/** 按住移动。 */
const move = (el: Element, x: number, y: number) =>
  fireEvent.pointerMove(el, { pointerId: 1, clientX: x, clientY: y });
/** 松开（默认左键）。 */
const release = (el: Element, x = 100, y = 100, button = 0) =>
  fireEvent.pointerUp(el, { button, pointerId: 1, clientX: x, clientY: y });

describe("VoiceReplyBubble（回复气泡）", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // ---- 展示生命周期：静置 + 手动关闭（无自动消失）----

  it("流式文本更新时跟随最新内容", () => {
    const { rerender } = render(<VoiceReplyBubble text="你好" />);
    expect(screen.getByText("你好")).toBeTruthy();
    rerender(<VoiceReplyBubble text="你好，我是" />);
    expect(screen.getByText("你好，我是")).toBeTruthy();
  });

  it("无内容时不渲染", () => {
    const { container } = render(<VoiceReplyBubble text="" />);
    expect(container.firstChild).toBeNull();
  });

  it("回复完结（text 清空）后静置不自动消失，等用户点击", () => {
    const { rerender } = render(<VoiceReplyBubble text="完整回复" />);
    rerender(<VoiceReplyBubble text="" />);
    // 越过原「定格 5s + 淡出 0.5s」的时间点后内容仍在
    act(() => {
      vi.advanceTimersByTime(6000);
    });
    expect(screen.getByText("完整回复")).toBeTruthy();
  });

  it("打断（text 清空）保留半截内容，不立即清除", () => {
    const { rerender } = render(<VoiceReplyBubble text="被打断的回复" />);
    rerender(<VoiceReplyBubble text="" />);
    expect(screen.getByText("被打断的回复")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(6000);
    });
    expect(screen.getByText("被打断的回复")).toBeTruthy();
  });

  it("新一轮文本到达顶掉静置内容", () => {
    const { rerender } = render(<VoiceReplyBubble text="旧回复" />);
    rerender(<VoiceReplyBubble text="" />);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    rerender(<VoiceReplyBubble text="新回复" />);
    expect(screen.getByText("新回复")).toBeTruthy();
    expect(screen.queryByText("旧回复")).toBeNull();
  });

  // ---- 点击关闭 ----

  it("内容定稿（text 清空）后点击气泡关闭", () => {
    const { rerender } = render(<VoiceReplyBubble text="定稿内容" />);
    rerender(<VoiceReplyBubble text="" />);
    press(screen.getByText("定稿内容"));
    release(screen.getByText("定稿内容"));
    expect(screen.queryByText("定稿内容")).toBeNull();
  });

  it("流式进行中（text 非空）点击不响应，内容未定稿", () => {
    render(<VoiceReplyBubble text="流式中的回复" />);
    press(screen.getByText("流式中的回复"));
    release(screen.getByText("流式中的回复"));
    expect(screen.getByText("流式中的回复")).toBeTruthy();
  });

  it("点击关闭后同 props 重渲染不复活", () => {
    const { rerender } = render(<VoiceReplyBubble text="定稿内容" />);
    rerender(<VoiceReplyBubble text="" />);
    const el = screen.getByText("定稿内容");
    press(el);
    release(el);
    expect(screen.queryByText("定稿内容")).toBeNull();
    rerender(<VoiceReplyBubble text="" />);
    expect(screen.queryByText("定稿内容")).toBeNull();
  });

  // ---- 点击 vs 拖动判定（5px 位移阈值）----

  it("按住位移超过阈值触发窗口拖动，且内容保留不关闭", () => {
    render(<VoiceReplyBubble text="拖我" />);
    const el = screen.getByText("拖我");
    press(el, 100, 100);
    move(el, 130, 100);
    expect(startDraggingMock).toHaveBeenCalled();
    release(el, 130, 100);
    expect(screen.getByText("拖我")).toBeTruthy();
  });

  it("位移未超阈值松开视为点击关闭，不触发拖动", () => {
    const { rerender } = render(<VoiceReplyBubble text="点我" />);
    rerender(<VoiceReplyBubble text="" />);
    const el = screen.getByText("点我");
    press(el, 100, 100);
    move(el, 103, 100);
    release(el, 103, 100);
    expect(startDraggingMock).not.toHaveBeenCalled();
    expect(screen.queryByText("点我")).toBeNull();
  });

  it("右键按下不触发拖动也不关闭", () => {
    const { rerender } = render(<VoiceReplyBubble text="右键无效" />);
    rerender(<VoiceReplyBubble text="" />);
    const el = screen.getByText("右键无效");
    fireEvent.pointerDown(el, { button: 2, pointerId: 1, clientX: 100, clientY: 100 });
    move(el, 130, 100);
    expect(startDraggingMock).not.toHaveBeenCalled();
    release(el, 130, 100, 2);
    expect(screen.getByText("右键无效")).toBeTruthy();
  });

  it("pointerCancel 后松开不误关闭", () => {
    render(<VoiceReplyBubble text="取消流" />);
    const el = screen.getByText("取消流");
    press(el);
    fireEvent.pointerCancel(el, { pointerId: 1 });
    release(el);
    expect(screen.getByText("取消流")).toBeTruthy();
  });

  // ---- 可见性上报（窗口根组件据此切换点击穿透）----

  it("可见性变化经 onVisibleChange 上报：出现为 true，点击关闭为 false", () => {
    const onVisibleChange = vi.fn();
    const { rerender } = render(<VoiceReplyBubble text="" onVisibleChange={onVisibleChange} />);
    expect(onVisibleChange).toHaveBeenLastCalledWith(false);
    rerender(<VoiceReplyBubble text="出现" onVisibleChange={onVisibleChange} />);
    expect(onVisibleChange).toHaveBeenLastCalledWith(true);
    rerender(<VoiceReplyBubble text="" onVisibleChange={onVisibleChange} />);
    expect(onVisibleChange).toHaveBeenLastCalledWith(true); // 静置保留，仍可见
    const el = screen.getByText("出现");
    press(el);
    release(el);
    expect(onVisibleChange).toHaveBeenLastCalledWith(false);
  });

  // ---- 插播通道（announcement，dsh 播报与回复共用同一个气泡）----

  it("插播在无流式文本时展示并静置不自动消失", () => {
    render(<VoiceReplyBubble text="" announcement="开工啦" />);
    expect(screen.getByText("开工啦")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(6000);
    });
    expect(screen.getByText("开工啦")).toBeTruthy();
  });

  it("插播被流式回复压制，回复完结后（新鲜期内）补展示", () => {
    const { rerender } = render(<VoiceReplyBubble text="" />);
    rerender(<VoiceReplyBubble text="长回复" announcement="插播台词" />);
    expect(screen.getByText("长回复")).toBeTruthy();
    expect(screen.queryByText("插播台词")).toBeNull();
    rerender(<VoiceReplyBubble text="" announcement="插播台词" />);
    expect(screen.getByText("插播台词")).toBeTruthy();
    expect(screen.queryByText("长回复")).toBeNull();
  });

  it("插播被压制超过新鲜期不再补展示，回复保持静置", () => {
    const { rerender } = render(<VoiceReplyBubble text="长回复" announcement="过期插播" />);
    expect(screen.getByText("长回复")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(6000);
    });
    rerender(<VoiceReplyBubble text="" announcement="过期插播" />);
    expect(screen.queryByText("过期插播")).toBeNull();
    expect(screen.getByText("长回复")).toBeTruthy();
  });

  it("插播到达时替换静置中的旧回复（最新发言胜出）", () => {
    const { rerender } = render(<VoiceReplyBubble text="旧回复" />);
    rerender(<VoiceReplyBubble text="" />);
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    rerender(<VoiceReplyBubble text="" announcement="插播台词" />);
    expect(screen.getByText("插播台词")).toBeTruthy();
    expect(screen.queryByText("旧回复")).toBeNull();
  });

  it("流式回复开始时立即顶掉展示中的插播", () => {
    const { rerender } = render(<VoiceReplyBubble text="" announcement="插播台词" />);
    expect(screen.getByText("插播台词")).toBeTruthy();
    rerender(<VoiceReplyBubble text="新回复" announcement="插播台词" />);
    expect(screen.getByText("新回复")).toBeTruthy();
    expect(screen.queryByText("插播台词")).toBeNull();
  });

  it("插播展示同样经 onVisibleChange 上报", () => {
    const onVisibleChange = vi.fn();
    render(<VoiceReplyBubble text="" announcement="插播台词" onVisibleChange={onVisibleChange} />);
    expect(onVisibleChange).toHaveBeenLastCalledWith(true);
  });
});
