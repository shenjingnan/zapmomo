/**
 * dsh 集成卡片状态机：把后端文件级检测 + 桥运行状态 + 心跳新鲜度合成为展示状态。
 *
 * 纯函数（时间由调用方注入），便于 vitest 直接覆盖各态组合；与 Rust 侧
 * `zapmomo::dsh::integration::detect` 的四个布尔一一对应。
 */
import type { DshIntegrationInfo } from "@/types/tauri";

/** 心跳在线窗口：45s（插件 15s 心跳间隔的 3 倍容错）。 */
export const ONLINE_WINDOW_MS = 45_000;

/** 集成卡片的展示状态（按引导优先级排列）。 */
export type IntegrationState =
  | "no-dsh" // 未检测到 dsh（~/.dsh 不存在）
  | "no-profile" // 有 dsh 但 web profile 未初始化（没跑过 dsh web）
  | "not-installed" // profile 就绪但插件未安装 → 一键安装
  | "half-activated" // 已安装但未激活（bundles 缺失）→ 展示修复命令
  | "awaiting-restart" // 已激活但桥无新鲜心跳（dsh web 未跑 / 未重启 / 桥未开）
  | "online"; // 在线：桥运行中且心跳新鲜

/**
 * 合成集成状态。`nowMs` 注入以便测试；心跳时间戳来自未来（时钟回拨）按过期算。
 * 桥未运行时 online 不可达——心跳依赖桥的发现文件，桥关了插件无处可发。
 */
export function composeIntegrationState(
  info: DshIntegrationInfo | null | undefined,
  bridgeRunning: boolean,
  lastHeartbeatAt: number | null,
  nowMs: number,
): IntegrationState {
  const s = info?.status;
  if (!s?.dsh_home_detected) return "no-dsh";
  if (!s.profile_ready) return "no-profile";
  if (!s.plugin_installed) return "not-installed";
  if (!s.plugin_activated) return "half-activated";
  const fresh =
    bridgeRunning &&
    lastHeartbeatAt !== null &&
    nowMs >= lastHeartbeatAt &&
    nowMs - lastHeartbeatAt < ONLINE_WINDOW_MS;
  return fresh ? "online" : "awaiting-restart";
}
