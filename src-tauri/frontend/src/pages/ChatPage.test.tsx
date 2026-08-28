import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { VoiceSessionState } from "@/hooks/useVoiceSession";
import { ChatPage } from "./ChatPage";

const { state } = vi.hoisted(() => {
  const makeVoice = (over: Partial<VoiceSessionState> = {}): VoiceSessionState => ({
    running: false,
    phase: "idle",
    partial: "",
    records: [],
    pendingReply: "",
    turnUserText: "",
    replyDone: false,
    queuedSentences: [],
    currentSentence: null,
    error: null,
    pending: false,
    start: vi.fn().mockResolvedValue(undefined),
    stop: vi.fn().mockResolvedValue(undefined),
    clearRecords: vi.fn().mockResolvedValue(undefined),
    ...over,
  });
  return { state: { voice: makeVoice(), capabilities: { kws: true, asr: true } } };
});

vi.mock("@/providers/RuntimeContext", () => ({
  useRuntime: () => ({
    voice: state.voice,
    // 默认 KWS/ASR 都启用（语音互动可用）；个别用例可改写 state.capabilities
    kws: { config: { config: { enabled: state.capabilities.kws, models_present: true } } },
    asr: { config: { config: { enabled: state.capabilities.asr, models_present: true } } },
  }),
}));

describe("ChatPage", () => {
  it("待唤醒状态显示徽标与空态提示", () => {
    state.voice = { ...state.voice, running: true, phase: "armed" };
    render(<ChatPage />);
    expect(screen.getByText("待唤醒")).toBeTruthy();
    expect(screen.getByText(/待唤醒中，喊唤醒词开始对话/)).toBeTruthy();
  });

  it("未启动显示空态与「未启动」徽标", () => {
    state.voice = { ...state.voice, running: false, phase: "idle" };
    render(<ChatPage />);
    expect(screen.getByText("未启动")).toBeTruthy();
    expect(screen.getByText(/打开开关后/)).toBeTruthy();
  });

  it("渲染记录气泡与实时字幕", () => {
    state.voice = {
      ...state.voice,
      running: true,
      phase: "listening",
      records: [{ role: "user", text: "你好", at: "2026-08-19T12:00:00" }],
      partial: "正在说",
    };
    render(<ChatPage />);
    // 用户气泡不再有「你」标签，靠右侧深色气泡区分；文本与实时字幕仍在
    expect(screen.getByText("你好")).toBeTruthy();
    expect(screen.getByText("正在说")).toBeTruthy();
    expect(screen.queryByText("你")).toBeNull();
  });

  it("渲染桌宠记录气泡", () => {
    state.voice = {
      ...state.voice,
      running: true,
      phase: "speaking",
      records: [{ role: "assistant", text: "好的，我记住了。", at: "2026-08-19T12:00:05" }],
    };
    render(<ChatPage />);
    // 桌宠气泡不再有「桌宠」标签，靠左侧浅色气泡区分；文本仍在
    expect(screen.getByText("好的，我记住了。")).toBeTruthy();
    expect(screen.queryByText("桌宠")).toBeNull();
  });

  it("渲染流式回复与正在播报指示", () => {
    state.voice = {
      ...state.voice,
      running: true,
      phase: "speaking",
      pendingReply: "今天天气不错。",
      currentSentence: "今天天气不错。",
      replyDone: false,
    };
    render(<ChatPage />);
    expect(screen.getByText("今天天气不错。")).toBeTruthy();
    expect(screen.getByText(/正在播报：今天天气不错。/)).toBeTruthy();
  });

  it("开关关闭时调用 stop", async () => {
    state.voice = { ...state.voice, running: true, phase: "armed" };
    render(<ChatPage />);
    await userEvent.click(screen.getByRole("switch"));
    expect(state.voice.stop).toHaveBeenCalled();
  });

  it("开关打开时调用 start", async () => {
    state.voice = { ...state.voice, running: false, phase: "idle" };
    render(<ChatPage />);
    await userEvent.click(screen.getByRole("switch"));
    expect(state.voice.start).toHaveBeenCalled();
  });

  it("无记录时清空按钮禁用", () => {
    state.voice = { ...state.voice, running: false, phase: "idle", records: [] };
    render(<ChatPage />);
    expect(screen.getByRole("button", { name: "清空" })).toBeDisabled();
  });

  it("有记录时点击清空调用 clearRecords", async () => {
    state.voice = {
      ...state.voice,
      running: false,
      phase: "idle",
      records: [{ role: "user", text: "你好", at: "2026-08-19T12:00:00" }],
    };
    render(<ChatPage />);
    await userEvent.click(screen.getByRole("button", { name: "清空" }));
    expect(state.voice.clearRecords).toHaveBeenCalled();
  });

  it("错误显示 Alert", () => {
    state.voice = { ...state.voice, running: false, phase: "idle", error: "缺模型" };
    render(<ChatPage />);
    expect(screen.getByText("语音会话异常")).toBeTruthy();
    expect(screen.getByText("缺模型")).toBeTruthy();
  });

  it("欢迎中/等说话阶段显示对应徽标", () => {
    state.voice = { ...state.voice, running: true, phase: "greeting" };
    render(<ChatPage />);
    expect(screen.getByText("欢迎中")).toBeTruthy();

    state.voice = { ...state.voice, phase: "waiting_speech" };
    render(<ChatPage />);
    expect(screen.getAllByText("聆听中").length).toBeGreaterThan(0);
  });

  it("KWS 或 ASR 未启用时提示语音互动不可用", () => {
    state.capabilities = { kws: false, asr: true };
    state.voice = { ...state.voice, running: false, phase: "idle" };
    render(<ChatPage />);
    expect(screen.getByText("语音互动未启用")).toBeTruthy();
    expect(screen.getByText(/同时启用「唤醒词」.*「语音识别」/)).toBeTruthy();
  });
});
