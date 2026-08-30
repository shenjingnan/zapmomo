import { useMemo } from "react";
import { CapabilityOverview } from "@/components/home/CapabilityOverview";
import { CurrentCompanionCard } from "@/components/home/CurrentCompanionCard";
import { deriveOverview } from "@/components/home/overviewMeta";
import { useCompanionLibrary } from "@/hooks/useCompanionLibrary";
import { useRuntime } from "@/providers/RuntimeContext";

/**
 * 概览页：当前是谁 + AI 能力状态。
 *
 * - 左（45%）：当前伙伴卡片（Live2D 预览 + 名称/使用中 + 桌宠尺寸）；
 * - 右（55%）：AI 能力 2×2 状态卡（纯展示）。
 *
 * 状态全部读取真实业务数据（useCompanionLibrary + useRuntime），页面本身无独立状态源。
 */
export function HomePage() {
  const { library, loading, error } = useCompanionLibrary();
  const { kws, asr, llm, tts, speaker, voice } = useRuntime();

  const companion = useMemo(
    () => library?.models.find((m) => m.id === library.active_model_id) ?? null,
    [library],
  );

  const statuses = useMemo(
    () => deriveOverview({ kws, asr, llm, tts, speaker, voice }),
    [kws, asr, llm, tts, speaker, voice],
  );

  return (
    <div className="flex h-full flex-col gap-4 overflow-hidden">
      <div>
        <h1 className="text-xl font-semibold tracking-tight text-text-primary">概览</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">查看你的桌面伙伴与 AI 能力状态</p>
      </div>

      <div className="grid min-h-0 flex-1 gap-4 lg:grid-cols-[45fr_55fr]">
        <CurrentCompanionCard companion={companion} loading={loading} error={error} />
        <CapabilityOverview statuses={statuses} />
      </div>
    </div>
  );
}
