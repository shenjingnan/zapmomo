import type { LlmConfigInfo } from "@/types/tauri";

/** 状态语义色：绿=ready、蓝=连接中/生成中、灰=未配置/未连接、红=错误。 */
export type LlmStatusTone = "good" | "loading" | "idle" | "error";

export const STATUS_COLOR: Record<LlmStatusTone, string> = {
  good: "text-emerald-600",
  loading: "text-blue-600",
  idle: "text-text-muted",
  error: "text-red-600",
};

/** 当前模型展示名：远程配置里的模型名（如 glm-4.7-flash），未配置返回 null。 */
export function currentModelName(cfg: LlmConfigInfo | null): string | null {
  const model = cfg?.model?.trim();
  return model ? model : null;
}

/** 是否已填写可连接的配置（base_url + model 均非空）。 */
export function isLlmConfigured(cfg: LlmConfigInfo | null): boolean {
  return Boolean(cfg?.base_url?.trim()) && Boolean(cfg?.model?.trim());
}

/**
 * 第 4 列「状态」完整状态机（判断顺序：错误 > 连接中 > 生成中 > 已连接 > 未连接 > 未配置）。
 * `configError`（get_llm_config 失败）不进入此状态机，由调用方单独展示。
 */
export function llmStatus(
  cfg: LlmConfigInfo | null,
  st: { ready: boolean; loading: boolean; generating: boolean; error: string | null },
): { tone: LlmStatusTone; label: string } {
  if (st.error) return { tone: "error", label: "错误" };
  if (st.loading) return { tone: "loading", label: "连接中" };
  if (st.generating) return { tone: "loading", label: "生成中" };
  if (st.ready) return { tone: "good", label: "已连接" };
  if (isLlmConfigured(cfg)) return { tone: "idle", label: "未连接" };
  return { tone: "idle", label: "未配置" };
}
