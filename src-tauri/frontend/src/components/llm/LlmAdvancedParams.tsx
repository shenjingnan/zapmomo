import { ChevronDown, CircleAlert, Save, SlidersHorizontal } from "lucide-react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import type { LlmParams, LlmParamsPatch } from "@/types/tauri";

type ParamKey = keyof LlmParamsPatch;

const PARAM_KEYS: ParamKey[] = [
  "temperature",
  "top_p",
  "max_tokens",
  "top_k",
  "min_p",
  "repeat_penalty",
  "seed",
];

interface ParamMeta {
  label: string;
  hint?: string;
  kind: "slider" | "number";
  min: number;
  max: number;
  step: number;
  suffix?: string;
}

/** 参数元数据：前端预校验边界与后端 `LlmParamsPatch::apply_to` 一致（后端是权威）。 */
const PARAM_META: Record<ParamKey, ParamMeta> = {
  max_tokens: {
    label: "最大生成 Tokens",
    kind: "number",
    min: 16,
    max: 262_144,
    step: 16,
    suffix: "token",
    hint: "单次回复最多生成的 token 数，越大回复越长、耗时越久。",
  },
  temperature: {
    label: "温度",
    kind: "slider",
    min: 0,
    max: 2,
    step: 0.05,
    hint: "越高回答越随机有创意，越低越稳定保守（0 = 总是选最可能的词）。",
  },
  top_p: {
    label: "Top-P",
    kind: "slider",
    min: 0,
    max: 1,
    step: 0.01,
    hint: "只从累计概率占比最高的词中采样，越小越保守、越大越多样。",
  },
  top_k: {
    label: "Top-K",
    kind: "number",
    min: 0,
    max: 500,
    step: 1,
    hint: "只从前 K 个最可能的词中采样，0 = 关闭该限制。",
  },
  min_p: {
    label: "Min-P",
    kind: "number",
    min: 0,
    max: 1,
    step: 0.01,
    hint: "过滤概率过低（低于最高词概率 × Min-P）的词，降低噪音。",
  },
  repeat_penalty: {
    label: "重复惩罚",
    kind: "number",
    min: 0,
    max: 3,
    step: 0.05,
    hint: "惩罚重复出现的词，值越大越少重复，1 = 关闭，小于 1 会鼓励重复。",
  },
  seed: {
    label: "随机种子",
    kind: "number",
    min: 0,
    max: 4_294_967_295,
    step: 1,
    hint: "固定后每次生成结果可复现，0 = 每次随机。",
  },
};

function toDraft(params: LlmParams | undefined): Record<ParamKey, string> {
  const out = {} as Record<ParamKey, string>;
  for (const k of PARAM_KEYS) out[k] = params ? String(params[k]) : "";
  return out;
}

function parseDraft(draft: Record<ParamKey, string>): LlmParamsPatch | null {
  const patch: LlmParamsPatch = {};
  for (const k of PARAM_KEYS) {
    const raw = draft[k].trim();
    if (raw === "") return null;
    const v = Number(raw);
    if (!Number.isFinite(v)) return null;
    patch[k] = v;
  }
  return patch;
}

function isPristine(
  draft: Record<ParamKey, string> | null,
  params: LlmParams | undefined,
): boolean {
  if (!draft || !params) return true;
  const patch = parseDraft(draft);
  if (!patch) return false; // 非法值视为已修改，允许点保存触发校验
  return PARAM_KEYS.every((k) => Math.abs((patch[k] as number) - params[k]) < 1e-6);
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
  // 用 text + inputMode 承载数字（Live2dCard 先例），避免 number 输入在浏览器里对
  // 小数中间态（如 "0."）的裁剪导致受控值抖动；范围校验在保存时统一做。
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
      {/* 固定宽度控件列 + 输入框右对齐：保证各行输入框右缘对齐（后缀用固定宽度槽，不顶开输入框） */}
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
 * 高级参数：采样参数（7 项），批量「保存」写 backend。
 * 草稿用字符串 map（Live2dCard 模式），点保存才 parse + 校验；温度/Top-P 用滑块 + 数字输入。
 */
export function LlmAdvancedParams() {
  const { llm } = useRuntime();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<Record<ParamKey, string> | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const params = llm.config?.params;

  // hydrate：config 就绪时填充草稿；dirty 时保留用户编辑，否则随 config 同步
  useEffect(() => {
    if (!params) return;
    setDraft((prev) => (prev === null || isPristine(prev, params) ? toDraft(params) : prev));
  }, [params]);

  const pristine = isPristine(draft, params);

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
      await llm.setParams(patch);
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
              <p className="mt-0.5 text-xs text-text-muted">采样与运行参数</p>
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
            {PARAM_KEYS.map((k) => (
              <ParamRow
                key={k}
                key_={k}
                value={draft?.[k] ?? ""}
                onChange={(v) => handleEdit(k, v)}
              />
            ))}
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
            <p className="text-xs text-text-muted">
              采样参数随请求发送，下次对话即生效；是否生效取决于服务端对 OpenAI 兼容参数的支持。
            </p>
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
