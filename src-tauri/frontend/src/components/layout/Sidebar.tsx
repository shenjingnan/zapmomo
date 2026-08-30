import { Home, Layers, MessageCircle, Puzzle, Settings, Users } from "lucide-react";
import { isMacOs } from "@/lib/platform";
import { NavItem } from "./NavItem";

const PRIMARY_NAV = [
  { to: "/home", icon: Home, label: "概览", end: true },
  { to: "/chat", icon: MessageCircle, label: "对话记录", end: true },
  { to: "/companion", icon: Users, label: "伙伴" },
  { to: "/models", icon: Layers, label: "模型" },
  { to: "/integrations", icon: Puzzle, label: "插件集成" },
  { to: "/settings", icon: Settings, label: "设置", end: true },
];

/** 左侧导航：顶部窗口按钮区 + 真实 logo + 主导航。 */
export function Sidebar() {
  const mac = isMacOs();

  return (
    <aside
      data-tauri-drag-region="deep"
      className="flex w-[248px] shrink-0 flex-col bg-sidebar-background"
    >
      {/* 顶部条：macOS 系统红绿灯留白；其余平台纯留白
          （三键由 AppShell 右上角悬浮条承担）。 */}
      <div
        className="flex h-8 shrink-0 items-center pl-3"
        style={mac ? { paddingLeft: "78px" } : undefined}
      />

      <div className="flex items-center justify-center px-6 pt-1">
        <img src="/logo.svg" alt="ZapMomo" className="h-24 w-24" />
      </div>

      <nav className="mt-5 flex flex-col gap-1.5 px-4">
        {PRIMARY_NAV.map((item) => (
          <NavItem key={item.to} to={item.to} icon={item.icon} label={item.label} end={item.end} />
        ))}
      </nav>
    </aside>
  );
}
