import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useVoiceSession } from "./useVoiceSession";

const { invokeMock, listenMock, eventHandlers } = vi.hoisted(() => {
  const handlers: Record<string, (payload: unknown) => void> = {};
  return {
    invokeMock: vi.fn().mockResolvedValue(undefined),
    listenMock: vi.fn((event: string, cb: (e: { payload: unknown }) => void) => {
      handlers[event] = (payload) => cb({ payload });
      return Promise.resolve(() => {});
    }),
    eventHandlers: handlers,
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

function emit(event: string, payload: unknown) {
  act(() => {
    eventHandlers[event]?.(payload);
  });
}

function Probe() {
  const voice = useVoiceSession();
  return (
    <div>
      <span data-testid="running">{String(voice.running)}</span>
      <span data-testid="phase">{voice.phase}</span>
      <span data-testid="enabled">{String(voice.enabled)}</span>
      <span data-testid="partial">{voice.partial}</span>
      <span data-testid="reply">{voice.pendingReply}</span>
      <span data-testid="turnUserText">{voice.turnUserText}</span>
      <span data-testid="turnSeq">{voice.turnSeq}</span>
      <span data-testid="current">{voice.currentSentence ?? ""}</span>
      <span data-testid="records">{voice.records.map((r) => `${r.role}:${r.text}`).join("|")}</span>
      <span data-testid="error">{voice.error ?? ""}</span>
      <button type="button" data-testid="enable" onClick={() => void voice.setEnabled(true)}>
        enable
      </button>
      <button type="button" data-testid="disable" onClick={() => void voice.setEnabled(false)}>
        disable
      </button>
      <button type="button" data-testid="clear" onClick={() => void voice.clearRecords()}>
        clear
      </button>
    </div>
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  invokeMock.mockResolvedValue(undefined);
});

describe("useVoiceSession", () => {
  it("回读后端运行态与持久化启用态、载入持久化记录，事件驱动状态", async () => {
    // 顺序：is_voice_session_running → get_voice_enabled → get_conversation_records
    invokeMock.mockResolvedValueOnce(true);
    invokeMock.mockResolvedValueOnce(true);
    invokeMock.mockResolvedValueOnce([
      { role: "user", text: "昨天的你好", at: "2026-08-18T10:00:00" },
      { role: "assistant", text: "你好呀", at: "2026-08-18T10:00:01" },
    ]);
    render(<Probe />);
    await waitFor(() => expect(screen.getByTestId("running").textContent).toBe("true"));
    await waitFor(() => expect(screen.getByTestId("enabled").textContent).toBe("true"));
    await waitFor(() =>
      expect(screen.getByTestId("records").textContent).toBe("user:昨天的你好|assistant:你好呀"),
    );

    emit("voice-session-state", { running: true, state: "armed" });
    expect(screen.getByTestId("phase").textContent).toBe("armed");

    emit("voice-session-transcript", { text: "你", is_final: false });
    expect(screen.getByTestId("partial").textContent).toBe("你");
    emit("voice-session-transcript", { text: "你好", is_final: true });
    expect(screen.getByTestId("partial").textContent).toBe("");
    expect(screen.getByTestId("records").textContent).toContain("user:你好");
  });

  it("transcript is_final 置位 turnUserText，新一轮顶替，打断不清", () => {
    render(<Probe />);
    emit("voice-session-transcript", { text: "第一句", is_final: true });
    expect(screen.getByTestId("turnUserText").textContent).toBe("第一句");
    // partial 不置位
    emit("voice-session-transcript", { text: "说到一半", is_final: false });
    expect(screen.getByTestId("turnUserText").textContent).toBe("第一句");
    // 打断（state 回 armed）保留——静置语义：想看的内容不被程序收走
    emit("voice-session-state", { running: true, state: "armed" });
    expect(screen.getByTestId("turnUserText").textContent).toBe("第一句");
    // 新一轮顶替
    emit("voice-session-transcript", { text: "第二句", is_final: true });
    expect(screen.getByTestId("turnUserText").textContent).toBe("第二句");
  });

  it("同文本连发 turnSeq 仍递增；partial 不递增，打断不清零", () => {
    render(<Probe />);
    emit("voice-session-transcript", { text: "继续", is_final: true });
    expect(screen.getByTestId("turnSeq").textContent).toBe("1");
    // partial 不递增
    emit("voice-session-transcript", { text: "说到一半", is_final: false });
    expect(screen.getByTestId("turnSeq").textContent).toBe("1");
    // 打断（state 回 armed）不清轮序
    emit("voice-session-state", { running: true, state: "armed" });
    expect(screen.getByTestId("turnSeq").textContent).toBe("1");
    // 同文本连发仍自增（气泡据此判新轮，值比较判不出）
    emit("voice-session-transcript", { text: "继续", is_final: true });
    expect(screen.getByTestId("turnSeq").textContent).toBe("2");
  });

  it("打断回 armed/idle 清空 partial：聆听中断不留残句（partial 仅聆听态有意义）", () => {
    render(<Probe />);
    emit("voice-session-transcript", { text: "说到一半", is_final: false });
    expect(screen.getByTestId("partial").textContent).toBe("说到一半");
    emit("voice-session-state", { running: true, state: "armed" });
    expect(screen.getByTestId("partial").textContent).toBe("");
    // idle 同样清
    emit("voice-session-transcript", { text: "又说一半", is_final: false });
    emit("voice-session-state", { running: false, state: "idle" });
    expect(screen.getByTestId("partial").textContent).toBe("");
  });

  it("LLM token 累积为 pendingReply，reply-finished 提交桌宠记录并清空", () => {
    render(<Probe />);
    emit("voice-session-token", { delta: "今天" });
    emit("voice-session-token", { delta: "天气不错。" });
    expect(screen.getByTestId("reply").textContent).toBe("今天天气不错。");

    emit("voice-session-play", { sentence: "今天天气不错。" });
    expect(screen.getByTestId("current").textContent).toBe("今天天气不错。");

    emit("voice-session-reply-finished", { reason: "Eos", text: "今天天气不错。" });
    expect(screen.getByTestId("reply").textContent).toBe("");
    expect(screen.getByTestId("records").textContent).toContain("assistant:今天天气不错。");
  });

  it("reply-finished 空回复（text null）不落空行", () => {
    render(<Probe />);
    emit("voice-session-reply-finished", { reason: "Eos", text: null });
    expect(screen.getByTestId("records").textContent).toBe("");
  });

  it("打断（state 回 armed）清空未提交的 pendingReply", () => {
    render(<Probe />);
    emit("voice-session-state", { running: true, state: "thinking" });
    emit("voice-session-token", { delta: "正在回复" });
    expect(screen.getByTestId("reply").textContent).toBe("正在回复");
    emit("voice-session-state", { running: true, state: "armed" });
    expect(screen.getByTestId("reply").textContent).toBe("");
  });

  it("stopped 复位为 idle 并透传错误", () => {
    render(<Probe />);
    emit("voice-session-state", { running: true, state: "speaking" });
    emit("voice-session-stopped", { error: "缺模型" });
    expect(screen.getByTestId("running").textContent).toBe("false");
    expect(screen.getByTestId("phase").textContent).toBe("idle");
    expect(screen.getByTestId("error").textContent).toBe("缺模型");
  });

  it("setEnabled 调用 set_voice_enabled（持久化 + 启停原子），乐观更新本地 enabled", async () => {
    const user = userEvent.setup();
    render(<Probe />);
    expect(screen.getByTestId("enabled").textContent).toBe("true");

    await user.click(screen.getByTestId("disable"));
    expect(invokeMock).toHaveBeenCalledWith("set_voice_enabled", { enabled: false });
    await waitFor(() => expect(screen.getByTestId("enabled").textContent).toBe("false"));

    await user.click(screen.getByTestId("enable"));
    expect(invokeMock).toHaveBeenCalledWith("set_voice_enabled", { enabled: true });
    await waitFor(() => expect(screen.getByTestId("enabled").textContent).toBe("true"));
  });

  it("setEnabled 失败：透出错误并回读后端权威态校正开关", async () => {
    const user = userEvent.setup();
    render(<Probe />);
    // set_voice_enabled 拒绝，紧随其后的 get_voice_enabled 回读返回 false
    invokeMock.mockRejectedValueOnce(new Error("缺模型"));
    invokeMock.mockResolvedValueOnce(false);

    await user.click(screen.getByTestId("disable"));

    await waitFor(() => expect(screen.getByTestId("error").textContent).toContain("缺模型"));
    await waitFor(() => expect(screen.getByTestId("enabled").textContent).toBe("false"));
  });

  it("clearRecords 调用命令并清空记录", async () => {
    invokeMock.mockResolvedValueOnce(false);
    invokeMock.mockResolvedValueOnce(true);
    invokeMock.mockResolvedValueOnce([{ role: "user", text: "你好", at: "2026-08-19T10:00:00" }]);
    const user = userEvent.setup();
    render(<Probe />);
    await waitFor(() => expect(screen.getByTestId("records").textContent).toContain("你好"));

    await user.click(screen.getByTestId("clear"));
    expect(invokeMock).toHaveBeenCalledWith("clear_conversation_records");
    await waitFor(() => expect(screen.getByTestId("records").textContent).toBe(""));
  });
});
