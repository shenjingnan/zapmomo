import { createContext, useContext } from "react";
import type { AsrConfigState } from "@/hooks/useAsrConfig";
import type { AsrDictateResultsState } from "@/hooks/useAsrDictateResults";
import type { AsrDictateState } from "@/hooks/useAsrDictate";
import type { AsrListeningState } from "@/hooks/useAsrListening";
import type { AsrModelDownloadState } from "@/hooks/useAsrModelDownload";
import type { AsrResultsState } from "@/hooks/useAsrResults";
import type { DevicesState } from "@/hooks/useDevices";
import type { KwsConfigState } from "@/hooks/useKwsConfig";
import type { ListeningState } from "@/hooks/useListening";
import type { LlmState } from "@/hooks/useLlm";
import type { ModelDownloadState } from "@/hooks/useModelDownload";
import type { DetectionEntry } from "@/hooks/useResults";
import type { TtsState } from "@/hooks/useTts";
import type { VoiceSessionState } from "@/hooks/useVoiceSession";
import type { AppInfo } from "@/types/tauri";

/** 全局运行态：由 `AppRuntimeProvider` 集中提供，页面/卡片通过 `useRuntime()` 读取。 */
export interface RuntimeState {
  appInfo: AppInfo | null;
  devices: DevicesState;
  kws: {
    config: KwsConfigState;
    download: ModelDownloadState;
    listening: ListeningState;
    results: DetectionEntry[];
  };
  asr: {
    config: AsrConfigState;
    download: AsrModelDownloadState;
    listening: AsrListeningState;
    /** 离线听写（SenseVoice/Whisper + VAD 分段）运行状态 */
    dictate: AsrDictateState;
    /** 离线听写结果段（独立于流式 results） */
    dictateResults: AsrDictateResultsState;
    results: AsrResultsState;
  };
  llm: LlmState;
  tts: TtsState;
  voice: VoiceSessionState;
  /** 全局选中的麦克风设备（KWS 与 ASR 共用） */
  device: string;
  setDevice: (device: string) => void;
  /** KWS 自定义唤醒词（路由外层持有；修改时持久化到 backend，重启后仍存在） */
  sessionKeywords: string;
  setSessionKeywords: (keywords: string) => void;
  /** 任一监听/识别进行中（用于禁用设备切换等） */
  anyListening: boolean;
}

export const RuntimeContext = createContext<RuntimeState | null>(null);

/** 读取全局运行态；必须在 `AppRuntimeProvider` 内使用。 */
export function useRuntime(): RuntimeState {
  const ctx = useContext(RuntimeContext);
  if (!ctx) {
    throw new Error("useRuntime 必须在 AppRuntimeProvider 内使用");
  }
  return ctx;
}
