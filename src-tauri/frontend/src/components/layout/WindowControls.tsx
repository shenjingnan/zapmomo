import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";

/** 窗口按钮样式（非 macOS 自绘三键）。 */
const windowButtonClass =
  "flex h-full w-9 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground";

/** 非 macOS 平台的自绘窗口三键（最小化/最大化/关闭）。
 *
 * 自带 flex 容器聚合成单个 flex 子项：AppShell 顶栏为 justify-between，
 * 若三键以 Fragment 平铺会成为多个子项被均匀摊开（三键间出现大间距）。
 */
export function WindowControls() {
  return (
    <div className="flex h-full shrink-0">
      <button
        type="button"
        aria-label="最小化"
        className={windowButtonClass}
        onClick={() => getCurrentWindow().minimize()}
      >
        <Minus className="h-4 w-4" />
      </button>
      <button
        type="button"
        aria-label="最大化"
        className={windowButtonClass}
        onClick={() => getCurrentWindow().toggleMaximize()}
      >
        <Square className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        aria-label="关闭"
        className="flex h-full w-9 items-center justify-center text-muted-foreground transition-colors hover:bg-red-600 hover:text-white"
        onClick={() => getCurrentWindow().close()}
      >
        <X className="h-4 w-4" />
      </button>
    </div>
  );
}
