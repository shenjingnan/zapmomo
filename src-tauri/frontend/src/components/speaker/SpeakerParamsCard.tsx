import { SlidersHorizontal } from "lucide-react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { useRuntime } from "@/providers/RuntimeContext";

/** 识别参数卡：相似度阈值（拖动松手即存）+ 最短语音时长 + 折叠高级参数。 */
export function SpeakerParamsCard() {
  const { speaker } = useRuntime();
  const { config, setParams } = speaker.config;

  const [threshold, setThreshold] = useState(0.6);
  const [minDuration, setMinDuration] = useState("1.0");
  const [respondOnly, setRespondOnly] = useState(false);
  const [numThreads, setNumThreads] = useState("1");
  const [debug, setDebug] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // 配置回读时同步表单（仅初次/刷新时）
  useEffect(() => {
    if (!config) return;
    setThreshold(config.threshold);
    setMinDuration(String(config.min_audio_duration_secs));
    setRespondOnly(config.respond_only_matched);
    setNumThreads(String(config.num_threads));
    setDebug(config.debug);
  }, [config]);

  const parsedMin = Number.parseFloat(minDuration);
  const parsedThreads = Number.parseInt(numThreads, 10);
  const canSave =
    Number.isFinite(parsedMin) &&
    parsedMin >= 0.1 &&
    parsedMin <= 5 &&
    Number.isFinite(parsedThreads) &&
    parsedThreads >= 1 &&
    parsedThreads <= 32;

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      await setParams({
        threshold,
        min_audio_duration_secs: parsedMin,
        respond_only_matched: respondOnly,
        num_threads: parsedThreads,
        debug,
      });
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <SlidersHorizontal className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">识别参数</h2>
            <p className="mt-0.5 text-xs text-text-muted">修改后自动重启语音会话使其生效</p>
          </div>
        </div>
      </div>

      {error && (
        <div className="px-3.5 pb-2">
          <Alert variant="destructive">
            <AlertDescription className="whitespace-pre-wrap">{error}</AlertDescription>
          </Alert>
        </div>
      )}

      <dl>
        <div className="flex items-center justify-between gap-6 px-3.5 py-2.5">
          <dt className="shrink-0 text-sm text-text-primary">
            相似度阈值
            <span className="mt-0.5 block text-xs text-text-muted">
              越大越严格；低于阈值判为 unknown
            </span>
          </dt>
          <dd className="flex w-52 shrink-0 items-center gap-3">
            <Slider
              value={[threshold]}
              min={0.3}
              max={0.9}
              step={0.05}
              aria-label="相似度阈值"
              onValueChange={(vals) => setThreshold(vals[0] ?? threshold)}
            />
            <span className="w-10 text-right font-mono text-sm text-text-primary">
              {threshold.toFixed(2)}
            </span>
          </dd>
        </div>
        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <dt className="shrink-0 text-sm text-text-primary">
            最短语音时长（秒）
            <span className="mt-0.5 block text-xs text-text-muted">
              短于该时长的语音直接跳过识别（防「嗯/啊」误识别）
            </span>
          </dt>
          <dd className="w-52 shrink-0 text-right">
            <Input
              type="number"
              min={0.1}
              max={5}
              step={0.1}
              value={minDuration}
              onChange={(e) => setMinDuration(e.target.value)}
              aria-label="最短语音时长（秒）"
              className="ml-auto h-8 w-24 text-right"
            />
          </dd>
        </div>
        <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
          <dt className="shrink-0 text-sm text-text-primary">
            仅响应已注册说话人
            <span className="mt-0.5 block text-xs text-text-muted">
              不匹配的语音将被忽略（不回复、不入历史、不存记录）；短于最短时长的
              语音无法确认身份，同样会被忽略。欢迎语对所有人播放。
            </span>
          </dt>
          <dd>
            <Switch
              checked={respondOnly}
              onCheckedChange={setRespondOnly}
              aria-label="仅响应已注册说话人"
            />
          </dd>
        </div>
      </dl>

      <div className="border-t border-divider px-3.5 py-2">
        <button
          type="button"
          className="text-xs text-text-secondary underline-offset-2 hover:text-text-primary hover:underline"
          onClick={() => setShowAdvanced((v) => !v)}
        >
          {showAdvanced ? "收起高级参数" : "高级参数"}
        </button>
      </div>

      {showAdvanced && (
        <dl>
          <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
            <dt className="shrink-0 text-sm text-text-primary">推理线程数</dt>
            <dd className="w-52 shrink-0 text-right">
              <Input
                type="number"
                min={1}
                max={32}
                value={numThreads}
                onChange={(e) => setNumThreads(e.target.value)}
                aria-label="推理线程数"
                className="ml-auto h-8 w-24 text-right"
              />
            </dd>
          </div>
          <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
            <dt className="shrink-0 text-sm text-text-primary">调试日志</dt>
            <dd>
              <Switch checked={debug} onCheckedChange={setDebug} aria-label="调试日志" />
            </dd>
          </div>
        </dl>
      )}

      <div className="flex items-center justify-end gap-2 border-t border-divider px-3.5 py-2.5">
        {saved && <span className="text-xs text-emerald-600">已保存</span>}
        <Button size="sm" onClick={() => void handleSave()} disabled={!canSave || saving}>
          {saving ? "保存中…" : "保存参数"}
        </Button>
      </div>
    </section>
  );
}
