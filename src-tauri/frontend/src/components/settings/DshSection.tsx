import { Bot } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { useToast } from "@/components/ui/toast";
import { api, onDshBridgeStatus } from "@/lib/tauri";
import type { DshConfigInfo } from "@/types/tauri";

/**
 * 设置页「外部感知（dsh 桥）」区块。
 *
 * dsh = deepseek-harness：其 Cordis 插件把任务事件 POST 到本应用的
 * loopback 桥（端口见发现文件 ~/.zapmomo/runtime/dsh-bridge.json），
 * 桌宠以气泡+语音播报。本区块提供开关、运行状态与测试播报。
 */
export function DshSection() {
  const toast = useToast();
  const [info, setInfo] = useState<DshConfigInfo | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(() => {
    void api
      .getDshConfig()
      .then(setInfo)
      .catch((e) => toast.error(String(e)));
  }, [toast]);

  useEffect(reload, [reload]);

  // 运行状态实时同步（桥启动/停止事件；error 一并带上供展示）
  useEffect(() => {
    const unlisten = onDshBridgeStatus((s) => {
      setInfo((prev) =>
        prev ? { ...prev, running: s.running, actual_port: s.port, error: s.error } : prev,
      );
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const toggleEnabled = async (enabled: boolean) => {
    setBusy(true);
    try {
      await api.setDshEnabled({ enabled });
      setInfo((prev) => (prev ? { ...prev, enabled } : prev));
      toast.success(enabled ? "dsh 桥已开启" : "dsh 桥已关闭");
      reload();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const patchParams = async (params: {
    voice_enabled?: boolean;
    llm_enabled?: boolean;
    record_to_history?: boolean;
  }) => {
    setBusy(true);
    try {
      await api.setDshParams({ params });
      setInfo((prev) => (prev ? { ...prev, ...params } : prev));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!info) return null;

  return (
    <section className="space-y-3">
      <h2 className="flex items-center gap-2 text-base font-semibold">
        <Bot className="size-4" />
        外部感知 · dsh 桥
      </h2>
      <p className="text-sm text-text-muted">
        接收 deepseek-harness 插件推送的任务事件，桌宠以气泡+语音播报。
        {info.running && info.actual_port
          ? ` 桥运行中 · 端口 ${info.actual_port}。`
          : " 桥未启动。"}
      </p>
      {info.error && (
        <p className="text-sm text-destructive" data-testid="dsh-bridge-error">
          桥异常：{info.error}
        </p>
      )}
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-4 text-sm">
          <span>启用 dsh 桥</span>
          <Switch
            aria-label="启用 dsh 桥"
            checked={info.enabled}
            disabled={busy}
            onCheckedChange={(v) => void toggleEnabled(v)}
          />
        </div>
        <div className="flex items-center justify-between gap-4 text-sm">
          <span>事件语音播报（语音会话进行中自动静音）</span>
          <Switch
            aria-label="事件语音播报"
            checked={info.voice_enabled}
            disabled={busy || !info.enabled}
            onCheckedChange={(v) => void patchParams({ voice_enabled: v })}
          />
        </div>
        <div className="flex items-center justify-between gap-4 text-sm">
          <span>LLM 播报文案（未连接或生成中回退固定台词）</span>
          <Switch
            aria-label="LLM 播报文案"
            checked={info.llm_enabled}
            disabled={busy || !info.enabled}
            onCheckedChange={(v) => void patchParams({ llm_enabled: v })}
          />
        </div>
        <div className="flex items-center justify-between gap-4 text-sm">
          <span>写入对话记录</span>
          <Switch
            aria-label="写入对话记录"
            checked={info.record_to_history}
            disabled={busy || !info.enabled}
            onCheckedChange={(v) => void patchParams({ record_to_history: v })}
          />
        </div>
      </div>
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={!info.enabled}
          onClick={() =>
            void api
              .testDshAnnounce()
              .then(() => toast.success("已发送测试播报，看一眼桌宠～"))
              .catch((e) => toast.error(String(e)))
          }
        >
          测试播报
        </Button>
      </div>
    </section>
  );
}
