import { useCallback, useEffect, useState } from "react";
import { api } from "@/lib/tauri";
import type { SpeakerConfigInfo, SpeakerParamsPatch } from "@/types/tauri";

export interface SpeakerConfigState {
  config: SpeakerConfigInfo | null;
  error: string | null;
  refresh: () => Promise<void>;
  /** 持久化「启用声纹识别」偏好（[speaker].enabled），写成功后回读配置。 */
  setEnabled: (enabled: boolean) => Promise<void>;
  /** 持久化声纹参数（[speaker] 批写入），写成功后回读配置。 */
  setParams: (patch: SpeakerParamsPatch) => Promise<void>;
}

/** 读取声纹识别配置与模型状态。 */
export function useSpeakerConfig(): SpeakerConfigState {
  const [config, setConfig] = useState<SpeakerConfigInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setConfig(await api.getSpeakerConfig());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const setEnabled = useCallback(
    async (enabled: boolean) => {
      try {
        await api.setSpeakerEnabled({ enabled });
        await refresh();
      } catch (e) {
        setError(String(e));
      }
    },
    [refresh],
  );

  const setParams = useCallback(
    async (patch: SpeakerParamsPatch) => {
      // 保存失败向上抛出，由调用方（参数表单）展示内联错误
      await api.setSpeakerParams({ params: patch });
      await refresh();
    },
    [refresh],
  );

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { config, error, refresh, setEnabled, setParams };
}
