import { CircleAlert, MessageSquare, Save, Settings2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useRuntime } from "@/providers/RuntimeContext";
import { LlmTestDialog } from "./LlmTestDialog";

/**
 * 基础配置（macOS 设置行）：远程服务连接三要素（API 地址 / API Key / 模型名）
 * + 底部「测试模型」。
 *
 * API Key 不回显明文：输入框留空表示不修改已保存的 Key；placeholder 展示掩码。
 */
export function LlmCoreConfig() {
  const { llm } = useRuntime();
  const [testOpen, setTestOpen] = useState(false);

  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // hydrate：config 就绪时填充（仅首次；用户编辑后不覆盖）
  const [hydrated, setHydrated] = useState(false);
  useEffect(() => {
    if (hydrated || !llm.config) return;
    setBaseUrl(llm.config.base_url ?? "");
    setModel(llm.config.model ?? "");
    setHydrated(true);
  }, [hydrated, llm.config]);

  const busy = llm.loading || llm.generating;
  const testDisabled = !llm.ready || busy;
  const pristine =
    hydrated &&
    baseUrl === (llm.config?.base_url ?? "") &&
    model === (llm.config?.model ?? "") &&
    apiKey === "";

  const handleSave = async () => {
    if (!baseUrl.trim()) {
      setSaveError("请填写 API 地址（如 https://open.bigmodel.cn/api/paas/v4）");
      return;
    }
    if (!model.trim()) {
      setSaveError("请填写模型名（如 glm-4.7-flash）");
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      // apiKey 留空 = 不修改已保存的 Key
      await llm.setConnection(baseUrl.trim(), apiKey.trim() ? apiKey.trim() : null, model.trim());
      setApiKey("");
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <Settings2 className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">基础配置</h2>
            <p className="mt-0.5 text-xs text-text-muted">
              OpenAI 兼容的远程服务连接（可填智谱 / DeepSeek / Ollama 等任意兼容端点）
            </p>
          </div>
        </div>
      </div>

      {llm.configError && (
        <div className="space-y-2 px-3.5 pb-2">
          <Alert variant="destructive">
            <AlertDescription className="whitespace-pre-wrap">
              读取配置失败：{llm.configError}
            </AlertDescription>
          </Alert>
        </div>
      )}

      <div className="space-y-3 px-3.5 pb-3">
        <div className="space-y-1.5">
          <label htmlFor="llm-base-url" className="text-sm text-text-primary">
            API 地址
          </label>
          <Input
            id="llm-base-url"
            type="text"
            placeholder="https://open.bigmodel.cn/api/paas/v4"
            value={baseUrl}
            onChange={(e) => {
              setSaveError(null);
              setBaseUrl(e.target.value);
            }}
          />
        </div>

        <div className="space-y-1.5">
          <label htmlFor="llm-api-key" className="text-sm text-text-primary">
            API Key
          </label>
          <Input
            id="llm-api-key"
            type="password"
            placeholder={
              llm.config?.api_key_masked ? `已保存（${llm.config.api_key_masked}）` : "输入 API Key"
            }
            value={apiKey}
            onChange={(e) => {
              setSaveError(null);
              setApiKey(e.target.value);
            }}
          />
          <p className="text-xs text-text-muted">留空则不修改已保存的 Key。</p>
        </div>

        <div className="space-y-1.5">
          <label htmlFor="llm-model" className="text-sm text-text-primary">
            模型名
          </label>
          <Input
            id="llm-model"
            type="text"
            placeholder="glm-4.7-flash"
            value={model}
            onChange={(e) => {
              setSaveError(null);
              setModel(e.target.value);
            }}
          />
        </div>

        {saveError && (
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">{saveError}</AlertDescription>
          </Alert>
        )}
      </div>

      <div className="flex flex-wrap gap-2 border-t border-divider px-3.5 py-2.5">
        <Button onClick={handleSave} disabled={saving || pristine} aria-label="保存连接配置">
          <Save className="h-4 w-4" />
          保存连接配置
        </Button>
        <Button
          variant="secondary"
          className="shadow-none"
          disabled={testDisabled}
          onClick={() => setTestOpen(true)}
        >
          <MessageSquare className="h-4 w-4" />
          测试模型
        </Button>
      </div>

      <LlmTestDialog open={testOpen} onClose={() => setTestOpen(false)} />
    </section>
  );
}
