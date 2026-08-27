import { CircleAlert, Play, SlidersVertical, Square, Volume2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import { modelNameFromDir, isCloneRequiredTtsKind, isCloneTtsKind } from "./ttsMeta";

/** 退出动画时长，需与卡片/遮罩的 duration 一致。 */
const EXIT_MS = 200;

interface TtsTestDialogProps {
  open: boolean;
  onClose: () => void;
  /** 打开「管理音色」对话框（两入口共用页面级实例）。 */
  onManageVoices?: () => void;
  /** 管理音色对话框是否打开（用于 Esc 门控：叠层时只关最上层）。 */
  manageOpen?: boolean;
}

/**
 * 测试语音合成对话框：当前模型 / 音色（默认 + 内置 + 已保存自定义）/ 语速 / 测试文本 /
 * 合成进度与结果。复用全局 `useRuntime().tts` 单实例 runtime，不创建第二套。
 *
 * 生命周期与 KWS/ASR TestDialog 保持一致：
 * - 「合成并播放」记录 `startedByDialog` 归属，合成完成自动播放；
 * - 关闭时仅停止「本对话框发起」且仍在合成的任务（fire-and-forget stop_tts，
 *   绝不暗示能即时中断推理）；正在播放的 `<audio>` 显式 pause + 清空 src，
 *   关闭后不得继续播放；绝不卸载模型、绝不误停非 Dialog 发起的全局任务。
 */
export function TtsTestDialog({ open, onClose, onManageVoices, manageOpen }: TtsTestDialogProps) {
  const { tts } = useRuntime();
  const [mounted, setMounted] = useState(open);
  const [closing, setClosing] = useState(false);
  const [text, setText] = useState("你好，我是 ZapMomo。");
  const [speed, setSpeed] = useState(1);
  // 本次合成是否由本对话框发起：决定关闭时是否调用 stop_tts 清理
  const startedByDialog = useRef(false);
  // 合成完成后是否自动播放（点「合成并播放」触发；被 autoplay 拦截时用户可点「播放」）
  const pendingAutoPlay = useRef(false);
  // 始终指向最新的 synthesizing 状态（供关闭时的异步判断使用）
  const synthesizingRef = useRef(tts.synthesizing);
  useEffect(() => {
    synthesizingRef.current = tts.synthesizing;
  }, [tts.synthesizing]);

  // 打开时挂载并播放进场动画；重置归属追踪；语速初始化用「高级配置」的全局默认值
  useEffect(() => {
    if (open) {
      setMounted(true);
      setClosing(false);
      startedByDialog.current = false;
      pendingAutoPlay.current = false;
      setSpeed(tts.config?.speed ?? 1);
    }
  }, [open, tts.config?.speed]);

  const enabled = tts.config?.enabled ?? true;

  // 「合成并播放」：记录归属 + 合成完成后自动播放
  const handleSynthesize = useCallback(() => {
    const trimmed = text.trim();
    if (!trimmed) return;
    startedByDialog.current = true;
    pendingAutoPlay.current = true;
    void tts.synthesize(trimmed, { speed });
  }, [text, speed, tts]);

  // 合成完成（result 更新）且需要自动播放时播放一次
  useEffect(() => {
    if (tts.result && pendingAutoPlay.current) {
      pendingAutoPlay.current = false;
      tts.play();
    }
  }, [tts.result, tts.play]);

  const stopDialogSynthesis = useCallback(() => {
    // fire-and-forget：stop_tts 会 join 到合成线程结束，不阻塞 UI
    void tts.stop();
  }, [tts]);

  const finishClose = useCallback(() => {
    const mine = startedByDialog.current;
    // playback：正在播放则暂停并清空 src，关闭后不得继续播放
    const el = tts.audioRef.current;
    if (el) {
      el.pause();
      el.removeAttribute("src");
      el.load();
    }
    setMounted(false);
    setClosing(false);
    onClose();
    // synthesis：仅停止「本对话框发起」且仍在合成的任务
    if (mine && synthesizingRef.current) stopDialogSynthesis();
  }, [onClose, stopDialogSynthesis, tts.audioRef]);

  const close = useCallback(() => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(finishClose, EXIT_MS);
  }, [closing, finishClose]);

  // Esc 取消；管理音色对话框叠层打开时不关本对话框
  useEffect(() => {
    if (!mounted || closing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !manageOpen) close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mounted, closing, manageOpen, close]);

  if (!mounted) return null;

  const modelName = modelNameFromDir(tts.config?.model_dir);
  // sid/固定音色模型（vits/matcha/kokoro/pocket）无参考音频克隆语义：音色固定，
  // 不提供选择与「管理音色」；克隆族（zipvoice/omnivoice/voxcpm2/qwen3_tts）可选音色
  const modelKind = tts.config?.model_type ?? "";
  const sidModel = !!modelKind && !isCloneTtsKind(modelKind);
  // 强制克隆族（qwen3_tts）：上游 Base 无 auto voice 兜底，无「默认音色」空值项
  const cloneRequired = isCloneRequiredTtsKind(modelKind);
  const synthPercent = Math.max(0, Math.min(100, (tts.progress ?? 0) * 100));

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-label="测试语音合成"
    >
      <button
        type="button"
        tabIndex={-1}
        aria-label="关闭对话框"
        className={cn(
          "absolute inset-0 cursor-default bg-black/20",
          closing ? "animate-out fade-out-0 duration-200" : "animate-in fade-in-0 duration-200",
        )}
        onClick={close}
      />
      <div
        className={cn(
          "relative flex max-h-[85vh] w-full max-w-xl flex-col rounded-xl border border-panel-border bg-panel-background",
          closing
            ? "animate-out fade-out-0 zoom-out-95 duration-200 ease-in"
            : "animate-in fade-in-0 zoom-in-95 duration-200 ease-out",
        )}
      >
        <div className="flex items-center justify-between gap-4 border-b border-divider px-5 py-4">
          <h3 className="text-sm font-semibold text-text-primary">测试语音合成</h3>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 shrink-0"
            onClick={close}
            aria-label="关闭"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>

        <div className="flex-1 space-y-3 overflow-y-auto px-5 py-4">
          {/* 当前模型 */}
          <dl className="rounded-md border border-panel-border bg-app-background/60">
            <div className="flex items-center justify-between gap-3 border-b border-divider px-3.5 py-2">
              <dt className="text-sm text-text-primary">当前模型</dt>
              <dd className="truncate text-sm text-text-secondary">{modelName ?? "未知模型"}</dd>
            </div>
          </dl>

          {/* 音色：默认 + 内置 + 已保存自定义（来自 list_tts_voices）；自定义经「管理音色」添加 */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between gap-2">
              <label className="text-sm text-text-primary" htmlFor="tts-test-voice">
                音色
              </label>
              {onManageVoices && !sidModel && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 gap-1 px-2 text-xs text-text-muted"
                  onClick={onManageVoices}
                >
                  <SlidersVertical className="h-3.5 w-3.5" />
                  管理音色
                </Button>
              )}
            </div>
            {sidModel ? (
              <Select value="fixed" disabled>
                <SelectTrigger id="tts-test-voice" aria-label="音色" className="w-full">
                  <SelectValue placeholder="默认音色（模型固定）" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="fixed">默认音色（模型固定）</SelectItem>
                </SelectContent>
              </Select>
            ) : (
              <Select
                value={tts.selectedVoice}
                onValueChange={tts.setSelectedVoice}
                disabled={tts.voices.length === 0}
              >
                <SelectTrigger id="tts-test-voice" aria-label="音色" className="w-full">
                  <SelectValue placeholder={cloneRequired ? "必须选择克隆音色" : "默认音色"} />
                </SelectTrigger>
                <SelectContent>
                  {!cloneRequired && <SelectItem value="">默认音色</SelectItem>}
                  {tts.voices.map((v) => (
                    <SelectItem key={v.id} value={v.id}>
                      {v.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </div>

          {/* 语速：真实 synthesize.speed 一次性参数 */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <label className="text-sm text-text-primary" htmlFor="tts-test-speed">
                语速
              </label>
              <span className="font-mono text-xs text-text-muted">{speed.toFixed(1)}x</span>
            </div>
            <Slider
              id="tts-test-speed"
              aria-label="语速"
              min={0.5}
              max={2}
              step={0.1}
              value={[speed]}
              onValueChange={(v) => setSpeed(v[0])}
              disabled={tts.synthesizing}
            />
          </div>

          {/* 测试文本 */}
          <div className="space-y-1.5">
            <label className="text-sm text-text-primary" htmlFor="tts-test-text">
              测试文本
            </label>
            <textarea
              id="tts-test-text"
              className="w-full rounded-md border border-input bg-background p-3 text-sm outline-none focus:ring-1 focus:ring-ring"
              rows={3}
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder="输入要合成的文本"
            />
          </div>

          {/* 状态 */}
          <div className="flex items-center gap-2">
            <span
              className={cn(
                "inline-flex items-center gap-1.5 text-sm font-medium",
                tts.synthesizing ? "text-blue-600" : "text-emerald-600",
              )}
            >
              <span className="h-1.5 w-1.5 rounded-full bg-current" />
              {tts.synthesizing ? "合成中" : "已就绪"}
            </span>
          </div>

          {tts.synthesizing && (
            <div className="space-y-1">
              <Progress value={synthPercent} />
              <p className="text-xs text-text-muted">合成中 {synthPercent.toFixed(0)}%</p>
            </div>
          )}

          {tts.result && (
            <p className="text-xs text-text-muted">
              已生成音频（{tts.result.duration.toFixed(1)}s · {tts.result.sample_rate}Hz）
            </p>
          )}

          {!enabled && (
            <Alert variant="warning">
              <CircleAlert className="h-4 w-4" />
              <AlertDescription className="whitespace-pre-wrap">
                语音合成已关闭，请在页面顶部开启后再测试。
              </AlertDescription>
            </Alert>
          )}

          {tts.error && (
            <Alert variant="destructive">
              <CircleAlert className="h-4 w-4" />
              <AlertDescription className="whitespace-pre-wrap">{tts.error}</AlertDescription>
            </Alert>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-divider px-5 py-3">
          <div className="flex flex-wrap gap-2">
            <Button
              onClick={handleSynthesize}
              disabled={tts.synthesizing || !enabled || !text.trim()}
            >
              <Play className="h-4 w-4" />
              合成并播放
            </Button>
            <Button variant="destructive" onClick={tts.stop} disabled={!tts.synthesizing}>
              <Square className="h-4 w-4" />
              停止
            </Button>
            {tts.audioUrl && !tts.synthesizing && (
              <Button variant="outline" onClick={tts.play}>
                <Volume2 className="h-4 w-4" />
                播放
              </Button>
            )}
          </div>
          <p className="shrink-0 text-xs text-text-muted">在本窗口内发起的合成，关闭时自动停止。</p>
        </div>

        {/* biome-ignore lint/a11y/useMediaCaption: 合成语音无字幕轨 */}
        <audio ref={tts.audioRef} src={tts.audioUrl ?? undefined} className="hidden" />
      </div>
    </div>
  );
}
