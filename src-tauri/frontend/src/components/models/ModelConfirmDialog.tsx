import { Button } from "@/components/ui/button";
import type { LibraryModel } from "@/types/modelLibrary";
import { ModelDialog } from "./ModelDialog";

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
    <ModelDialog
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
        确定要{external ? "移除登记" : "卸载"}{" "}
        <span className="font-semibold">{model?.displayName}</span> 吗？
      </p>
      <p className="text-sm text-text-secondary">
        {external
          ? "只会取消 ZapMomo 中的登记，不会删除你的原始模型文件。"
          : "模型文件将从本地删除。"}
      </p>
    </ModelDialog>
  );
}
