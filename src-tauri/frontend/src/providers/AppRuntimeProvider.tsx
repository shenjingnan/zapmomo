import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { useToast } from "@/components/ui/toast";
import { useAppInfo } from "@/hooks/useAppInfo";
import { useAsrConfig } from "@/hooks/useAsrConfig";
import { useAsrDictate } from "@/hooks/useAsrDictate";
import { useAsrDictateResults } from "@/hooks/useAsrDictateResults";
import { useAsrListening } from "@/hooks/useAsrListening";
import { useAsrModelDownload } from "@/hooks/useAsrModelDownload";
import { useAsrResults } from "@/hooks/useAsrResults";
import { useDevices } from "@/hooks/useDevices";
import { useKwsConfig } from "@/hooks/useKwsConfig";
import { useListening } from "@/hooks/useListening";
import { useLlm } from "@/hooks/useLlm";
import { useModelDownload } from "@/hooks/useModelDownload";
import { useResults } from "@/hooks/useResults";
import { useTts } from "@/hooks/useTts";
import { useVoiceSession } from "@/hooks/useVoiceSession";
import { api } from "@/lib/tauri";
import { RuntimeContext, type RuntimeState } from "./RuntimeContext";

/**
 * 运行态 Provider：把 KWS / ASR / LLM / TTS / Live2D 的 hooks 集中在此调用，
 * 并常驻于路由外层（`<Routes>` 之外），使监听/下载/流式/加载状态不随页面切换丢失。
 * Router 只负责「当前显示哪个 UI」，不决定 runtime 生命周期。
 */
export function AppRuntimeProvider({ children }: { children: ReactNode }) {
  const toast = useToast();
  const appInfo = useAppInfo();
  const devices = useDevices();
  const kwsConfig = useKwsConfig();
  const kwsDownload = useModelDownload(kwsConfig.refresh);
  const listening = useListening();
  const results = useResults();
  const asrConfig = useAsrConfig();
  const asrDownload = useAsrModelDownload(asrConfig.refresh);
  const asrListening = useAsrListening();
  const asrResults = useAsrResults();
  const asrDictate = useAsrDictate();
  const asrDictateResults = useAsrDictateResults();
  const llm = useLlm();
  const tts = useTts();
  const voice = useVoiceSession();
  // 麦克风选择：跨页面全局共享（KWS/ASR/概览均消费），持久化到 backend settings.toml（顶层 microphone）。
  // 启动时回读后端；旧版本遗留的 localStorage 记忆做一次性迁移（读后即清，仅在读成功后才清理）。
  const [device, setDeviceState] = useState("");
  useEffect(() => {
    let cancelled = false;
    (async () => {
      let saved: string;
      try {
        saved = await api.getMicrophone();
      } catch {
        return; // 读取失败：保持默认，不迁移不清理
      }
      if (cancelled) return;
      if (saved) {
        setDeviceState(saved);
      } else {
        const legacy = localStorage.getItem("zapmomo.microphone");
        if (legacy) {
          setDeviceState(legacy);
          void api.setMicrophone({ mic: legacy }).catch(() => {});
        }
      }
      localStorage.removeItem("zapmomo.microphone");
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const setDevice = useCallback(
    (d: string) => {
      setDeviceState(d);
      // 监听中切换会触发后端用新设备重启监听；失败（如新设备不可用）时提示原因。
      void api.setMicrophone({ mic: d }).catch((e) => toast.error(String(e)));
    },
    [toast],
  );

  // 设备列表就绪后校验记忆的设备是否仍存在（如外设拔出），否则清空避免 start 时按不存在设备报错
  useEffect(() => {
    if (device && devices.devices.length > 0 && !devices.devices.includes(device)) {
      setDevice("");
    }
  }, [device, devices.devices, setDevice]);

  // 会话级自定义唤醒词：随 runtime 常驻路由外，页面切走再回来不丢；
  // 修改时防抖持久化到 backend（[kws].custom_keywords），启动时回读一次（下次打开仍存在，自动监听也用）。
  const [sessionKeywords, setSessionKeywordsState] = useState("");
  const keywordsTimer = useRef<number | null>(null);
  const keywordsHydrated = useRef(false);
  const setSessionKeywords = useCallback((kw: string) => {
    setSessionKeywordsState(kw);
    if (keywordsTimer.current) window.clearTimeout(keywordsTimer.current);
    keywordsTimer.current = window.setTimeout(() => {
      void api.setKwsCustomKeywords({ keywords: kw }).catch(() => {});
      keywordsTimer.current = null;
    }, 300);
  }, []);

  // 配置加载后仅回填一次持久化的自定义唤醒词（避免后续配置刷新覆盖用户正在输入的内容）
  useEffect(() => {
    if (keywordsHydrated.current) return;
    const persisted = kwsConfig.config?.custom_keywords;
    if (persisted != null) {
      setSessionKeywordsState(persisted);
      keywordsHydrated.current = true;
    }
  }, [kwsConfig.config?.custom_keywords]);

  const anyListening =
    listening.isListening || asrListening.isListening || asrDictate.isDictating || voice.running;

  const value: RuntimeState = {
    appInfo,
    devices,
    kws: { config: kwsConfig, download: kwsDownload, listening, results },
    asr: {
      config: asrConfig,
      download: asrDownload,
      listening: asrListening,
      dictate: asrDictate,
      dictateResults: asrDictateResults,
      results: asrResults,
    },
    llm,
    tts,
    voice,
    device,
    setDevice,
    sessionKeywords,
    setSessionKeywords,
    anyListening,
  };

  return <RuntimeContext.Provider value={value}>{children}</RuntimeContext.Provider>;
}
