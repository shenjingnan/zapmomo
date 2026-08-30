import {
  AudioLines,
  AudioWaveform,
  Brain,
  Fingerprint,
  type LucideIcon,
  Mic,
  Volume2,
} from "lucide-react";
import { isLlmConfigured } from "@/components/llm/llmMeta";
import { deriveListenerStatus, type ListenerKind } from "@/components/models/capabilityStatus";
import type { LlmState } from "@/hooks/useLlm";
import type { TtsState } from "@/hooks/useTts";
import type { VoiceSessionState } from "@/hooks/useVoiceSession";
import type { RuntimeState } from "@/providers/RuntimeContext";

/** 状态语义色（与模型页 ModelSummary / 各能力 meta 的语义完全一致）。 */
export type OverviewTone = "good" | "idle" | "loading" | "error";

export const OVERVIEW_STATUS_COLOR: Record<OverviewTone, string> = {
  good: "text-emerald-600",
  idle: "text-text-muted",
  loading: "text-blue-600",
  error: "text-red-600",
};

/** AI 能力小卡数据（纯展示：Icon + 名称 + 缩写 + 状态）。 */
export interface CapabilityStatus {
  key: "kws" | "asr" | "llm" | "tts" | "speaker" | "voice";
  name: string;
  code: string;
  icon: LucideIcon;
  accent: string;
  label: string;
  tone: OverviewTone;
}

export interface OverviewInput {
  kws: RuntimeState["kws"];
  asr: RuntimeState["asr"];
  llm: LlmState;
  tts: TtsState;
  speaker: RuntimeState["speaker"];
  voice: VoiceSessionState;
}

/** 监听型能力 kind → 概览页文案（listening 态按 KWS/ASR 区分）。 */
function listenerLabel(kind: ListenerKind, active: "监听中" | "识别中"): string {
  switch (kind) {
    case "error":
      return "异常";
    case "starting":
      return "启动中";
    case "listening":
      return active;
    case "ready":
      return "已就绪";
    case "disabled":
      return "未启用";
    case "not_configured":
      return "未配置";
  }
}

/**
 * KWS 状态：错误 > 监听中 > 已就绪/未启用 > 未配置。
 * `enabled=true` 但未在监听是合法状态（启动自动监听失败会静默降级，
 * 见 lib.rs setup），此时能力已配置并开启，展示「已就绪」而非「未启用」。
 */
function kwsStatus(kws: RuntimeState["kws"]): { label: string; tone: OverviewTone } {
  const st = deriveListenerStatus({
    error: kws.listening.error,
    isListening: kws.listening.isListening,
    enabled: kws.config.config?.enabled,
    modelsPresent: kws.config.config?.models_present,
  });
  return { label: listenerLabel(st.kind, "监听中"), tone: st.tone };
}

/** ASR 状态：错误 > 启动中 > 识别中 > 已就绪/未启用 > 未配置（与 KWS 一致：读取持久化 enabled）。 */
function asrStatus(asr: RuntimeState["asr"]): { label: string; tone: OverviewTone } {
  const st = deriveListenerStatus({
    error: asr.listening.error,
    pending: asr.listening.pending,
    isListening: asr.listening.isListening,
    enabled: asr.config.config?.enabled,
    modelsPresent: asr.config.config?.models_present,
  });
  return { label: listenerLabel(st.kind, "识别中"), tone: st.tone };
}

/** LLM 状态：错误 > 生成中 > 连接中 > 已连接 > 未连接 > 未配置（远程连接语义，词汇沿用 llmMeta）。 */
function llmStatus(llm: LlmState): { label: string; tone: OverviewTone } {
  if (llm.error) return { label: "异常", tone: "error" };
  if (llm.generating) return { label: "生成中", tone: "loading" };
  if (llm.loading) return { label: "连接中", tone: "loading" };
  if (llm.ready) return { label: "已连接", tone: "good" };
  if (isLlmConfigured(llm.config)) return { label: "未连接", tone: "idle" };
  return { label: "未配置", tone: "idle" };
}

