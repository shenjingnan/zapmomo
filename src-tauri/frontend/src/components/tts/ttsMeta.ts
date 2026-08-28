import type { TtsConfigInfo } from "@/types/tauri";

/** 状态语义色：绿=已就绪、蓝=合成中、灰=未下载/已关闭/加载中、红=配置错误。 */
export type TtsStatusTone = "good" | "loading" | "idle" | "error";

export const TTS_STATUS_COLOR: Record<TtsStatusTone, string> = {
  good: "text-emerald-600",
  loading: "text-blue-600",
  idle: "text-text-muted",
  error: "text-red-600",
};

/** 从 model_dir 派生展示名：取 basename。空路径返回 null，不硬编码任何模型名。 */
export function modelNameFromDir(dir: string | null | undefined): string | null {
  if (!dir) return null;
  return dir.split(/[\\/]/).pop() ?? dir;
}

/**
 * TTS 顶部状态机（判断顺序：配置错误 > 合成中 > 模型缺失 > 已关闭 > 已就绪）。
 * 模型缺失优先于 enabled=false，避免 enabled=false 且模型缺失时只显示「已关闭」而掩盖未下载。
 * 合成错误（tts.error）属于测试会话级错误，在 TestDialog 内展示，不进入顶部状态机（与概览页一致）。
 */
export function ttsStatus(
  cfg: TtsConfigInfo | null,
  synthesizing: boolean,
  configError: string | null,
): { tone: TtsStatusTone; label: string } {
  if (configError) return { tone: "error", label: "配置错误" };
  if (synthesizing) return { tone: "loading", label: "合成中" };
  if (!cfg) return { tone: "idle", label: "加载中" };
  if (!cfg.models_present) return { tone: "idle", label: "未下载模型" };
  if (cfg.enabled === false) return { tone: "idle", label: "已关闭" };
  return { tone: "good", label: "已就绪" };
}

/** 模型类型徽标文案（选择模型弹窗 / 音色语义判断共用）。 */
export function ttsModelKindLabel(kind: string): string {
  switch (kind) {
    case "zipvoice":
      return "ZipVoice 克隆";
    case "omnivoice":
      return "OmniVoice 克隆";
    case "voxcpm2":
      return "VoxCPM2 克隆";
    case "qwen3_tts_06":
    case "qwen3_tts_17":
      return "Qwen3-TTS 克隆";
    default:
      return "TTS";
  }
}

/**
 * 参考音频克隆族（共享音色库与音色管理入口）：
 * zipvoice（sherpa）/ omnivoice / voxcpm2 / qwen3_tts 两尺寸（audiocpp）。
 * 音色语义判断（TtsBasicConfig / TtsTestDialog）的统一事实源，新增克隆族只改这里。
 */
export function isCloneTtsKind(kind: string): boolean {
  return (
    kind === "zipvoice" ||
    kind === "omnivoice" ||
    kind === "voxcpm2" ||
    kind === "qwen3_tts_06" ||
    kind === "qwen3_tts_17"
  );
}

/**
 * 强制克隆族（qwen3_tts 两尺寸）：上游 Base 无 auto voice 兜底、包内无内置音色，
 * 必须选择克隆音色——无「默认（自动音色）」空值项，音色库为空时下拉禁用。
 */
export function isCloneRequiredTtsKind(kind: string): boolean {
  return kind === "qwen3_tts_06" || kind === "qwen3_tts_17";
}
