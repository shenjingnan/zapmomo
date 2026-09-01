/**
 * 平台下载数据与 UA 检测逻辑（纯 TS，无 DOM，可被 Server/Client/测试三方引用）。
 *
 * 直链与根 README「下载桌面应用」一致：全部指向 GitHub Releases 的 latest
 * 下载目录，无需登录 GitHub、自动跟随最新版本。
 *
 * 注意：Linux deb 是 `amd64`、rpm 是 `x86_64`，命名不一致，勿统一替换。
 */

export type PlatformKey =
  | 'windows-x64'
  | 'macos-arm64'
  | 'macos-x64'
  | 'linux-x64'
  | 'unknown';

export interface DownloadFile {
  label: string;
  fileName: string;
  url: string;
  /** 适用系统 / 格式说明（如 Linux 各发行版），用于下拉菜单副标题。 */
  systems?: string;
}

export interface Platform {
  key: PlatformKey;
  os: string;
  arch: string;
  files: DownloadFile[];
  note?: string;
}

export const RELEASE_BASE =
  'https://github.com/shenjingnan/zapmomo/releases/latest/download/';

/** Releases 页面兜底入口（UA 识别失败 / 移动端时使用）。 */
export const RELEASES_PAGE = 'https://github.com/shenjingnan/zapmomo/releases';

/** 全部可下载平台（files[0] 为该平台默认下载）。 */
export const PLATFORMS: Platform[] = [
  {
    key: 'windows-x64',
    os: 'Windows',
    arch: 'x64',
    files: [
      {
        label: 'EXE',
        fileName: 'ZapMomo_Windows_x64.exe',
        url: `${RELEASE_BASE}ZapMomo_Windows_x64.exe`,
      },
      {
        label: 'MSI',
        fileName: 'ZapMomo_Windows_x64.msi',
        url: `${RELEASE_BASE}ZapMomo_Windows_x64.msi`,
      },
    ],
  },
  {
    key: 'macos-arm64',
    os: 'macOS',
    arch: 'Apple Silicon',
    files: [
      {
        label: 'DMG',
        fileName: 'ZapMomo_macOS_arm64.dmg',
        url: `${RELEASE_BASE}ZapMomo_macOS_arm64.dmg`,
      },
    ],
    note: '未签名：首次打开提示「已损坏」？双击 dmg 内「首次打开修复.command」自动安装并修复',
  },
  {
    key: 'macos-x64',
    os: 'macOS',
    arch: 'Intel',
    files: [
      {
        label: 'DMG',
        fileName: 'ZapMomo_macOS_x64.dmg',
        url: `${RELEASE_BASE}ZapMomo_macOS_x64.dmg`,
      },
    ],
    note: '未签名：首次打开提示「已损坏」？双击 dmg 内「首次打开修复.command」自动安装并修复',
  },
  {
    key: 'linux-x64',
    os: 'Linux',
    arch: 'x86_64',
    files: [
      {
        label: 'DEB',
        fileName: 'ZapMomo_Linux_amd64.deb',
        url: `${RELEASE_BASE}ZapMomo_Linux_amd64.deb`,
        systems: 'Debian / Ubuntu 等',
      },
      {
        label: 'RPM',
        fileName: 'ZapMomo_Linux_x86_64.rpm',
        url: `${RELEASE_BASE}ZapMomo_Linux_x86_64.rpm`,
        systems: 'Fedora / RHEL / openSUSE 等',
      },
      {
        label: 'AppImage',
        fileName: 'ZapMomo_Linux_amd64.AppImage',
        url: `${RELEASE_BASE}ZapMomo_Linux_amd64.AppImage`,
        systems: '所有发行版通用（免安装）',
      },
    ],
  },
];

export function platformByKey(key: PlatformKey): Platform | undefined {
  return PLATFORMS.find((p) => p.key === key);
}

export interface DetectInput {
  /** navigator.userAgent */
  ua: string;
  /** navigator.userAgentData?.platform（低熵，Chromium 系）或 navigator.platform */
  platform?: string;
  /** navigator.userAgentData?.architecture（'arm' | 'x86' | 'unknown'） */
  arch?: string;
}

/**
 * 根据浏览器 UA 信息判定下载平台。纯函数、无副作用，可单测。
 *
 * macOS 的 Apple Silicon / Intel 区分依赖 `userAgentData.architecture`
 * （Chromium 系低熵属性）；UA 字符串里的 `Intel Mac OS X` 不可靠（Apple
 * Silicon 上的浏览器为兼容也上报该值），因此禁止据此判 Intel。arch 缺失
 * （Safari/Firefox/隐私模式）默认返回 arm64——2026 年 Mac 已基本是 Apple
 * Silicon，且展开区会醒目列出 macos-x64 供手动选择。
 */
export function detectPlatform(input: DetectInput): PlatformKey {
  const ua = (input.ua ?? '').toLowerCase();
  const platform = (input.platform ?? '').toLowerCase();
  const arch = (input.arch ?? '').toLowerCase();

  // 移动端不提供桌面安装包
  if (/android|iphone|ipad|ipod/.test(ua)) return 'unknown';

  // userAgentData.platform 优先（低熵、最准确）
  if (platform === 'windows') return 'windows-x64';
  if (platform === 'linux') return 'linux-x64';
  if (platform === 'macos' || platform === 'macintel') return resolveMacArch(arch);

  // 无 userAgentData 时的 UA 字符串回退
  if (/windows/.test(ua)) return 'windows-x64';
  if (/linux/.test(ua)) return 'linux-x64';
  if (/mac os x|macintosh/.test(ua)) return resolveMacArch(arch);

  return 'unknown';
}

/** x86 → Intel；arm 或缺失 → Apple Silicon（安全默认）。 */
function resolveMacArch(arch: string): PlatformKey {
  if (arch === 'x86') return 'macos-x64';
  return 'macos-arm64';
}
