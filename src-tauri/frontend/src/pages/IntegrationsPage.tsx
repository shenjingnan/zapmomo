import { DshIntegrationCard } from "@/components/integrations/DshIntegrationCard";

/**
 * 插件集成页：以集成卡片呈现可与应用联动的外部工具。
 *
 * 首个集成：deepseek-harness（dsh 桥联动）。卡片自带环境检测 / 一键安装 / 在线状态；
 * 后续新集成在此页追加卡片即可（暂不做通用注册表机制，见设计方案 YAGNI）。
 */
export function IntegrationsPage() {
  return (
    <div className="space-y-4 pb-4">
      <h1 className="text-2xl font-semibold tracking-tight text-text-primary">插件集成</h1>
      <DshIntegrationCard />
    </div>
  );
}
