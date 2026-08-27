import { ArrowLeft } from "lucide-react";
import { Link } from "react-router-dom";
import { LlmCoreConfig } from "@/components/llm/LlmCoreConfig";
import { LlmRunControl } from "@/components/llm/LlmRunControl";
import { LlmSystemPrompt } from "@/components/llm/LlmSystemPrompt";
import { LlmThinkingConfig } from "@/components/llm/LlmThinkingConfig";

/** AI 大脑（LLM）配置页：标题行含连接开关与状态 + 远程连接配置 + 系统提示词。 */
export function LlmPage() {
  return (
    <div className="space-y-4">
      <Link
        to="/models"
        className="inline-flex items-center gap-1.5 text-sm text-text-secondary transition-colors hover:text-text-primary"
      >
        <ArrowLeft className="h-4 w-4" />
        模型与能力
      </Link>

      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <h1 className="text-2xl font-semibold tracking-tight text-text-primary">
          AI 大脑（LLM）配置
        </h1>
        <LlmRunControl />
      </header>

      <LlmCoreConfig />

      <LlmThinkingConfig />

      <LlmSystemPrompt />
    </div>
  );
}
