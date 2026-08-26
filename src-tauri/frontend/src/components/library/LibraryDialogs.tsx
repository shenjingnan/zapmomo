import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import type { LibraryModel, ModelType } from "@/types/modelLibrary";
import { LibraryDialog } from "./LibraryDialog";
import { formatBytes, TYPE_META, tagLabel } from "./libraryMeta";

// ---------------------------------------------------------------- 确认框 ----

interface ModelConfirmDialogProps {
  open: boolean;
  onClose: () => void;
  model: LibraryModel | null;
  onConfirm: (model: LibraryModel) => void;
}

/** 卸载（managed）/ 移除（external）确认框。 */
export function ModelConfirmDialog({ open, onClose, model, onConfirm }: ModelConfirmDialogProps) {
  const external = model?.ownership === "external";
  return (
    <LibraryDialog
      open={open}
      onClose={onClose}
      title={external ? "移除模型" : "卸载模型"}
      width="md"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            取消
          </Button>
          <Button
            variant={external ? "outline" : "destructive"}
            aria-label={external ? "确认移除" : "确认卸载"}
            onClick={() => model && onConfirm(model)}
          >
            {external ? "移除" : "卸载"}
          </Button>
        </div>
      }
    >
      <p className="text-sm text-text-primary">
        确定要{external ? "从模型库移除" : "卸载"}{" "}
        <span className="font-semibold">{model?.displayName}</span> 吗？
      </p>
      <p className="text-sm text-text-secondary">
        {external
          ? "只会取消 ZapMomo 中的登记，不会删除你的原始模型文件。"
          : "模型文件将从本地删除。"}
      </p>
    </LibraryDialog>
  );
}

// ---------------------------------------------------------------- 详情框 ----

interface ModelDetailDialogProps {
  open: boolean;
  onClose: () => void;
  model: LibraryModel | null;
}

function DetailRow({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 py-1.5 text-sm">
      <dt className="shrink-0 text-text-secondary">{label}</dt>
      <dd className="min-w-0 truncate text-right text-text-primary">{value}</dd>
    </div>
  );
}

/** 模型详情（只读；评分/评论/下载量等未来再做）。 */
export function ModelDetailDialog({ open, onClose, model }: ModelDetailDialogProps) {
  if (!model) return null;
  const meta = TYPE_META[model.modelType];
  return (
    <LibraryDialog open={open} onClose={onClose} title="模型详情" width="lg">
      <div className="flex items-center gap-3">
        <span
          className={cn("flex h-10 w-10 items-center justify-center rounded-full", meta.iconClass)}
        >
          <meta.icon className="h-5 w-5" />
        </span>
        <div>
          <p className="text-sm font-semibold text-text-primary">{model.displayName}</p>
          <p className="text-xs text-text-secondary">{model.id}</p>
        </div>
      </div>

      <dl className="divide-y divide-divider rounded-md border border-panel-border">
        <DetailRow label="类型" value={meta.label} />
        <DetailRow label="运行时" value={model.runtime} />
        <DetailRow label="格式" value={model.format} />
        {model.parameterCount && <DetailRow label="参数量" value={model.parameterCount} />}
        {model.quantization && <DetailRow label="量化" value={model.quantization} />}
        <DetailRow label="版本" value={model.version || "—"} />
        <DetailRow label="大小" value={formatBytes(model.sizeBytes)} />
        {model.languages.length > 0 && (
          <DetailRow label="语言" value={model.languages.join(" / ")} />
        )}
        {model.tags.length > 0 && (
          <DetailRow label="标签" value={model.tags.map(tagLabel).join(" / ")} />
        )}
        <DetailRow
          label="来源"
          value={
            model.source === "registry"
              ? model.ownership === "managed"
                ? "ZapMomo 下载"
                : "Registry（本地导入）"
              : "本地模型"
          }
        />
        {model.localPath && <DetailRow label="安装位置" value={model.localPath} />}
        {model.installedAt && <DetailRow label="安装时间" value={model.installedAt} />}
        {model.homepage && (
          <DetailRow
            label="主页"
            value={
              <a
                href={model.homepage}
                target="_blank"
                rel="noreferrer"
                className="text-blue-600 hover:underline"
              >
                {model.homepage}
              </a>
            }
          />
        )}
      </dl>
    </LibraryDialog>
  );
}

// ---------------------------------------------------------- 添加本地模型 ----

interface AddLocalModelDialogProps {
  open: boolean;
  onClose: () => void;
  onAddLocal: (
    path: string,
    modelType?: ModelType | null,
    registryId?: string | null,
  ) => Promise<void>;
}

// LLM 已改为远程连接（无本地 GGUF），本地模型只支持 KWS/ASR/TTS 目录。
const TYPE_CHOICES: { value: string; label: string }[] = [
  { value: "auto", label: "自动识别" },
  { value: "kws", label: "KWS（唤醒词）" },
  { value: "asr", label: "ASR（语音识别）" },
  { value: "tts", label: "TTS（语音合成）" },
];

/**
 * 添加本地模型对话框：选择模型目录（自动识别 / 手动选类型）。
 * 只注册路径，不复制大文件。
 */
export function AddLocalModelDialog({ open, onClose, onAddLocal }: AddLocalModelDialogProps) {
  const [typeChoice, setTypeChoice] = useState("auto");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) {
      setTypeChoice("auto");
      setBusy(false);
    }
  }, [open]);

  const doAdd = async (path: string, modelType?: ModelType | null) => {
    setBusy(true);
    try {
      await onAddLocal(path, modelType, null);
      onClose();
    } finally {
      setBusy(false);
    }
  };

  const pickDirectory = async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === "string") {
      await doAdd(dir, typeChoice === "auto" ? null : (typeChoice as ModelType));
    }
  };

  return (
    <LibraryDialog open={open} onClose={onClose} title="添加本地模型" width="md">
      <p className="text-sm text-text-secondary">选择模型目录。只注册路径，不复制大文件。</p>

      <div className="space-y-2">
        <Button
          variant="outline"
          className="w-full justify-start"
          onClick={pickDirectory}
          disabled={busy}
        >
          <FolderOpen className="h-4 w-4" />
          选择模型目录
        </Button>
      </div>

      <div className="space-y-1.5">
        <p className="text-xs text-text-secondary">目录类型</p>
        <Select value={typeChoice} onValueChange={setTypeChoice}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {TYPE_CHOICES.map((c) => (
              <SelectItem key={c.value} value={c.value}>
                {c.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className="text-[11px] text-text-muted">
          选「自动识别」时，ZapMomo 仅当目录能唯一匹配一种模型类型才添加。
        </p>
      </div>
    </LibraryDialog>
  );
}
