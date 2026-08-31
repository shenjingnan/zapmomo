import { Badge } from "@/components/ui/badge";
import type { VoiceSessionStatus, VoiceStatusTone } from "./voiceSessionStatus";

/** 语义色 → 徽标配色（与 ModelSummary / 概览卡的状态色语义一致）。 */
const TONE_CLASS: Record<VoiceStatusTone, string> = {
  good: "bg-emerald-500/10 text-emerald-600",
  idle: "bg-text-muted/10 text-text-muted",
  loading: "bg-blue-500/10 text-blue-600",
  warn: "bg-amber-500/10 text-amber-600",
  error: "bg-red-500/10 text-red-600",
};

/** 语音会话状态徽标：状态文案与色调由 `voiceSessionStatus` 统一推导。 */
export function VoiceStatusBadge({ status }: { status: VoiceSessionStatus }) {
  return <Badge className={TONE_CLASS[status.tone]}>{status.label}</Badge>;
}
