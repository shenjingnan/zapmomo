import { Brain } from "lucide-react";
import { useEffect, useState } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useToast } from "@/components/ui/toast";
import { useRuntime } from "@/providers/RuntimeContext";

/** 思考力度档位（GUI 提供三档；max 仅手改 settings.toml 可达） */
const EFFORT_OPTIONS = [
  { value: "low", label: "低（最快）" },
  { value: "medium", label: "中" },
  { value: "high", label: "高" },
] as const;

/**
 * 深度思考区块：thinking 开关 + 推理强度下拉。
 *
 * 开关关闭时强度下拉置灰但保留所选值（后端同样保留字段仅忽略，重新打开即恢复）；
 * 两项变更即时保存。仅 provider = "anthropic"（原生 Messages API）生效，
 * 其他 provider 显示适用范围提示。
 */
export function LlmThinkingConfig() {
  const { llm } = useRuntime();
  const toast = useToast();
  const [thinking, setThinking] = useState(false);
  const [effort, setEffort] = useState("medium");
  const [busy, setBusy] = useState(false);

  // hydrate：config 就绪时填充（仅首次；用户操作后不覆盖）。
  // thinking 兜底 false：旧版本测试 mock / 异常后端缺字段时不至于渲染成非受控
  const [hydrated, setHydrated] = useState(false);
  useEffect(() => {
    if (hydrated || !llm.config) return;
    setThinking(llm.config.thinking ?? false);
    if (llm.config.reasoning_effort) {
      setEffort(llm.config.reasoning_effort);
    }
    setHydrated(true);
  }, [hydrated, llm.config]);

  const save = async (nextThinking: boolean, nextEffort?: string) => {
    setBusy(true);
    try {
      await llm.setThinkingParams(nextThinking, nextEffort);
      return true;
    } catch (e) {
      toast.error(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const handleToggle = async (v: boolean) => {
    setThinking(v); // 乐观更新，失败由 refreshConfig 回读纠正
    if (!(await save(v))) {
      setThinking(!v);
    }
  };

  const handleEffortChange = async (v: string) => {
    const prev = effort;
    setEffort(v);
    if (!(await save(true, v))) {
      setEffort(prev);
    }
  };

  const isAnthropic = llm.config?.provider === "anthropic";

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <Brain className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">深度思考</h2>
            <p className="mt-0.5 text-xs text-text-muted">
              让模型先推理再回答（回答更准但更慢），关闭可获得最快响应。仅 Anthropic 原生接口生效。
            </p>
          </div>
        </div>
      </div>

      <div className="space-y-2.5 px-3.5 pb-3">
        <div className="flex items-center justify-between gap-4 text-sm">
          <span>启用深度思考</span>
          <Switch
            aria-label="启用深度思考"
            checked={thinking}
            disabled={busy || !llm.config}
            onCheckedChange={(v) => void handleToggle(v)}
          />
        </div>
        <div className="flex items-center justify-between gap-4 text-sm">
          <span className={!thinking ? "text-text-muted" : undefined}>推理强度</span>
          <Select
            value={effort}
            disabled={!thinking || busy || !llm.config}
            onValueChange={(v) => void handleEffortChange(v)}
          >
            <SelectTrigger className="w-36" aria-label="推理强度">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {EFFORT_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {!isAnthropic && (
          <p className="text-xs text-text-muted">
            当前 provider 为「{llm.config?.provider ?? "未知"}」，本设置对其不生效。
          </p>
        )}
      </div>
    </section>
  );
}
