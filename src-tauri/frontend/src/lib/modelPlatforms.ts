/**
 * 预设平台过滤：以后端 `list_model_library` 返回为**可见性唯一事实源**。
 *
 * 为什么不用 `isMacOs()/isWindows()` 判定可见性：UA 在 Apple Silicon 上同样报
 * "Intel Mac OS X"，无法区分 macOS 架构；platforms 的唯一权威是后端
 * `models_for_current_platform()`（registry 的 platforms 字段）。后端列表里
 * 出现的 id = 当前平台可见；平台受限的预设只在该 id 出现于后端列表时渲染。
 */

/** 与 registry `platforms` 字段取值对齐的三元组简写。 */
export type PlatformId = "darwin-aarch64" | "darwin-x86_64" | "linux-x86_64" | "windows-x86_64";

/** 带平台约束的预设条目（`platforms` 缺省 = 全平台可见）。 */
export interface PresetWithPlatforms {
  id: string;
  platforms?: readonly PlatformId[];
}

/**
 * 过滤出当前平台可见的预设。
 *
 * `backend` 为 null（列表尚未加载）时返回空数组——避免闪现本平台不可用的
 * 预设（曾导致「能下载但不能切换」：预设硬编码全量展示，后端列表才是门控）。
 * 平台受限模型的可见性随 registry 解锁自动放开，前端无需二次硬编码。
 */
export function visiblePresets<P extends PresetWithPlatforms>(
  presets: readonly P[],
  backend: { id: string }[] | null,
): P[] {
  if (!backend) return [];
  const ids = new Set(backend.map((m) => m.id));
  return presets.filter((p) => !p.platforms || ids.has(p.id));
}
