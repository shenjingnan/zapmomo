import { ChevronDown, CircleAlert, Save, SlidersHorizontal } from "lucide-react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import type { TtsConfigInfo, TtsParamsPatch } from "@/types/tauri";

type ParamKey = "num_steps" | "speed" | "num_threads";

const PARAM_KEYS: ParamKey[] = ["num_steps", "speed", "num_threads"];

interface ParamMeta {
  label: string;
  hint?: string;
  kind: "slider" | "number";
  min: number;
  max: number;
  step: number;
  suffix?: string;
}

/** 参数元数据：前端预校验边界与后端 `TtsParamsPatch::apply_to` 一致（后端是权威）。 */
const PARAM_META: Record<ParamKey, ParamMeta> = {
  num_steps: {
    label: "扩散步数",
    kind: "number",
    min: 1,
    max: 32,
    step: 1,
    suffix: "步",
    hint: "ZipVoice 扩散解码步数，越高音质越好、合成越慢。",
  },
  speed: {
    label: "默认语速",
    kind: "slider",
    min: 0.5,
    max: 2,
    step: 0.05,
    hint: "测试语音对话框的初始语速，可在测试中临时调整。",
  },
  num_threads: {
    label: "线程数",
    kind: "number",
    min: 1,
    max: 32,
    step: 1,
    suffix: "线程",
    hint: "用于推理的 CPU 线程数。",
  },
};

function toDraft(params: TtsConfigInfo | null | undefined): Record<ParamKey, string> {
  if (!params) {
    return { num_steps: "", speed: "", num_threads: "" };
  }
  return {
    num_steps: String(params.num_steps),
    speed: String(params.speed),
    num_threads: String(params.num_threads),
  };
}

function parseDraft(draft: Record<ParamKey, string>): TtsParamsPatch | null {
  const patch = {} as TtsParamsPatch;
  for (const k of PARAM_KEYS) {
    const raw = draft[k].trim();
    if (raw === "") return null;
    const v = Number(raw);
    if (!Number.isFinite(v)) return null;
    (patch as Record<ParamKey, number>)[k] = v;
  }
  return patch;
}

function isPristine(
  draft: Record<ParamKey, string> | null,
  params: TtsConfigInfo | null | undefined,
): boolean {
  if (!draft || !params) return true;
  const patch = parseDraft(draft);
  if (!patch) return false; // 非法值视为已修改，允许点保存触发校验
  return PARAM_KEYS.every(
    (k) => Math.abs((patch as Record<ParamKey, number>)[k] - params[k]) < 1e-6,
  );
}

interface ParamRowProps {
  key_: ParamKey;
  value: string;
  onChange: (v: string) => void;
}

function ParamRow({ key_, value, onChange }: ParamRowProps) {
  const meta = PARAM_META[key_];
  const numeric = Number(value);
  const valid = value.trim() !== "" && Number.isFinite(numeric);
  const sharedInput = {
    type: "text" as const,
    inputMode: "decimal" as const,
    value,
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => onChange(e.target.value),
    "aria-label": meta.label,
  };

  return (
    <div className="flex items-start gap-4 px-3.5 py-2.5">
      <div className="min-w-0 flex-1">
        <p className="text-sm text-text-primary">{meta.label}</p>
        {meta.hint && <p className="mt-0.5 text-xs text-text-muted">{meta.hint}</p>}
      </div>
      <div className="flex w-64 shrink-0 items-center justify-end gap-2.5 pt-0.5">
        {meta.kind === "slider" ? (
          <Slider
            value={[valid ? numeric : meta.min]}
            min={meta.min}
            max={meta.max}
            step={meta.step}
            onValueChange={([v]) => onChange(String(v))}
            className="min-w-0 flex-1"
            aria-label={meta.label}
          />
        ) : null}
        <div className="flex shrink-0 items-center gap-1">
          <Input {...sharedInput} className="w-20 text-right" />
          <span className="w-8 shrink-0 text-left text-xs text-text-muted">
            {meta.suffix ?? ""}
          </span>
        </div>
      </div>
    </div>
  );
}

