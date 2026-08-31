import { HardDrive } from "lucide-react";
import { ModelDialog } from "@/components/models/ModelDialog";
import { Button } from "@/components/ui/button";
import { formatBytes } from "@/lib/utils";
import type { StoragePrompt } from "@/types/modelLibrary";

interface StoragePromptDialogProps {
  info: StoragePrompt;
  /** 目录写入进行中（禁用按钮防重复提交） */
  busy: boolean;
  /** 使用默认位置（确认一次性标记后放行） */
  onUseDefault: () => void;
  /** 打开系统目录选择器（选中的目录由 Provider 写入 set_data_dir） */
  onPickDir: () => void;
  /** 取消本次操作（不写标记，下次操作会再次询问） */
  onCancel: () => void;
}

/** 单行路径展示（标签 + 路径 + 可用空间）。 */
function DirRow({ label, dir, available }: { label: string; dir: string; available?: number }) {
  return (
    <div>
      <dt className="text-xs text-text-secondary">{label}</dt>
      <dd className="mt-0.5">
        <span className="break-all text-sm text-text-primary">{dir}</span>
        {available !== undefined && (
          <span className="ml-2 shrink-0 text-xs text-text-muted">
            可用 {formatBytes(available)}
          </span>
        )}
      </dd>
    </div>
  );
}

/**
 * 存储位置引导弹窗：首次下载/导入前询问模型存放目录。
 *
 * 背景：Windows 上默认落在系统盘（`C:\Users\<u>\.zapmomo`），大模型会挤占 C 盘。
 * 建议目录由后端挑选（非系统盘、非可移动盘、剩余空间最大），用户也可自选任意目录；
 * 「取消」不写确认标记，下次操作会再次询问。
 */
export function StoragePromptDialog({
  info,
  busy,
  onUseDefault,
  onPickDir,
  onCancel,
}: StoragePromptDialogProps) {
  return (
    <ModelDialog
      open
      onClose={onCancel}
      title="选择模型存储位置"
      width="lg"
      footer={
        <div className="flex justify-end gap-2">
          <Button size="sm" variant="ghost" onClick={onCancel} disabled={busy}>
            取消
          </Button>
          <Button size="sm" variant="outline" onClick={onPickDir} disabled={busy}>
            选择其他位置…
          </Button>
          <Button size="sm" onClick={onUseDefault} disabled={busy}>
            使用默认位置
          </Button>
        </div>
      }
    >
      <div className="flex items-start gap-2.5">
        <HardDrive className="mt-0.5 h-4 w-4 shrink-0 text-text-secondary" />
        <p className="text-sm text-text-muted">
          模型与角色文件将存放到以下目录。Windows 上默认位置在系统盘，若 C
          盘空间紧张，建议选择其他磁盘。
        </p>
      </div>
      <dl className="space-y-3 rounded-lg border border-panel-border px-3.5 py-2.5">
        <DirRow label="默认位置" dir={info.defaultDir} available={info.defaultAvailable} />
        {info.suggestedDir && (
          <DirRow
            label="建议位置（剩余空间更大的磁盘）"
            dir={info.suggestedDir}
            available={info.suggestedAvailable ?? undefined}
          />
        )}
      </dl>
      <p className="text-xs text-text-muted">
        settings、日志等小文件仍保留在 ~/.zapmomo，不会迁移；之后可随时在「设置 →
        存储位置」更改并迁移已有模型。
      </p>
    </ModelDialog>
  );
}
