'use client';

import { useState } from 'react';
import { PLATFORMS, RELEASES_PAGE, platformByKey } from '@/lib/downloads';
import type { Platform, PlatformKey } from '@/lib/downloads';
import { ChevronDownIcon, PlatformIcon } from './platform-icons';

interface DropdownOption {
  title: string;
  subtitle: string;
  url: string;
}

/** 主按钮平铺顺序：macOS、Windows、Linux（用各自平台的代表 key）。 */
const PRIMARY_KEYS: PlatformKey[] = ['macos-arm64', 'windows-x64', 'linux-x64'];

/** 每个平台按钮的下拉选项：macOS 按芯片架构区分，其余按安装包格式区分。 */
function optionsFor(platform: Platform | undefined): DropdownOption[] {
  if (!platform) return [];
  if (platform.os === 'macOS') {
    return PLATFORMS.filter((p) => p.os === 'macOS').map((p) => ({
      title: p.arch,
      subtitle: p.key === 'macos-arm64' ? 'M1/M2/M3/M4' : 'x64',
      url: p.files[0].url,
    }));
  }
  return platform.files.map((f) => ({
    title: f.label,
    subtitle: f.systems ?? f.fileName,
    url: f.url,
  }));
}

/**
 * 下载区：macOS / Windows / Linux 三个按钮横向平铺。
 *
 * - macOS   → 下拉选 Apple Silicon / Intel
 * - Windows → 点击直接下载 EXE（无下拉）
 * - Linux   → 下拉选 DEB / RPM / AppImage
 * - 同一时刻只展开一个下拉，点击菜单外关闭
 */
export function DownloadSection() {
  const [activeMenu, setActiveMenu] = useState<PlatformKey | null>(null);

  return (
    <div className="mt-10 flex flex-col items-center">
      <div className="flex flex-wrap items-center justify-center gap-3">
        {PRIMARY_KEYS.map((key) => {
          const platform = platformByKey(key);
          const isMac = key === 'macos-arm64' || key === 'macos-x64';
          const isActive = activeMenu === key;

          // Windows：点击直接下载 EXE，不展开下拉
          if (key === 'windows-x64') {
            return (
              <a
                key={key}
                href={platform?.files[0].url ?? RELEASES_PAGE}
                className="inline-flex cursor-pointer items-center gap-2.5 rounded-xl bg-fd-primary px-5 py-3 text-fd-primary-foreground shadow-xs ring-1 ring-fd-primary hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-fd-primary"
              >
                <PlatformIcon platform={key} className="size-5" />
                <span className="text-sm font-semibold">{platform?.os}</span>
              </a>
            );
          }

          return (
            <div key={key} className="relative">
              <button
                type="button"
                onClick={() => setActiveMenu(isActive ? null : key)}
                aria-expanded={isActive}
                aria-haspopup="menu"
                className="inline-flex cursor-pointer items-center gap-2.5 rounded-xl bg-fd-primary px-5 py-3 text-fd-primary-foreground shadow-xs ring-1 ring-fd-primary hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-fd-primary"
              >
                <PlatformIcon platform={key} className="size-5" />
                <span className="text-sm font-semibold">{platform?.os}</span>
                <ChevronDownIcon
                  className={`size-4 transition-transform ${isActive ? 'rotate-180' : ''}`}
                />
              </button>

              {isActive && (
                <div className="relative">
                  {/* 点击菜单外任意处关闭 */}
                  <div
                    className="fixed inset-0 z-10"
                    onClick={() => setActiveMenu(null)}
                    aria-hidden="true"
                  />
                  <div
                    role="menu"
                    className="absolute left-1/2 z-20 mt-3 w-80 -translate-x-1/2 overflow-hidden rounded-xl border border-fd-border bg-fd-card text-left shadow-lg"
                  >
                    {optionsFor(platform).map((opt) => (
                      <a
                        key={opt.url}
                        href={opt.url}
                        role="menuitem"
                        onClick={() => setActiveMenu(null)}
                        className="flex cursor-pointer items-center justify-between gap-3 px-4 py-3 transition-colors hover:bg-fd-accent hover:text-fd-accent-foreground"
                      >
                        <span className="shrink-0 text-sm font-medium text-fd-foreground">
                          {opt.title}
                        </span>
                        <span className="truncate text-xs text-fd-muted-foreground">
                          {opt.subtitle}
                        </span>
                      </a>
                    ))}
                    {isMac && (
                      <p className="border-t border-fd-border px-4 py-2.5 text-xs leading-relaxed text-fd-muted-foreground">
                        ⚠️ 未签名：首次打开提示「已损坏」？双击 dmg 内的{' '}
                        <code className="rounded bg-fd-muted px-1 py-0.5">
                          首次打开修复.command
                        </code>{' '}
                        自动安装并修复
                        <br />
                        不确定芯片？左上角  →「关于本机」看「芯片」一栏
                      </p>
                    )}
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>

      <p className="mt-4 text-center text-xs text-fd-muted-foreground">
        <a href={RELEASES_PAGE} className="text-fd-primary hover:underline">
          在 GitHub Releases 查看全部安装包 →
        </a>
      </p>
    </div>
  );
}
