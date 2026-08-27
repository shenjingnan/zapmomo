import {
  AudioWaveform,
  Brain,
  ChevronRight,
  Database,
  type LucideIcon,
  Mic,
  RefreshCw,
  Volume2,
} from "lucide-react";
import { type ReactNode, useState } from "react";
import { Link } from "react-router-dom";
import { AsrModelSwitchMenu } from "@/components/models/AsrModelSwitchMenu";
import {
  deriveListenerStatus,
  type ListenerKind,
  type ListenerStatus,
} from "@/components/models/capabilityStatus";
import { KwsModelSwitchMenu } from "@/components/models/KwsModelSwitchMenu";
import { currentModelName, isLlmConfigured } from "@/components/llm/llmMeta";
import { TtsModelSwitchMenu } from "@/components/models/TtsModelSwitchMenu";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";

type StatusTone = "good" | "idle" | "loading" | "error";

const STATUS_COLOR: Record<StatusTone, string> = {
  good: "text-emerald-600",
  idle: "text-text-muted",
  loading: "text-blue-600",
  error: "text-red-600",
};

/** KWS/ASR 语义 kind → 本页文案（主动态「监听中/识别中」由调用方按能力指定）。 */
const LISTENER_TEXT: Record<ListenerKind, string> = {
  error: "错误",
  starting: "启动中",
  listening: "",
  ready: "已就绪",
  disabled: "未启用",
  not_configured: "未配置模型",
};

/** 把共享推导结果映射为本页的行状态（listening 用 activeLabel 区分 KWS/ASR）。 */
function listenerRow(st: ListenerStatus, activeLabel: string): { text: string; tone: StatusTone } {
  return {
    text: st.kind === "listening" ? activeLabel : LISTENER_TEXT[st.kind],
    tone: st.tone,
  };
}

interface SummaryRowData {
  accent: string;
  icon: LucideIcon;
  name: string;
  /** 模型名（字符串）或自定义展示（KWS/ASR/TTS 行为快速切换下拉；LLM 行展示远程模型名）。 */
  model: ReactNode;
  statusText: string;
  statusTone: StatusTone;
  gearHref?: string;
  toggled: boolean;
  onToggle: () => void;
  toggleDisabled?: boolean;
}

/** 模型摘要单行：整行可点击进入对应配置页；右侧状态 + chevron 指示。 */
function SummaryRow({ row }: { row: SummaryRowData }) {
  const Icon = row.icon;
  const content = (
    <>
      <span
        className={cn("flex h-9 w-9 shrink-0 items-center justify-center rounded-full", row.accent)}
      >
        <Icon className="h-4 w-4" />
      </span>

      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-text-primary">{row.name}</p>
        <p className="truncate text-xs text-text-secondary">{row.model}</p>
      </div>

      <div className="flex shrink-0 items-center gap-3">
        {/* 行内开关：拦截点击冒泡，避免触发整行 Link 导航到配置页 */}
        {/* biome-ignore lint/a11y/noStaticElementInteractions: 静态容器仅拦截鼠标冒泡，交互由内部 Switch 承载 */}
        {/* biome-ignore lint/a11y/useKeyWithClickEvents: 仅拦截点击冒泡防误触导航，键盘交互由内部 Switch 处理 */}
        <span
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          <Switch
            checked={row.toggled}
            onCheckedChange={row.onToggle}
            disabled={row.toggleDisabled}
            trackClass="bg-emerald-500"
            aria-label={`${row.name}开关`}
          />
        </span>
        <span
          className={cn(
            "flex items-center gap-1.5 whitespace-nowrap text-xs",
            STATUS_COLOR[row.statusTone],
          )}
        >
          <span className="h-1.5 w-1.5 rounded-full bg-current" />
          {row.statusText}
        </span>
        {row.gearHref && <ChevronRight className="h-4 w-4 shrink-0 text-text-muted" />}
      </div>
    </>
  );

  const rowClass = "flex items-center gap-4 px-5 py-3.5";

  if (!row.gearHref) {
    return <div className={rowClass}>{content}</div>;
  }

  return (
    <Link
      to={row.gearHref}
      aria-label={`配置${row.name}`}
      className={cn(rowClass, "transition-colors hover:bg-nav-hover")}
    >
      {content}
    </Link>
  );
}

