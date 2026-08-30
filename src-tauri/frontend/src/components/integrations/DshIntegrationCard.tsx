import { open } from "@tauri-apps/plugin-dialog";
import { CircleCheck, Copy, Download, Puzzle, RotateCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { useToast } from "@/components/ui/toast";
import { composeIntegrationState, type IntegrationState } from "@/lib/dshIntegration";
import { api, onDshBridgeStatus, onDshInstallProgress } from "@/lib/tauri";
import type { DshConfigInfo, DshInstallProgress, DshIntegrationInfo } from "@/types/tauri";

/** 状态徽章文案（online 绿、半成品/等待橙、缺失中性——样式按 tone 区分）。 */
const STATE_META: Record<IntegrationState, { label: string; tone: "online" | "warn" | "muted" }> = {
  "no-dsh": { label: "未检测到 dsh", tone: "muted" },
  "no-profile": { label: "dsh 未初始化", tone: "warn" },
  "not-installed": { label: "插件未安装", tone: "warn" },
  "half-activated": { label: "已安装 · 未激活", tone: "warn" },
  "awaiting-restart": { label: "已激活 · 等待在线", tone: "warn" },
  online: { label: "在线", tone: "online" },
};

/** 心跳新鲜度重算间隔（心跳 15s 一跳，这里 5s 足够跟手）。 */
const NOW_TICK_MS = 5_000;

/**
 * 插件集成页「deepseek-harness」（dsh 桥）集成卡片。
 *
 * 状态机（lib/dshIntegration 纯函数合成）：no-dsh / no-profile / not-installed /
 * half-activated / awaiting-restart / online。检测全走文件级（~/.dsh 布局），
 * 在线判定走插件心跳（45s 窗口）。桥无独立开关：启停跟随插件安装状态——安装
 * 完成后端自动拉起、卸载完成后自动停止；「启用 dsh 桥」开关已随该语义移除。
 */
export function DshIntegrationCard() {
  const toast = useToast();
  const [info, setInfo] = useState<DshConfigInfo | null>(null);
  const [integration, setIntegration] = useState<DshIntegrationInfo | null>(null);
  const [heartbeatAt, setHeartbeatAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [install, setInstall] = useState<DshInstallProgress | null>(null);
  const [installOp, setInstallOp] = useState<"install" | "uninstall">("install");
  const [installing, setInstalling] = useState(false);

  const reload = useCallback(() => {
    void api
      .getDshConfig()
      .then(setInfo)
      .catch((e) => toast.error(String(e)));
    void api
      .detectDshIntegration()
      .then(setIntegration)
      .catch((e) => toast.error(String(e)));
    void api
      .getDshBridgeStatus()
      .then((s) => {
        setHeartbeatAt(s.last_heartbeat_at ?? null);
        setInfo((prev) => (prev ? { ...prev, running: s.running, actual_port: s.port } : prev));
      })
      .catch(() => {});
  }, [toast]);

  useEffect(reload, [reload]);

  // 桥状态实时同步（启动/停止/心跳事件；心跳时间戳直接驱动在线判定）
  useEffect(() => {
    const unlisten = onDshBridgeStatus((s) => {
      if (s.last_heartbeat_at !== null) setHeartbeatAt(s.last_heartbeat_at);
      setInfo((prev) =>
        prev ? { ...prev, running: s.running, actual_port: s.port, error: s.error } : prev,
      );
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 安装进度流
  useEffect(() => {
    const unlisten = onDshInstallProgress((p) => {
      setInstall(p);
      if (p.state === "done") {
        setInstalling(false);
        reload();
      } else if (p.state === "failed") {
        setInstalling(false);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload]);

  // 心跳无事件可等（dsh 退出不再发），靠本地时钟翻转在线判定
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), NOW_TICK_MS);
    return () => clearInterval(t);
  }, []);

  const patchParams = async (params: {
    voice_enabled?: boolean;
    llm_enabled?: boolean;
    record_to_history?: boolean;
  }) => {
    try {
      await api.setDshParams({ params });
      setInfo((prev) => (prev ? { ...prev, ...params } : prev));
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleInstall = async (path?: string) => {
    setInstallOp("install");
    setInstalling(true);
    setInstall({ state: "discovering", message: "准备安装…" });
    try {
      await api.installDshPlugin({ path: path ?? null });
    } catch (e) {
      toast.error(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const handleUninstall = async () => {
    setInstallOp("uninstall");
    setInstalling(true);
    setInstall({ state: "discovering", message: "准备卸载…" });
    try {
      await api.uninstallDshPlugin();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const pickExecutable = async () => {
    const file = await open({
      multiple: false,
      title: "选择 dsh 可执行文件",
    });
    if (typeof file === "string" && file) {
      void handleInstall(file);
    }
  };

  const copyManualCommand = async () => {
    const cmd = integration?.manual_command;
    if (!cmd) return;
    await navigator.clipboard.writeText(cmd);
    toast.success("已复制手动安装命令");
  };

  if (!info) return null;

  const state = composeIntegrationState(integration, info.running, heartbeatAt, now);
  const meta = STATE_META[state];
  const busy = installing;
  // 插件已激活（进入 dsh bundles）：桥随此后台常驻，行为开关可用
  const activated = state === "awaiting-restart" || state === "online";
  const pluginInstalled = state === "half-activated" || activated;

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="flex items-center justify-between gap-3 px-3.5 pt-3">
        <div className="flex items-center gap-2.5">
          <Puzzle className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">deepseek-harness</h2>
            <p className="mt-0.5 text-xs text-text-muted">
              dsh 任务事件实时联动桌宠：气泡 + 语音播报
            </p>
          </div>
        </div>
        <Badge
          data-testid="dsh-integration-state"
          variant={meta.tone === "online" ? "default" : "secondary"}
          className={
            meta.tone === "warn" ? "bg-amber-500/15 text-amber-600 dark:text-amber-400" : undefined
          }
        >
          {meta.label}
        </Badge>
      </div>

      <dl className="mt-2 divide-y divide-divider">
        {/* 状态引导区：按状态机给出下一步动作 */}
        <div className="space-y-2 px-3.5 py-2.5">
          {state === "no-dsh" && (
            <p className="text-sm text-text-muted" data-testid="dsh-no-dsh-hint">
              未检测到 dsh（deepseek-harness）环境。安装并运行一次{" "}
              <code className="rounded bg-panel-border px-1">dsh web</code> 后重开本页即可识别。
            </p>
          )}
          {state === "no-profile" && (
            <p className="text-sm text-text-muted">
              检测到 dsh，但 web profile 尚未初始化——请先在终端运行一次{" "}
              <code className="rounded bg-panel-border px-1">dsh web</code>。
            </p>
          )}
          {state === "not-installed" && (
            <div className="flex flex-wrap items-center justify-between gap-2">
              <p className="text-sm text-text-muted">
                检测到 dsh 环境，安装本插件后桌宠即可联动 dsh 任务事件。
              </p>
              <div className="flex shrink-0 items-center gap-1.5">
                <Button size="sm" disabled={busy} onClick={() => void handleInstall()}>
                  <Download className="h-3.5 w-3.5" />
                  一键安装
                </Button>
                <Button size="sm" variant="ghost" onClick={() => void copyManualCommand()}>
                  <Copy className="h-3.5 w-3.5" />
                  复制命令
                </Button>
              </div>
            </div>
          )}
          {state === "half-activated" && (
            <div className="flex flex-wrap items-center justify-between gap-2">
              <p className="text-sm text-amber-600 dark:text-amber-400">
                插件已安装但未进入 dsh 加载列表（bundles）。重跑一次安装命令即可修复：
              </p>
              <Button size="sm" variant="outline" onClick={() => void copyManualCommand()}>
                <Copy className="h-3.5 w-3.5" />
                复制修复命令
              </Button>
            </div>
          )}
          {state === "awaiting-restart" && (
            <p className="text-sm text-text-muted">
              插件已就绪，启动 dsh web 即自动上线（首次安装需重启 dsh web 生效）。
            </p>
          )}
          {state === "online" && (
            <p
              className="flex items-center gap-1.5 text-sm text-text-muted"
              data-testid="dsh-online-hint"
            >
              <CircleCheck className="h-4 w-4 text-emerald-500" />
              联动生效中
              {info.actual_port ? ` · 桥端口 ${info.actual_port}` : ""}
              。在 dsh 里发起任务，桌宠会实时播报。
            </p>
          )}
          {info.error && (
            <p className="text-sm text-destructive" data-testid="dsh-bridge-error">
              桥异常：{info.error}
            </p>
          )}
          {install && (
            <div className="space-y-1" data-testid="dsh-install-progress">
              <p
                className={
                  install.state === "failed"
                    ? "text-sm text-destructive"
                    : "text-sm text-text-muted"
                }
              >
                {/* 失败兜底文案与重试按钮仅安装态需要；卸载失败直接展示后端消息 */}
                {install.state === "failed" && installOp === "install"
                  ? "安装失败。可手动选择 dsh 可执行文件重试，或复制命令在终端执行。"
                  : install.message}
              </p>
              {install.state === "failed" && installOp === "install" && (
                <div className="flex flex-wrap items-center gap-1.5">
                  <Button size="sm" variant="outline" onClick={() => void pickExecutable()}>
                    <RotateCw className="h-3.5 w-3.5" />
                    选择 dsh 可执行文件
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => void copyManualCommand()}>
                    <Copy className="h-3.5 w-3.5" />
                    复制手动命令
                  </Button>
                </div>
              )}
            </div>
          )}
        </div>

        {/* 行为开关组：仅在插件已激活后有意义（桥启停本身跟随安装状态，无独立开关） */}
        <div className="px-3.5 py-2.5">
          <div className="flex items-center justify-between gap-4 py-1.5 text-sm">
            <span>事件语音播报（语音会话进行中自动静音）</span>
            <Switch
              aria-label="事件语音播报"
              checked={info.voice_enabled}
              disabled={busy || !activated}
              onCheckedChange={(v) => void patchParams({ voice_enabled: v })}
            />
          </div>
          <div className="flex items-center justify-between gap-4 py-1.5 text-sm">
            <span>LLM 播报文案（未连接或生成中回退固定台词）</span>
            <Switch
              aria-label="LLM 播报文案"
              checked={info.llm_enabled}
              disabled={busy || !activated}
              onCheckedChange={(v) => void patchParams({ llm_enabled: v })}
            />
          </div>
          <div className="flex items-center justify-between gap-4 py-1.5 text-sm">
            <span>写入对话记录</span>
            <Switch
              aria-label="写入对话记录"
              checked={info.record_to_history}
              disabled={busy || !activated}
              onCheckedChange={(v) => void patchParams({ record_to_history: v })}
            />
          </div>
          <div className="flex items-center gap-2 pt-2">
            <Button
              size="sm"
              variant="outline"
              disabled={!activated}
              onClick={() =>
                void api
                  .testDshAnnounce()
                  .then(() => toast.success("已发送测试播报，看一眼桌宠～"))
                  .catch((e) => toast.error(String(e)))
              }
            >
              测试播报
            </Button>
            {pluginInstalled && (
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => void handleUninstall()}
              >
                <Trash2 className="h-3.5 w-3.5" />
                卸载插件
              </Button>
            )}
          </div>
        </div>
      </dl>
    </section>
  );
}
