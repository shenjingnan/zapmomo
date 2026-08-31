import { Badge } from "@/components/ui/badge";
import type { VoiceSessionPhase } from "@/types/tauri";

const PHASE_META: Record<VoiceSessionPhase, { label: string; className: string }> = {
  idle: { label: "未启动", className: "bg-text-muted/10 text-text-muted" },
  armed: { label: "待唤醒", className: "bg-emerald-500/10 text-emerald-600" },
  greeting: { label: "欢迎中", className: "bg-violet-500/10 text-violet-600" },
  waiting_speech: { label: "聆听中", className: "bg-blue-500/10 text-blue-600" },
  listening: { label: "聆听中", className: "bg-blue-500/10 text-blue-600" },
  thinking: { label: "思考中", className: "bg-amber-500/10 text-amber-600" },
  speaking: { label: "播报中", className: "bg-violet-500/10 text-violet-600" },
};

/**
 * 语音会话阶段文案（徽标与模型摘要行共用，避免两处漂移）：
 * `running` 但阶段仍为 idle（会话线程启动/LLM 加载中，尚未收到后端 state 事件）时为「启动中」。
 */
export function voicePhaseLabel(phase: VoiceSessionPhase, running: boolean): string {
  if (running && phase === "idle") return "启动中";
  return PHASE_META[phase].label;
}

/**
 * 语音会话状态徽标：阶段文案见 {@link voicePhaseLabel}。
 */
export function VoiceStatusBadge({
  phase,
  running,
}: {
  phase: VoiceSessionPhase;
  running: boolean;
}) {
  if (running && phase === "idle") {
    return <Badge className="bg-blue-500/10 text-blue-600">启动中</Badge>;
  }
  const meta = PHASE_META[phase];
  return <Badge className={meta.className}>{meta.label}</Badge>;
}
