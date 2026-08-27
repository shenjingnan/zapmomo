import { ChevronDown, CircleAlert, MessageSquareText, Save } from "lucide-react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { useRuntime } from "@/providers/RuntimeContext";

/**
 * 系统提示词：textarea 编辑 + 保存写 backend（`set_llm_system_prompt`）。
 * 修改需重新连接远程 provider 才生效（provider 创建时拷入 system_prompt）。
 */
export function LlmSystemPrompt() {
  const { llm } = useRuntime();
  const [open, setOpen] = useState(true);
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const configPrompt = llm.config?.system_prompt;

  // hydrate：config 就绪时填充一次，之后保留用户编辑
  useEffect(() => {
    if (configPrompt === undefined) return;
    setDraft((prev) => (prev === null ? configPrompt : prev));
  }, [configPrompt]);

  const pristine = draft === null || draft === configPrompt;

  const handleSave = async () => {
    if (draft === null) return;
    setSaving(true);
    setSaveError(null);
    try {
      await llm.setSystemPrompt(draft);
      // 系统提示词在 provider 创建时固化，已加载且内容变化时主动重载使其生效
      if (llm.ready && draft !== configPrompt) {
        await llm.load();
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
            <MessageSquareText className="h-4 w-4 shrink-0 text-text-secondary" />
            <span>
              <h2 className="text-base font-semibold text-text-primary">
                系统提示词（System Prompt）
              </h2>
              <p className="mt-0.5 text-xs text-text-muted">设定模型的角色与行为基础</p>
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
          <div className="space-y-3 px-4 py-3">
            <textarea
              className="w-full rounded-md border border-panel-border bg-app-background/60 p-3 text-sm text-text-primary outline-none transition-colors focus:border-primary/50 focus:ring-1 focus:ring-primary/20"
              rows={4}
              value={draft ?? ""}
              onChange={(e) => {
                setSaveError(null);
                setDraft(e.target.value);
              }}
              placeholder="输入系统提示词…"
              aria-label="系统提示词"
              disabled={draft === null}
            />

            {saveError && (
              <Alert variant="destructive">
                <CircleAlert className="h-4 w-4" />
                <AlertDescription className="whitespace-pre-wrap">{saveError}</AlertDescription>
              </Alert>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2">
              <p className="text-xs text-text-muted">保存后会自动重新连接使其生效。</p>
              <Button
                size="sm"
                disabled={pristine || saving}
                onClick={handleSave}
                aria-label="保存提示词"
              >
                <Save className="h-4 w-4" />
                保存
              </Button>
            </div>
          </div>
        </CollapsibleContent>
      </Collapsible>
    </section>
  );
}
