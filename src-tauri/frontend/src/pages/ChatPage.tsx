import { useEffect, useRef } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { VoiceStatusBadge } from "@/components/voice/VoiceStatusBadge";
import { useRuntime } from "@/providers/RuntimeContext";

/**
 * 对话记录页：持久化对话记录（用户一句 / 桌宠一句，各占一行）+ 实时字幕 + 流式回复，纯展示。
 *
 * 语音本身全在后端（`voice` 会话线程），页面只订阅 `voice-session-*` 事件做展示；
 * 记录由后端落盘（`~/.zapmomo/conversations.json`），跨应用重启保留。
 * 会话的启用/停用入口在「模型与能力」页「语音会话」开关。
 */

/** 把 ISO 时间戳格式化为本地时刻（HH:mm:ss）。 */
function formatTime(at: string): string {
  const d = new Date(at);
  if (Number.isNaN(d.getTime())) return at;
  return d.toLocaleTimeString("zh-CN", { hour12: false });
}

export function ChatPage() {
  const { voice, kws, asr } = useRuntime();
  const kwsEnabled = kws.config.config?.enabled ?? false;
  const asrEnabled = asr.config.config?.enabled ?? false;
  const capabilitiesReady = kwsEnabled && asrEnabled;

  // 记录 / 流式字幕更新时自动滚动到底部（新消息在底部）
  const scrollRef = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖值仅作「内容变化触发滚动」信号，不参与 effect 计算
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [voice.records, voice.partial, voice.pendingReply]);

  const hasContent =
    voice.records.length > 0 || Boolean(voice.partial) || Boolean(voice.pendingReply);

  return (
    <div className="flex h-full flex-col gap-4 overflow-hidden">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-xl font-semibold tracking-tight text-text-primary">对话记录</h1>
            <VoiceStatusBadge phase={voice.phase} running={voice.running} />
          </div>
          <p className="mt-0.5 text-sm text-muted-foreground">
            喊唤醒词开始对话，播报中喊唤醒词可打断
          </p>
        </div>
      </div>

      {!capabilitiesReady && (
        <Alert>
          <AlertTitle>语音互动未启用</AlertTitle>
          <AlertDescription>
            语音互动需要同时启用「唤醒词」(KWS)
            与「语音识别」(ASR)。请在「模型与能力」页开启后使用。
          </AlertDescription>
        </Alert>
      )}

      {voice.error && (
        <Alert variant="destructive">
          <AlertTitle>语音会话异常</AlertTitle>
          <AlertDescription>{voice.error}</AlertDescription>
        </Alert>
      )}

      <Card className="flex min-h-0 flex-1 flex-col">
        <CardHeader className="flex flex-row items-center justify-between space-y-0">
          <CardTitle>对话记录</CardTitle>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void voice.clearRecords()}
            disabled={voice.records.length === 0}
          >
            清空
          </Button>
        </CardHeader>
        <CardContent ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
          {!hasContent && (
            <p className="text-sm text-muted-foreground">
              {voice.running
                ? "待唤醒中，喊唤醒词开始对话…"
                : voice.enabled
                  ? "语音互动未在运行"
                  : "语音互动未开启：到「模型与能力」页打开「语音会话」开关后开始对话"}
            </p>
          )}

          {voice.records.length > 0 && (
            <div className="flex flex-col gap-3">
              {voice.records.map((rec) => {
                const isUser = rec.role === "user";
                return (
                  <div key={rec.at} className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
                    <div
                      className={`max-w-[75%] rounded-2xl px-3.5 py-2 text-sm shadow-none ${
                        isUser
                          ? "rounded-br-sm bg-blue-100 text-text-primary"
                          : "rounded-bl-sm bg-muted text-text-primary"
                      }`}
                    >
                      <p className="whitespace-pre-wrap break-words">{rec.text}</p>
                      <p className="mt-1 text-right text-[11px] text-text-muted">
                        {formatTime(rec.at)}
                      </p>
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {voice.partial && (
            <p className="text-right text-sm italic text-muted-foreground">{voice.partial}</p>
          )}

          {voice.pendingReply && (
            <div className="flex justify-start">
              <div className="max-w-[75%] rounded-2xl rounded-bl-sm bg-muted px-3.5 py-2 text-sm text-text-primary">
                <p className="whitespace-pre-wrap break-words">{voice.pendingReply}</p>
              </div>
            </div>
          )}

          {voice.currentSentence && !voice.replyDone && (
            <p className="mt-1 text-xs text-violet-600">正在播报：{voice.currentSentence}</p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
