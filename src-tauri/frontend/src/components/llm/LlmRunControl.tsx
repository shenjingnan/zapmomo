import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import { isLlmConfigured, llmStatus, STATUS_COLOR } from "./llmMeta";

/**
 * 标题行右侧的运行控制：连接/断开开关 + 状态反馈。
 * 开关 = 操作（checked 绑定 llm.ready），右侧文字 = 反馈
 * （未配置/未连接/连接中/已连接/生成中/错误）。
 * 未配置 / 连接中 / 生成中时开关禁用，防止重复连接或生成中断开。
 */
export function LlmRunControl() {
  const { llm } = useRuntime();
  const configured = isLlmConfigured(llm.config);
  const status = llmStatus(llm.config, llm);

  const handleToggle = (on: boolean) => {
    if (on) void llm.load();
    else void llm.unload();
  };

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex items-center gap-2.5">
        <span
          className={cn(
            "inline-flex items-center gap-1.5 text-sm font-medium",
            STATUS_COLOR[status.tone],
          )}
          title={llm.error ?? undefined}
        >
          <span className="h-1.5 w-1.5 rounded-full bg-current" />
          {status.label}
        </span>
        <Switch
          aria-label="连接开关"
          checked={llm.ready}
          onCheckedChange={handleToggle}
          disabled={!configured || llm.loading || llm.generating}
          trackClass="bg-emerald-500"
        />
      </div>
      {llm.error && (
        <span className="max-w-xs truncate text-right text-xs text-destructive" title={llm.error}>
          {llm.error}
        </span>
      )}
    </div>
  );
}
