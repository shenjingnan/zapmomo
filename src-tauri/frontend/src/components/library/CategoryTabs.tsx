import { cn } from "@/lib/utils";
import type { ModelCategory } from "@/types/catalog";

// LLM 已改为远程连接（无本地模型），模型库只服务 KWS/ASR/TTS 三类本地模型。
const TABS: { value: ModelCategory | null; label: string }[] = [
  { value: null, label: "全部" },
  { value: "asr", label: "ASR" },
  { value: "tts", label: "TTS" },
  { value: "kws", label: "KWS" },
];

/** 分类 Tab（进入 query state；改动重置分页）。 */
export function CategoryTabs({
  value,
  onChange,
}: {
  value: ModelCategory | null;
  onChange: (cat: ModelCategory | null) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {TABS.map((t) => {
        const active = value === t.value;
        return (
          <button
            key={t.label}
            type="button"
            onClick={() => onChange(t.value)}
            className={cn(
              "inline-flex h-8 items-center rounded-full px-3.5 text-sm font-medium transition-colors",
              active
                ? "bg-nav-active text-primary"
                : "text-text-secondary hover:bg-nav-hover hover:text-text-primary",
            )}
          >
            {t.label}
          </button>
        );
      })}
    </div>
  );
}
