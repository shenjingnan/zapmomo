import { useCallback, useEffect, useState } from "react";
import { useToast } from "@/components/ui/toast";
import { api, onLlmError, onLlmFinished, onLlmStatus, onLlmToken } from "@/lib/tauri";
import type { LlmConfigInfo, LlmParamsPatch } from "@/types/tauri";

export interface LlmState {
  config: LlmConfigInfo | null;
  configError: string | null;
  refreshConfig: () => Promise<void>;
  ready: boolean;
  loading: boolean;
  generating: boolean;
  response: string;
  error: string | null;
  load: () => Promise<void>;
  unload: () => Promise<void>;
  chat: (text: string) => Promise<void>;
  stop: () => Promise<void>;
  /** 保存远程连接配置（base_url/api_key/model）；失败时 rethrow 供表单内联展示错误 */
  setConnection: (baseUrl: string, apiKey: string | null, model: string) => Promise<void>;
  /** 批量保存采样参数；失败时 rethrow 供保存按钮内联展示错误 */
  setParams: (params: LlmParamsPatch) => Promise<void>;
  /** 保存系统提示词；失败时 rethrow */
  setSystemPrompt: (prompt: string) => Promise<void>;
}

/**
 * LLM 状态管理：配置读取、模型加载/卸载、流式对话。
 * 加载结果经 `llm-status`/`llm-error` 事件同步，token 流经 `llm-token`，
 * 生成结束经 `llm-finished`。
 */
export function useLlm(): LlmState {
  const [config, setConfig] = useState<LlmConfigInfo | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [loading, setLoading] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [response, setResponse] = useState("");
  const [error, setError] = useState<string | null>(null);

  const toast = useToast();

  /** 命令失败：写入 error 状态（UI 红字「错误」）并通过右上角通知透出真实原因。 */
  const fail = useCallback(
    (e: unknown) => {
      const msg = String(e);
      setError(msg);
      toast.error(msg);
    },
    [toast],
  );

  const refreshConfig = useCallback(async () => {
    try {
      const c = await api.getLlmConfig();
      setConfig(c);
      setReady(c.ready);
      setConfigError(null);
    } catch (e) {
      setConfigError(String(e));
    }
  }, []);

  useEffect(() => {
    void refreshConfig();
  }, [refreshConfig]);

  useEffect(() => {
    const unsubs = [
      onLlmToken((delta) => setResponse((prev) => prev + delta.text)),
      onLlmFinished(() => setGenerating(false)),
      onLlmError((e) => {
        setError(e);
        setLoading(false);
        setGenerating(false);
      }),
      onLlmStatus((s) => {
        setReady(s.ready);
        setLoading(false);
      }),
    ];
    return () => {
      unsubs.forEach((u) => {
        u.then((fn) => fn());
      });
    };
  }, []);

  const load = useCallback(async () => {
    setError(null);
    setLoading(true);
    try {
      await api.loadLlmModel();
      // 加载结果经 llm-status / llm-error 事件更新
    } catch (e) {
      fail(e);
      setLoading(false);
    }
  }, [fail]);

  const unload = useCallback(async () => {
    try {
      await api.unloadLlmModel();
      setReady(false);
      setResponse("");
    } catch (e) {
      fail(e);
    }
  }, [fail]);

  const chat = useCallback(
    async (text: string) => {
      const trimmed = text.trim();
      if (!trimmed) return;
      setError(null);
      setResponse("");
      setGenerating(true);
      try {
        await api.chatLlm({ text: trimmed });
        // token 流经 llm-token 事件，结束经 llm-finished
      } catch (e) {
        fail(e);
        setGenerating(false);
      }
    },
    [fail],
  );

  const stop = useCallback(async () => {
    try {
      await api.stopLlm();
    } catch (e) {
      fail(e);
    }
  }, [fail]);

  const setConnection = useCallback(
    async (baseUrl: string, apiKey: string | null, model: string) => {
      try {
        await api.setLlmConnection({ baseUrl, apiKey, model });
        await refreshConfig();
      } catch (e) {
        setError(String(e));
        throw e; // 表单需要内联错误反馈
      }
    },
    [refreshConfig],
  );

  const setParams = useCallback(
    async (params: LlmParamsPatch) => {
      try {
        await api.setLlmParams({ params });
        await refreshConfig();
      } catch (e) {
        setError(String(e));
        throw e; // 保存按钮需要内联错误反馈
      }
    },
    [refreshConfig],
  );

  const setSystemPrompt = useCallback(
    async (prompt: string) => {
      try {
        await api.setLlmSystemPrompt({ prompt });
        await refreshConfig();
      } catch (e) {
        setError(String(e));
        throw e;
      }
    },
    [refreshConfig],
  );

  return {
    config,
    configError,
    refreshConfig,
    ready,
    loading,
    generating,
    response,
    error,
    load,
    unload,
    chat,
    stop,
    setConnection,
    setParams,
    setSystemPrompt,
  };
}
