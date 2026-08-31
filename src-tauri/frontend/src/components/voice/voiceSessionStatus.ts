import type { LlmState } from "@/hooks/useLlm";
import type { VoiceSessionState } from "@/hooks/useVoiceSession";
import type { VoiceSessionPhase } from "@/types/tauri";

/** 语音会话状态语义色（概览卡 / 模型页行 / 记录页徽标共用同一套语义）。 */
export type VoiceStatusTone = "good" | "idle" | "loading" | "warn" | "error";

export interface VoiceSessionStatus {
  label: string;
  tone: VoiceStatusTone;
}

const PHASE_LABEL: Record<VoiceSessionPhase, string> = {
  idle: "未启动",
  armed: "待唤醒",
  greeting: "欢迎中",
  waiting_speech: "聆听中",
  listening: "聆听中",
  thinking: "思考中",
  speaking: "播报中",
};

const PHASE_TONE: Record<VoiceSessionPhase, VoiceStatusTone> = {
  idle: "idle",
  armed: "good",
  greeting: "loading",
  waiting_speech: "good",
  listening: "good",
  thinking: "loading",
  speaking: "loading",
};

/** 大脑未就绪：LLM 未连接/未加载且不在连接中——此时听完这轮话也生成不出回复。 */
function brainNotReady(llm: Pick<LlmState, "ready" | "loading">): boolean {
  return !llm.ready && !llm.loading;
}

/**
 * 语音会话状态推导（唯一真源，三处 UI 共用，避免文案/语义漂移）：
 *
 * - 错误 > 运行阶段 > 未运行（已启用/未开启，读持久化 enabled）
 * - **就绪度修饰**：会话在听候/聆听但 LLM 未就绪时，标签追加「·大脑未就绪」并降为
 *   warn 警示色——此时喊唤醒词说完话也到不了生成环节，提前透出而不是等报错
 *   （会话线程不随引擎卸载停止，见 `voice/session.rs` 的共享引擎槽设计）。
 *   思考中/播报中阶段本身意味着引擎在工作，不加修饰。
 */
export function voiceSessionStatus(
  voice: Pick<VoiceSessionState, "running" | "phase" | "enabled" | "error">,
  llm: Pick<LlmState, "ready" | "loading">,
): VoiceSessionStatus {
  if (voice.error) return { label: "异常", tone: "error" };
  if (!voice.running) {
    return voice.enabled ? { label: "已启用", tone: "good" } : { label: "未开启", tone: "idle" };
  }
  if (voice.phase === "idle") return { label: "启动中", tone: "loading" };

  const base: VoiceSessionStatus = {
    label: PHASE_LABEL[voice.phase],
    tone: PHASE_TONE[voice.phase],
  };
  const listeningForSpeech =
    voice.phase === "armed" || voice.phase === "waiting_speech" || voice.phase === "listening";
  if (listeningForSpeech && brainNotReady(llm)) {
    return { label: `${base.label}·大脑未就绪`, tone: "warn" };
  }
  return base;
}
