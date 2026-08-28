import { ChevronDown, CircleAlert, Save, SlidersHorizontal } from "lucide-react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";
import type { AsrConfigInfo, AsrParamsPatch } from "@/types/tauri";
import { isStreamingAsr } from "./asrMeta";

type NumericKey =
  | "num_threads"
  | "chunk_size"
  | "rule1_min_trailing_silence"
  | "rule2_min_trailing_silence"
  | "rule3_min_utterance_length"
  | "blank_penalty";

const NUMERIC_KEYS: NumericKey[] = [
  "num_threads",
  "chunk_size",
  "rule1_min_trailing_silence",
  "rule2_min_trailing_silence",
  "rule3_min_utterance_length",
  "blank_penalty",
];

interface NumericMeta {
  label: string;
  hint?: string;
  min: number;
  max: number;
  step: number;
  suffix?: string;
}

/** 参数元数据：前端预校验边界与后端 `AsrParamsPatch::apply_to` 一致（后端是权威）。 */
const NUMERIC_META: Record<NumericKey, NumericMeta> = {
  num_threads: {
    label: "线程数",
    min: 1,
    max: 32,
    step: 1,
    suffix: "线程",
    hint: "用于推理的 CPU 线程数。",
  },
  chunk_size: {
    label: "采样块大小",
    min: 400,
    max: 16000,
    step: 100,
    suffix: "采样",
    hint: "每次喂给模型的采样数（@16k），越小延迟越低、CPU 占用越高。",
  },
  rule1_min_trailing_silence: {
    label: "断句·尾随静音 1",
    min: 0,
    max: 10,
    step: 0.1,
    suffix: "秒",
    hint: "静音超过此时长且置信度足够时自动断句。",
  },
  rule2_min_trailing_silence: {
    label: "断句·尾随静音 2",
    min: 0,
    max: 10,
    step: 0.1,
    suffix: "秒",
    hint: "更宽松的静音断句阈值。",
  },
  rule3_min_utterance_length: {
    label: "断句·最大句长",
    min: 5,
    max: 60,
    step: 0.5,
    suffix: "秒",
    hint: "一句话最长持续时间，超过即强制断句。",
  },
  blank_penalty: {
    label: "空白符惩罚",
    min: 0,
    max: 2,
    step: 0.01,
    hint: "抑制空白符输出，通常保持 0.0。",
  },
};

function toDraft(params: AsrConfigInfo, keys: NumericKey[]): Record<NumericKey, string> {
  const out = {} as Record<NumericKey, string>;
  for (const k of keys) out[k] = String(params[k]);
  return out;
}

function parseNumericDraft(
  draft: Record<NumericKey, string>,
  keys: NumericKey[],
): Record<NumericKey, number> | null {
  const out = {} as Record<NumericKey, number>;
  for (const k of keys) {
    // 防御：模型族切换瞬间旧草稿可能缺新键（hydrate 重建前的渲染），缺键视为未填
    const raw = draft[k]?.trim() ?? "";
    if (raw === "") return null;
    const v = Number(raw);
    if (!Number.isFinite(v)) return null;
    out[k] = v;
  }
  return out;
}

function isPristine(
  draft: Record<NumericKey, string> | null,
  params: AsrConfigInfo | null | undefined,
  keys: NumericKey[],
): boolean {
  if (!draft || !params) return true;
  const parsed = parseNumericDraft(draft, keys);
  if (!parsed) return false; // 非法值视为已修改，允许点保存触发校验
  return keys.every((k) => Math.abs(parsed[k] - params[k]) < 1e-6);
}

interface NumericRowProps {
  key_: NumericKey;
  value: string;
  onChange: (v: string) => void;
}

function NumericRow({ key_, value, onChange }: NumericRowProps) {
  const meta = NUMERIC_META[key_];
  return (
    <div className="flex items-start gap-4 px-3.5 py-2.5">
      <div className="min-w-0 flex-1">
        <p className="text-sm text-text-primary">{meta.label}</p>
        {meta.hint && <p className="mt-0.5 text-xs text-text-muted">{meta.hint}</p>}
      </div>
      <div className="flex w-64 shrink-0 items-center justify-end gap-2.5 pt-0.5">
        <div className="flex shrink-0 items-center gap-1">
          <Input
            type="text"
            inputMode="decimal"
            value={value}
            onChange={(e) => onChange(e.target.value)}
            className="w-20 text-right"
            aria-label={meta.label}
          />
          <span className="w-8 shrink-0 text-left text-xs text-text-muted">
            {meta.suffix ?? ""}
          </span>
        </div>
      </div>
    </div>
  );
}