/** 模型摘要：分组 List（macOS Settings 风格），非 DataTable。 */
export function ModelSummary() {
  const { kws, asr, llm, tts, device, sessionKeywords } = useRuntime();
  const [refreshing, setRefreshing] = useState(false);

  const refreshAll = async () => {
    setRefreshing(true);
    try {
      await Promise.all([
        kws.config.refresh(),
        asr.config.refresh(),
        llm.refreshConfig(),
        tts.refreshConfig(),
      ]);
    } finally {
      setRefreshing(false);
    }
  };

  const llmConfigured = isLlmConfigured(llm.config);
  const asrConfigured = asr.config?.config?.models_present ?? false;
  const kwsConfigured = kws.config?.config?.models_present ?? false;
  const ttsConfigured = tts.config?.models_present ?? false;
  const ttsEnabled = tts.config?.enabled ?? true;

  // KWS/ASR 开关绑定**持久化 enabled**（模型与能力页启用/禁用能力，重启保持）
  const asrOn = asr.config?.config?.enabled ?? false;
  const kwsOn = kws.config?.config?.enabled ?? false;

  /** KWS 开关：持久化「启用」+ 立即开始/停止监听（与配置页 KwsRunControl 一致）。 */
  const handleKwsToggle = () => {
    if (kwsOn) {
      if (kws.listening.isListening) void kws.listening.stop();
      void kws.config.setEnabled(false);
    } else {
      void kws.config.setEnabled(true);
      void kws.listening.start(device || null, sessionKeywords || null);
    }
  };

  /** ASR 开关：持久化「启用」+ 立即开始/停止识别（与配置页一致）。 */
  const handleAsrToggle = () => {
    if (asrOn) {
      if (asr.listening.isListening) void asr.listening.stop();
      void asr.config.setEnabled(false);
    } else {
      void asr.config.setEnabled(true);
      void asr.listening.start(device || null);
    }
  };

  // KWS/ASR 行状态：共享推导读取持久化 enabled（启用→已就绪，关闭→未启用）。
  const kwsSummary = listenerRow(
    deriveListenerStatus({
      error: kws.listening.error,
      isListening: kws.listening.isListening,
      enabled: kws.config?.config?.enabled,
      modelsPresent: kwsConfigured,
    }),
    "监听中",
  );
  const asrSummary = listenerRow(
    deriveListenerStatus({
      error: asr.listening.error,
      pending: asr.listening.pending,
      isListening: asr.listening.isListening,
      enabled: asr.config?.config?.enabled,
      modelsPresent: asrConfigured,
    }),
    "识别中",
  );

  const rows: SummaryRowData[] = [
    {
      accent: "bg-violet-100 text-violet-600",
      icon: AudioWaveform,
      name: "唤醒词（KWS）",
      model: kwsConfigured ? <KwsModelSwitchMenu /> : "未配置模型",
      statusText: kwsSummary.text,
      statusTone: kwsSummary.tone,
      gearHref: "/models/kws",
      toggled: kwsOn,
      onToggle: handleKwsToggle,
    },
    {
      accent: "bg-blue-100 text-blue-600",
      icon: Mic,
      name: "语音识别（ASR）",
      model: asrConfigured ? <AsrModelSwitchMenu /> : "未配置模型",
      statusText: asrSummary.text,
      statusTone: asrSummary.tone,
      gearHref: "/models/asr",
      toggled: asrOn,
      onToggle: handleAsrToggle,
    },
    {
      accent: "bg-emerald-100 text-emerald-600",
      icon: Brain,
      name: "AI 大脑（LLM）",
      model: currentModelName(llm.config) ?? "未配置模型",
      statusText: llm.error
        ? "错误"
        : llm.loading
          ? "连接中"
          : llm.ready
            ? "已连接"
            : llmConfigured
              ? "未连接"
              : "未配置模型",
      statusTone: llm.error ? "error" : llm.loading ? "loading" : llm.ready ? "good" : "idle",
      gearHref: "/models/llm",
      toggled: llm.ready,
      onToggle: () => (llm.ready ? llm.unload() : llm.load()),
      toggleDisabled: llm.loading || !llmConfigured,
    },
    {
      accent: "bg-amber-100 text-amber-600",
      icon: Volume2,
      name: "语音合成（TTS）",
      model: ttsConfigured ? <TtsModelSwitchMenu /> : "未配置模型",
      statusText: !ttsEnabled
        ? "已关闭"
        : tts.synthesizing
          ? "合成中"
          : ttsConfigured
            ? "已就绪"
            : "未配置模型",
      statusTone: !ttsEnabled
        ? "idle"
        : tts.synthesizing
          ? "loading"
          : ttsConfigured
            ? "good"
            : "idle",
      gearHref: "/models/tts",
      toggled: ttsEnabled,
      onToggle: () => tts.setEnabled(!ttsEnabled),
    },
  ];

  return (
    <section className="rounded-[16px] border border-panel-border bg-panel-background">
      <div className="flex flex-wrap items-center justify-between gap-2 px-5 py-4">
        <h2 className="text-base font-semibold text-text-primary">模型摘要</h2>
        <div className="flex gap-2">
          <Button variant="ghost" size="sm" onClick={refreshAll} disabled={refreshing}>
            <RefreshCw className={cn("h-4 w-4", refreshing && "animate-spin")} />
            刷新状态
          </Button>
          <Button variant="outline" size="sm" className="shadow-none" asChild>
            <Link to="/models/library">
              <Database className="h-4 w-4" />
              管理模型
            </Link>
          </Button>
        </div>
      </div>
      <div className="divide-y divide-[#eef1f6]">
        {rows.map((row) => (
          <SummaryRow key={row.name} row={row} />
        ))}
      </div>
    </section>
  );
}
