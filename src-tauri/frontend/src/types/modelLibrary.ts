/** 模型库（Model Library）类型定义，与 Rust `model_library` 的 camelCase 序列化一一对应。 */

export type ModelType = "kws" | "asr" | "llm" | "tts";
export type ModelSource = "registry" | "local";
export type StorageOwnership = "managed" | "external";
export type InstallState = "not_installed" | "downloading" | "installed" | "invalid";
export type RuntimeStatus = "inactive" | "active" | "switching" | "pending_restart" | "load_failed";
export type RuntimeAction =
  | "none"
  | "reloaded"
  | "restart_required"
  | "reload_failed_rolled_back"
  | "reload_failed_rollback_failed";

export interface LibraryModel {
  id: string;
  name: string;
  displayName: string;
  modelType: ModelType;
  runtime: string;
  format: string;
  description: string;
  languages: string[];
  tags: string[];
  parameterCount: string | null;
  quantization: string | null;
  version: string;
  sizeBytes: number | null;
  homepage: string | null;
  /** 是否有内置下载源（false = LLM 需导入 GGUF） */
  downloadable: boolean;
  source: ModelSource;
  ownership: StorageOwnership;
  installState: InstallState;
  /** 是否为该能力当前选择的模型（RuntimeSelection） */
  current: boolean;
  /** 运行状态（仅 current 模型有意义） */
  runtimeStatus: RuntimeStatus;
  localPath: string | null;
  installedAt: string | null;
  /** 稳定安装身份（set_current_model / delete_model 按此定位具体 Artifact） */
  installId: string | null;
  /** HF repo_id（若可映射） */
  repoId: string | null;
  /** 兼容性级别（verified/compatible/possible/unsupported） */
  compatibility: string | null;
}

export interface SystemResources {
  totalMemory: number;
  availableMemory: number;
  diskTotal: number;
  diskAvailable: number;
  cpuUsage: number;
}

/** 存储信息（`get_storage_info`），与 Rust `StorageInfoView` camelCase 对应。 */
export interface StorageInfo {
  /** 已解析的 data_dir（null = 使用默认 ~/.zapmomo） */
  dataDir: string | null;
  modelsDir: string;
  companionsDir: string;
  legacyModelsDir: string | null;
  legacyCompanionsDir: string | null;
  legacyModelsBytes: number;
  legacyCompanionsBytes: number;
  migrationAvailable: boolean;
  migrating: boolean;
  sameVolume: boolean;
  diskTotal: number;
  diskAvailable: number;
}

export interface MigrateFailedItem {
  name: string;
  reason: string;
}

/** 首次下载/导入前的存储位置引导信息（`get_storage_prompt`）。 */
export interface StoragePrompt {
  /** 是否建议弹引导（data_dir 未设置 && 无已装模型 && 用户未确认过） */
  promptRecommended: boolean;
  /** 默认数据根展示值（~/.zapmomo 展开后的绝对路径） */
  defaultDir: string;
  modelsDir: string;
  companionsDir: string;
  /** 建议目录（非默认卷中剩余空间最大的固定盘；单盘机器 = null） */
  suggestedDir: string | null;
  /** 建议卷可用字节 */
  suggestedAvailable: number | null;
  /** 默认卷可用字节 */
  defaultAvailable: number;
}

/** 迁移进度（`storage-migrate-progress`）。 */
export interface StorageMigrateProgress {
  state: "scanning" | "moving" | "finishing" | "done" | "cancelled" | "failed";
  currentItem: string | null;
  itemsDone: number;
  itemsTotal: number;
  bytesDone: number;
  bytesTotal: number;
  message: string;
  failedItems: MigrateFailedItem[];
}

export interface SetCurrentResult {
  modelType: ModelType;
  modelId: string;
  path: string;
  runtimeAction: RuntimeAction;
  effectiveImmediately: boolean;
  message: string;
}

export type LibraryProgressStage =
  | "preparing"
  | "downloading"
  | "verifying"
  | "extracting"
  | "done"
  | "cancelled"
  | "failed";

export interface ModelLibraryProgress {
  modelId: string;
  stage: LibraryProgressStage;
  asset: string;
  overallPercent: number;
  bytesDownloaded: number;
  totalBytes: number;
  message: string;
}