/**
 * 高级参数：线程数 / 采样块大小 / 端点检测（尾随静音 1、2 / 最大句长）/ 空白符惩罚 /
 * 热词增强 / 自动标点 / 调试输出（批保存）。
 * 引擎参数在识别启动时固化：保存后若正在识别会自动重启识别使改动生效。
 */
export function AsrAdvancedParams() {
  const { asr, device } = useRuntime();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<Record<NumericKey, string> | null>(null);
  const [hotwordsDraft, setHotwordsDraft] = useState<string | null>(null);
  const [endpointDraft, setEndpointDraft] = useState<boolean | null>(null);
  const [punctDraft, setPunctDraft] = useState<boolean | null>(null);
  const [debugDraft, setDebugDraft] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const params = asr.config.config;
  const punctAvailable = params?.punctuation_present ?? false;
  // 离线模型（SenseVoice/Whisper）无流式语义：隐藏 chunk_size/断句/空白惩罚/热词/端点检测
  const streaming = isStreamingAsr(params?.model_type);
  // 热词/空白惩罚为 transducer（zipformer）专属：paraformer 隐藏（后端也会忽略）
  const zipformerOnly = params?.model_type === "zipformer" || !params?.model_type;
  // 热词：zipformer 走 context graph、Qwen3-ASR 嵌提示词（后端转逗号格式），均可见可存；
  // audio.cpp 后端上游无热词选项，隐藏（后端 patch 层也会过滤）
  const hotwordsSupported =
    (zipformerOnly || params?.model_type === "qwen3_asr") && params?.backend !== "audiocpp";
  const numericKeys: NumericKey[] = zipformerOnly
    ? NUMERIC_KEYS
    : streaming
      ? NUMERIC_KEYS.filter((k) => k !== "blank_penalty")
      : NUMERIC_KEYS.filter((k) => k === "num_threads");

  // hydrate：config 就绪时填充草稿；数字草稿在 dirty 时保留用户编辑，开关/热词仅首次填充。
  // 键集随模型族变化（如 qwen3 → zipformer 扩出 blank_penalty/断句），旧草稿缺键时
  // 整体重建，避免用残缺草稿做 pristine 比对。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 有意仅随 params 刷新；numericKeys 为每次渲染重建的派生数组，入 deps 会无限循环
  useEffect(() => {
    if (!params) return;
    setDraft((prev) =>
      prev === null || !numericKeys.every((k) => k in prev) || isPristine(prev, params, numericKeys)
        ? toDraft(params, numericKeys)
        : prev,
    );
    setHotwordsDraft((prev) => (prev === null ? (params.hotwords ?? "") : prev));
    setEndpointDraft((prev) => (prev === null ? params.enable_endpoint : prev));
    setPunctDraft((prev) => (prev === null ? params.enable_punctuation : prev));
    setDebugDraft((prev) => (prev === null ? params.debug : prev));
  }, [params]); // eslint-disable-line react-hooks/exhaustive-deps

  const hydrated =
    draft !== null &&
    hotwordsDraft !== null &&
    endpointDraft !== null &&
    punctDraft !== null &&
    debugDraft !== null &&
    params != null;
  const pristine =
    hydrated &&
    isPristine(draft, params, numericKeys) &&
    hotwordsDraft === (params.hotwords ?? "") &&
    endpointDraft === params.enable_endpoint &&
    punctDraft === params.enable_punctuation &&
    debugDraft === params.debug;

  const handleNumericEdit = (k: NumericKey, v: string) => {
    setSaveError(null);
    setDraft((prev) => {
      if (!prev) return prev;
      return { ...prev, [k]: v };
    });
  };

  const handleSave = async () => {
    if (!draft || !params) return;
    const numeric = parseNumericDraft(draft, numericKeys);
    if (!numeric) {
      setSaveError("请将全部参数填写为有效数字");
      return;
    }
    for (const k of numericKeys) {
      const meta = NUMERIC_META[k];
      const v = numeric[k];
      if (v < meta.min || v > meta.max) {
        setSaveError(`${meta.label} 需在 ${meta.min}~${meta.max} 之间`);
        return;
      }
    }
    const patch: AsrParamsPatch = {
      ...numeric,
      // 热词仅 zipformer/Qwen3-ASR 生效、端点检测属流式语义：不适用时不随保存下发（后端缺省不修改）
      hotwords: hotwordsSupported ? (hotwordsDraft ?? "") : undefined,
      enable_endpoint: streaming ? (endpointDraft ?? params.enable_endpoint) : undefined,
      enable_punctuation: punctDraft ?? params.enable_punctuation,
      debug: debugDraft ?? params.debug,
    };
    setSaving(true);
    setSaveError(null);
    try {
      await asr.config.setParams(patch);
      // 引擎参数固化于识别启动时：若正在识别，重启使改动生效
      if (asr.listening.isListening) {
        await asr.listening.stop();
        await asr.listening.start(device || null);
      }
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
              <p className="mt-0.5 text-xs text-text-muted">断句、性能、热词与标点</p>
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
            {numericKeys.map((k) => (
              <NumericRow
                key={k}
                key_={k}
                value={draft?.[k] ?? ""}
                onChange={(v) => handleNumericEdit(k, v)}
              />
            ))}

            {hotwordsSupported && (
              <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
                <div className="min-w-0">
                  <p className="text-sm text-text-primary">热词增强</p>
                  <p className="mt-0.5 text-xs text-text-muted">
                    {zipformerOnly
                      ? "空格分隔（中文直接写），提升专有名词/人名识别；设置后引擎自动启用束搜索。"
                      : "空格分隔（中文直接写），嵌入 Qwen3 提示词提升专有名词/人名识别。"}
                  </p>
                </div>
                <Input
                  className="w-64 shrink-0"
                  value={hotwordsDraft ?? ""}
                  onChange={(e) => {
                    setSaveError(null);
                    setHotwordsDraft(e.target.value);
                  }}
                  placeholder="如：文森特卡索 ZapMomo"
                  aria-label="热词增强"
                />
              </div>
            )}

            {streaming && (
              <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
                <div className="min-w-0">
                  <p className="text-sm text-text-primary">端点检测</p>
                  <p className="mt-0.5 text-xs text-text-muted">
                    静音自动断句，开启后说一句话自动出最终结果。
                  </p>
                </div>
                <Switch
                  aria-label="端点检测"
                  checked={endpointDraft ?? params?.enable_endpoint ?? true}
                  onCheckedChange={(v) => {
                    setSaveError(null);
                    setEndpointDraft(v);
                  }}
                  trackClass="bg-emerald-500"
                />
              </div>
            )}

            <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
              <div className="min-w-0">
                <p className="text-sm text-text-primary">自动标点</p>
                <p className="mt-0.5 text-xs text-text-muted">
                  识别结果自动加中文标点。
                  {!punctAvailable && "（当前未安装标点模型，开启不生效）"}
                </p>
              </div>
              <Switch
                aria-label="自动标点"
                checked={punctDraft ?? params?.enable_punctuation ?? true}
                onCheckedChange={(v) => {
                  setSaveError(null);
                  setPunctDraft(v);
                }}
                trackClass="bg-emerald-500"
              />
            </div>

            <div className="flex items-center justify-between gap-3.5 px-3.5 py-2.5">
              <div className="min-w-0">
                <p className="text-sm text-text-primary">调试输出</p>
                <p className="mt-0.5 text-xs text-text-muted">输出详细的推理调试日志。</p>
              </div>
              <Switch
                aria-label="调试输出"
                checked={debugDraft ?? params?.debug ?? false}
                onCheckedChange={(v) => {
                  setSaveError(null);
                  setDebugDraft(v);
                }}
                trackClass="bg-emerald-500"
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
            <p className="text-xs text-text-muted">
              修改保存后，若正在识别会自动重启识别使改动生效。
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
