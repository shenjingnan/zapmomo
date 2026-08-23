import type { TtsConfigInfo, TtsVoice } from "@/types/tauri";

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
    case "vits":
      return "VITS";
    case "matcha":
      return "Matcha";
    case "kokoro":
      return "Kokoro";
    case "pocket":
      return "PocketTTS";
    case "omnivoice":
      return "OmniVoice 克隆";
    default:
      return "TTS";
  }
}

/** Kokoro 音色分组（与后端 `KokoroVoiceGroup` serde snake_case 对应）。 */
export type KokoroGroup = "english_female" | "chinese_female" | "chinese_male";

/** 分组展示顺序与中文标签（中文优先：女声 → 男声 → 英文）。 */
const KOKORO_GROUP_ORDER: Array<{ group: KokoroGroup; label: string }> = [
  { group: "chinese_female", label: "中文女声" },
  { group: "chinese_male", label: "中文男声" },
  { group: "english_female", label: "英文女声" },
];

/**
 * 把 Kokoro 音色列表按语言分组（分组下拉数据源）。
 * 非分组音色（group 为空，如 zipvoice 参考音色混入）归入空分组排除。
 */
export function groupKokoroVoices(
  voices: TtsVoice[],
): Array<{ group: KokoroGroup; label: string; items: TtsVoice[] }> {
  return KOKORO_GROUP_ORDER.map(({ group, label }) => ({
    group,
    label,
    items: voices.filter((v) => v.group === group),
  })).filter((g) => g.items.length > 0);
}