/** TTS 状态：配置错误 > 合成中 > 未配置 > 已关闭 > 已就绪（顺序沿用 ttsMeta：模型缺失优先于已关闭）。 */
function ttsStatus(tts: TtsState): { label: string; tone: OverviewTone } {
  if (tts.configError) return { label: "异常", tone: "error" };
  if (tts.synthesizing) return { label: "合成中", tone: "loading" };
  const cfg = tts.config;
  if (!cfg) return { label: "加载中", tone: "idle" };
  if (!cfg.models_present) return { label: "未配置", tone: "idle" };
  if (cfg.enabled === false) return { label: "已关闭", tone: "idle" };
  return { label: "已就绪", tone: "good" };
}

/** 声纹识别状态：错误 > 未配置（模型缺失）> 未启用 > 已就绪（无运行态，开关即全部）。 */
function speakerStatus(speaker: RuntimeState["speaker"]): { label: string; tone: OverviewTone } {
  if (speaker.config.error) return { label: "异常", tone: "error" };
  const cfg = speaker.config.config;
  if (!cfg) return { label: "加载中", tone: "idle" };
  if (!cfg.model_present) return { label: "未配置", tone: "idle" };
  if (!cfg.enabled) return { label: "未启用", tone: "idle" };
  return { label: "已就绪", tone: "good" };
}

/** 语音会话状态：错误 > 启动中 > 欢迎中/待唤醒/聆听中/思考中/播报中 > 未启动。 */
function voiceStatus(voice: VoiceSessionState): { label: string; tone: OverviewTone } {
  if (voice.error) return { label: "异常", tone: "error" };
  if (voice.running && voice.phase === "idle") return { label: "启动中", tone: "loading" };
  switch (voice.phase) {
    case "armed":
      return { label: "待唤醒", tone: "good" };
    case "greeting":
      return { label: "欢迎中", tone: "loading" };
    case "waiting_speech":
    case "listening":
      return { label: "聆听中", tone: "good" };
    case "thinking":
      return { label: "思考中", tone: "loading" };
    case "speaking":
      return { label: "播报中", tone: "loading" };
    default:
      return { label: "未启动", tone: "idle" };
  }
}

/**
 * 概览页 AI 能力状态推导（纯函数）：基于真实 runtime 字段推导，
 * 不维护第二套状态源。顺序固定为 KWS / ASR / LLM / TTS / 声纹（与模型摘要一致）。
 */
export function deriveOverview(input: OverviewInput): CapabilityStatus[] {
  const { kws, asr, llm, tts, speaker, voice } = input;
  const kwsState = kwsStatus(kws);
  const asrState = asrStatus(asr);
  const llmState = llmStatus(llm);
  const ttsState = ttsStatus(tts);
  const speakerState = speakerStatus(speaker);
  const voiceState = voiceStatus(voice);

  return [
    {
      key: "kws",
      name: "唤醒词",
      code: "KWS",
      icon: AudioWaveform,
      accent: "bg-violet-100 text-violet-600",
      ...kwsState,
    },
    {
      key: "asr",
      name: "语音识别",
      code: "ASR",
      icon: Mic,
      accent: "bg-blue-100 text-blue-600",
      ...asrState,
    },
    {
      key: "llm",
      name: "AI 大脑",
      code: "LLM",
      icon: Brain,
      accent: "bg-emerald-100 text-emerald-600",
      ...llmState,
    },
    {
      key: "tts",
      name: "语音合成",
      code: "TTS",
      icon: Volume2,
      accent: "bg-amber-100 text-amber-600",
      ...ttsState,
    },
    {
      key: "speaker",
      name: "声纹识别",
      code: "SPK",
      icon: Fingerprint,
      accent: "bg-teal-100 text-teal-600",
      ...speakerState,
    },
    {
      key: "voice",
      name: "语音会话",
      code: "VOICE",
      icon: AudioLines,
      accent: "bg-pink-100 text-pink-600",
      ...voiceState,
    },
  ];
}
