import { X } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

/**
 * 宽度档位映射。必须双写 sm: 前缀字面量：
 * DialogContent 默认带 sm:max-w-lg（responsive variant 排序在无前缀规则之后，
 * 会覆盖无前缀的 max-w-*），只有同断点值才能在 twMerge 中胜出。
 */
const WIDTHS = {
  sm: "max-w-sm sm:max-w-sm",
  md: "max-w-md sm:max-w-md",
  lg: "max-w-lg sm:max-w-lg",
} as const;

type DialogWidth = keyof typeof WIDTHS;

interface ModelDialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  /** 底部操作区（确认框的取消/确认按钮等） */
  footer?: ReactNode;
  /** 弹窗宽度档位：sm=384px / md=448px / lg=512px */
  width?: DialogWidth;
}

/**
 * 通用对话框外壳：基于 shadcn/ui Dialog（Radix），
 * 自动获得焦点陷阱、焦点归还、滚动锁定与 Escape 关闭；
 * 视觉保持 macOS 面板风格（浅遮罩 + 面板色卡片 + 200ms fade/zoom）。
 */
export function ModelDialog({
  open,
  onClose,
  title,
  children,
  footer,
  width = "md",
}: ModelDialogProps) {
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        overlayClassName="bg-black/20 duration-200"
        className={cn(
          // w-[calc(100%-2rem)]：fixed 定位下 100% 即视口宽，窄窗口两侧各留 1rem（等效原版外层 p-4）
          "flex max-h-[85vh] w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden rounded-xl border-panel-border bg-panel-background p-0 shadow-none",
          WIDTHS[width],
        )}
      >
        <div className="flex items-center justify-between gap-4 border-b border-divider px-5 py-4">
          <DialogTitle className="text-left text-sm font-semibold text-text-primary">
            {title}
          </DialogTitle>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 shrink-0"
            onClick={onClose}
            aria-label="关闭"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
        <div className="flex-1 space-y-3 overflow-y-auto px-5 py-4">{children}</div>
        {footer && <div className="border-t border-divider px-5 py-3">{footer}</div>}
      </DialogContent>
    </Dialog>
  );
}