/**
 * 高级参数：扩散步数、默认语速、线程数（批保存）+ 调试输出开关。
 * TTS 引擎在每次合成时新建，因此保存后**下一次合成即生效**，无需重启（区别于 KWS/ASR）。
 */
export function TtsAdvancedParams() {
  const { tts } = useRuntime();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<Record<ParamKey, string> | null>(null);
  const [debugDraft, setDebugDraft] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const params = tts.config;
  // 收录模型均为扩散/克隆模型，扩散步数全族适用（无按 kind 隐藏项）
  const visibleKeys = PARAM_KEYS;

  // hydrate：config 就绪时填充草稿；dirty 时保留用户编辑，否则随 config 同步
  useEffect(() => {
    if (!params) return;
    setDraft((prev) => (prev === null || isPristine(prev, params) ? toDraft(params) : prev));
    setDebugDraft(params.debug);
  }, [params]);

  const pristine = isPristine(draft, params) && debugDraft === (params?.debug ?? false);

  const handleEdit = (k: ParamKey, v: string) => {
    setSaveError(null);
    setDraft((prev) => {
      if (!prev) return prev;
      return { ...prev, [k]: v };
    });
  };

  const handleSave = async () => {
    if (!draft) return;
    const patch = parseDraft(draft);
    if (!patch) {
      setSaveError("请将全部参数填写为有效数字");
      return;
    }
    for (const k of PARAM_KEYS) {
      const meta = PARAM_META[k];
      const v = (patch as Record<ParamKey, number>)[k];
      if (v < meta.min || v > meta.max) {
        setSaveError(`${meta.label} 需在 ${meta.min}~${meta.max} 之间`);
        return;
      }
    }
    setSaving(true);
    setSaveError(null);
    try {
      await tts.setParams({ ...patch, debug: debugDraft });
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="rounded-[16px] border border-panel-border bg-panel-background">
      <Collapsible open={open} onOpenChange={setOpen}>
        <CollapsibleTrigger className="flex items-center justify-between gap-2 px-4 py-3 text-left">
          <span className="flex items-center gap-2.5">
            <SlidersHorizontal className="h-4 w-4 shrink-0 text-text-secondary" />
            <span>
              <h2 className="text-base font-semibold text-text-primary">高级参数</h2>
              <p className="mt-0.5 text-xs text-text-muted">扩散步数、语速、性能等</p>
            </span>
          </span>
          <ChevronDown
            className={cn(
              "h-4 w-4 shrink-0 text-text-muted transition-transform",
              open && "rotate-180",
            )}
          />
        </CollapsibleTrigger>
        <CollapsibleContent className="border-t border-divider">
          <div>
            {visibleKeys.map((k) => (
              <ParamRow
                key={k}
                key_={k}
                value={draft?.[k] ?? ""}
                onChange={(v) => handleEdit(k, v)}
              />
            ))}
            <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
              <div className="min-w-0">
                <p className="text-sm text-text-primary">调试输出</p>
                <p className="mt-0.5 text-xs text-text-muted">输出详细的推理调试日志。</p>
              </div>
              <Switch
                aria-label="调试输出"
                checked={debugDraft}
                onCheckedChange={(v) => {
                  setSaveError(null);
                  setDebugDraft(v);
                }}
              />
            </div>
          </div>

          {saveError && (
            <div className="px-3.5 pb-2.5">
              <Alert variant="destructive">
                <CircleAlert className="h-4 w-4" />
                <AlertDescription className="whitespace-pre-wrap">{saveError}</AlertDescription>
              </Alert>
            </div>
          )}

          <div className="flex flex-wrap items-center justify-between gap-2 px-3.5 py-2.5">
            <p className="text-xs text-text-muted">修改保存后，下一次合成自动生效。</p>
            <Button
              size="sm"
              disabled={pristine || saving}
              onClick={handleSave}
              aria-label="保存参数"
            >
              <Save className="h-4 w-4" />
              保存
            </Button>
          </div>
        </CollapsibleContent>
      </Collapsible>
    </section>
  );
}
