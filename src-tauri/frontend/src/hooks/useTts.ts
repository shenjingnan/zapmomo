import { type RefObject, useCallback, useEffect, useRef, useState } from "react";
import { api, onTtsProgress, onTtsResult, onTtsStopped, toAssetUrl } from "@/lib/tauri";
import type {
  SaveTtsVoiceRequest,
  TtsConfigInfo,
  TtsParamsPatch,
  TtsResult,
  TtsVoice,
} from "@/types/tauri";

export interface TtsState {
  config: TtsConfigInfo | null;
  configError: string | null;
  refreshConfig: () => Promise<void>;
  setEnabled: (enabled: boolean) => Promise<void>;
  /** 批量保存合成参数（扩散步数/默认语速/线程/调试）；失败向上抛出，由调用方展示内联错误。 */
  setParams: (patch: TtsParamsPatch) => Promise<void>;
  /** 音色列表（模型包内置 + 用户自定义音色库）。 */
  voices: TtsVoice[];
  /** 当前音色 id（持久化到 `[tts].voice`，即全局默认音色；空 = 内置 leijun）。 */
  selectedVoice: string;
  /** 设定音色并持久化为全局默认音色（写 `[tts].voice`）；失败经 `error` 暴露。 */
  setSelectedVoice: (id: string) => Promise<void>;
  /** 保存一个自定义音色到音色库；成功后刷新 voices。 */
  saveVoice: (req: SaveTtsVoiceRequest) => Promise<TtsVoice>;
  /** 删除一个自定义音色；成功后刷新 voices。 */
  deleteVoice: (id: string) => Promise<void>;
  /** 录制 N 秒麦克风为 wav，返回路径（供保存为音色）。 */
  recordVoice: (seconds: number, device?: string | null) => Promise<string>;
  synthesizing: boolean;
  progress: number | null;
  result: TtsResult | null;
  error: string | null;
  /** 发起一次合成；`opts.speed` 为真实 synthesize_tts 一次性参数（缺省走后端配置默认）。 */
  synthesize: (text: string, opts?: { speed?: number | null }) => Promise<void>;
  stop: () => Promise<void>;
  audioUrl: string | null;
  audioRef: RefObject<HTMLAudioElement | null>;
  play: () => void;
}

/**
 * TTS 状态管理：配置读取、音色列表、合成触发/进度/结果、播放、自定义音色库。
 * 合成走后台线程，进度与结果经 `tts-progress` / `tts-result` / `tts-stopped` 事件同步。
 * 模型下载/切换统一走「选择模型」弹窗（`list_model_library`），此处不持有 legacy 下载通道。
 */
export function useTts(): TtsState {
  const [config, setConfig] = useState<TtsConfigInfo | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [voices, setVoices] = useState<TtsVoice[]>([]);
  const [selectedVoice, setSelectedVoiceLocal] = useState("");
  const [synthesizing, setSynthesizing] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [result, setResult] = useState<TtsResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);

  const refreshConfig = useCallback(async () => {
    try {
      const cfg = await api.getTtsConfig();
      setConfig(cfg);
      setConfigError(null);
      // 载入持久化的默认音色（[tts].voice；未设置则空 = 内置 leijun）
      setSelectedVoiceLocal(cfg.voice ?? "");
    } catch (e) {
      setConfigError(String(e));
    }
  }, []);

  useEffect(() => {
    void refreshConfig();
  }, [refreshConfig]);

  // 持久化「是否启用语音合成」，写入 [tts].enabled 后刷新配置。
  const setEnabled = useCallback(
    async (enabled: boolean) => {
      try {
        await api.setTtsEnabled({ enabled });
        await refreshConfig();
      } catch (e) {
        setError(String(e));
      }
    },
    [refreshConfig],
  );

  // 加载音色列表（模型包内置 + 用户自定义音色库）。
  const refreshVoices = useCallback(async () => {
    try {
      setVoices(await api.listTtsVoices());
    } catch {
      setVoices([]);
    }
  }, []);

  useEffect(() => {
    void refreshVoices();
  }, [refreshVoices]);

  useEffect(() => {
    const unsubs = [
      onTtsProgress((p) => setProgress(p.percent)),
      onTtsResult((r) => setResult(r)),
      onTtsStopped((payload) => {
        setSynthesizing(false);
        if (payload.error) setError(payload.error);
      }),
    ];
    return () => {
      unsubs.forEach((u) => {
        u.then((fn) => fn());
      });
    };
  }, []);

  // 设定音色并持久化为全局默认音色（[tts].voice），所有合成（测试/语音会话）生效。
  const setSelectedVoice = useCallback(async (id: string) => {
    setSelectedVoiceLocal(id);
    try {
      await api.setTtsVoice(id || null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const synthesize = useCallback(
    async (text: string, opts?: { speed?: number | null }) => {
      const trimmed = text.trim();
      if (!trimmed) return;
      setError(null);
      setResult(null);
      setProgress(null);
      setSynthesizing(true);
      // 收录模型均为参考音频克隆族：自定义音色 → 直接传其 wav+转写；
      // 否则传音色 id（内置/默认音色，缺省走 auto voice/内置 leijun）
      const savedVoice = voices.find((v) => v.custom && v.id === selectedVoice);
      try {
        await api.synthesizeTts({
          text: trimmed,
          speed: opts?.speed ?? null,
          sid: null,
          voice: savedVoice ? null : selectedVoice || null,
          referenceWav: savedVoice ? savedVoice.wav_path : null,
          referenceText: savedVoice ? savedVoice.reference_text : null,
        });
      } catch (e) {
        setError(String(e));
        setSynthesizing(false);
      }
    },
    [voices, selectedVoice],
  );

  // 批量保存合成参数（扩散步数/默认语速/线程/调试），写入 [tts] 后刷新配置。
  // 不 catch：保存失败向上抛出，由调用方（高级参数表单）展示内联错误。
  const setParams = useCallback(
    async (patch: TtsParamsPatch) => {
      await api.setTtsParams({ params: patch });
      await refreshConfig();
    },
    [refreshConfig],
  );

  // 音色库：保存/删除/录音，成功后刷新 voices。
  const saveVoice = useCallback(
    async (req: SaveTtsVoiceRequest) => {
      const v = await api.saveTtsVoice(req);
      await refreshVoices();
      return v;
    },
    [refreshVoices],
  );

  const deleteVoice = useCallback(
    async (id: string) => {
      await api.deleteTtsVoice({ id });
      await refreshVoices();
    },
    [refreshVoices],
  );

  const recordVoice = useCallback(async (seconds: number, device?: string | null) => {
    return api.recordTtsVoice({ seconds, device: device ?? null });
  }, []);

  const stop = useCallback(async () => {
    try {
      await api.stopTts();
      setSynthesizing(false);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const play = useCallback(() => {
    const el = audioRef.current;
    if (!el) return;
    void el.play().catch(() => {});
  }, []);

  const audioUrl = result ? toAssetUrl(result.path) : null;

  return {
    config,
    configError,
    refreshConfig,
    setEnabled,
    setParams,
    voices,
    selectedVoice,
    setSelectedVoice,
    saveVoice,
    deleteVoice,
    recordVoice,
    synthesizing,
    progress,
    result,
    error,
    synthesize,
    stop,
    audioUrl,
    audioRef,
    play,
  };
}
