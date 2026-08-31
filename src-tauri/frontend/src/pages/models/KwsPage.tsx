import { ArrowLeft, CircleAlert } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router-dom";
import { KwsAdvancedParams } from "@/components/kws/KwsAdvancedParams";
import { KwsBasicConfig } from "@/components/kws/KwsBasicConfig";
import { KwsModelDialog } from "@/components/kws/KwsModelDialog";
import { KwsRunControl } from "@/components/kws/KwsRunControl";
import { KwsTechnicalInfo } from "@/components/kws/KwsTechnicalInfo";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { useRuntime } from "@/providers/RuntimeContext";

/**
 * 唤醒词（KWS）配置页：标题行含监听开关 + 基础配置 + 模型信息 + 高级参数。
 *
 * 监听错误详情在本页渲染：模型摘要/概览只显示「错误」两字，错误文案的唯一
 * 展示面在测试弹窗移走后曾无处安放，这里兜底透出（错误在下次启动成功时清除）。
 */
export function KwsPage() {
  const { kws } = useRuntime();
  const [switchOpen, setSwitchOpen] = useState(false);

  return (
    <div className="space-y-4">
      <Link
        to="/models"
        className="inline-flex items-center gap-1.5 text-sm text-text-secondary transition-colors hover:text-text-primary"
      >
        <ArrowLeft className="h-4 w-4" />
        模型与能力
      </Link>

      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <h1 className="text-2xl font-semibold tracking-tight text-text-primary">
          唤醒词（KWS）配置
        </h1>
        <KwsRunControl />
      </header>

      {kws.listening.error && (
        <Alert variant="destructive">
          <CircleAlert className="h-4 w-4" />
          <AlertDescription className="whitespace-pre-wrap">{kws.listening.error}</AlertDescription>
        </Alert>
      )}

      <KwsBasicConfig onSwitchOpen={() => setSwitchOpen(true)} />

      <KwsTechnicalInfo />

      <KwsAdvancedParams />

      <KwsModelDialog open={switchOpen} onClose={() => setSwitchOpen(false)} />
    </div>
  );
}
