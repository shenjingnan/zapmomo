import { Plus, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import { CategoryTabs } from "@/components/library/CategoryTabs";
import { AddLocalModelDialog } from "@/components/library/LibraryDialogs";
import { ModelDetailDrawer } from "@/components/library/ModelDetailDrawer";
import { ModelFilterBar } from "@/components/library/ModelFilterBar";
import { ModelListPane } from "@/components/library/ModelListPane";
import { Button } from "@/components/ui/button";
import { useModelCatalog } from "@/hooks/useModelCatalog";
import { useModelDetail } from "@/hooks/useModelDetail";
import { useModelDownloads } from "@/hooks/useModelDownloads";
import { useModelLibrary } from "@/hooks/useModelLibrary";
import { cn } from "@/lib/utils";
import type { UnifiedModelItem } from "@/types/catalog";

/** 模型库：搜索/筛选 + 分类 Tab + 可滚动全宽列表；点击模型从右滑出详情抽屉（固定不随列表滚动）。 */
export function LibraryPage() {
  const catalog = useModelCatalog();
  const detail = useModelDetail();
  const downloads = useModelDownloads();
  const lib = useModelLibrary();
  const [addOpen, setAddOpen] = useState(false);

  // 默认只展示 ZapMomo 可用模型（Verified + Compatible）；「显示全部模型」打开后展示所有兼容级别
  const filtered = useMemo(() => {
    if (catalog.showAll) return catalog.items;
    return catalog.items.filter(
      (i) => i.compatibility === "verified" || i.compatibility === "compatible",
    );
  }, [catalog.items, catalog.showAll]);

  const onSelect = (item: UnifiedModelItem) => {
    detail.select(item);
  };

  return (
    <div className="flex h-full flex-col gap-3">
      {/* 顶部 */}
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-text-primary">模型库</h1>
          <p className="mt-0.5 text-sm text-text-secondary">发现、下载和管理 AI 模型</p>
        </div>
        <div className="flex items-center gap-2">
          <span className="inline-flex items-center gap-1.5 rounded-full border border-panel-border bg-panel-background px-2.5 py-1 text-xs text-text-secondary">
            <span className="h-1.5 w-1.5 rounded-full bg-blue-500" />
            Hugging Face
          </span>
          <Button
            variant="outline"
            size="sm"
            className="shadow-none"
            onClick={() => void catalog.refresh()}
            disabled={catalog.loading}
          >
            <RefreshCw className={cn("h-4 w-4", catalog.loading && "animate-spin")} />
            刷新
          </Button>
          <Button size="sm" onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            添加本地模型
          </Button>
        </div>
      </header>

      {/* 搜索 + 筛选 */}
      <ModelFilterBar
        search={catalog.query.search}
        onSearch={catalog.setSearch}
        language={catalog.query.language}
        onLanguage={catalog.setLanguage}
        parameters={catalog.query.parameters}
        onParameters={catalog.setParameters}
        sort={catalog.query.sort}
        onSort={catalog.setSort}
        showAll={catalog.showAll}
        onToggleShowAll={catalog.toggleShowAll}
      />

      {/* 分类 Tab */}
      <CategoryTabs value={catalog.query.category} onChange={catalog.setCategory} />

      {/* 列表（flex-1）+ 详情抽屉（占据右侧，左侧缩短）；列表内部滚动由 ModelListPane 管理 */}
      <div className="flex min-h-0 flex-1 gap-3">
        <div className="min-h-0 flex-1">
          <ModelListPane
            items={filtered}
            loading={catalog.loading}
            loadingMore={catalog.loadingMore}
            error={catalog.error}
            hasMore={catalog.hasMore}
            selectedId={detail.selected?.canonicalKey ?? null}
            onSelect={onSelect}
            onRetry={catalog.retry}
            onLoadMore={catalog.loadMore}
          />
        </div>
        <ModelDetailDrawer detail={detail} lib={lib} downloads={downloads} onClose={detail.close} />
      </div>

      <AddLocalModelDialog open={addOpen} onClose={() => setAddOpen(false)} onAddLocal={lib.addLocal} />
    </div>
  );
}
