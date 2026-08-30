import { describe, expect, it } from "vitest";
import { composeIntegrationState, ONLINE_WINDOW_MS } from "@/lib/dshIntegration";
import type { DshIntegrationInfo } from "@/types/tauri";

/** 造检测状态：缺省全 false，传入覆盖。 */
function makeInfo(overrides: Partial<DshIntegrationInfo["status"]> = {}): DshIntegrationInfo {
  return {
    status: {
      dsh_home_detected: true,
      profile_ready: true,
      plugin_installed: true,
      plugin_activated: true,
      ...overrides,
    },
    manual_command: "dsh plugin --profile web add @zapmomo-ai/dsh-plugin",
  };
}

describe("composeIntegrationState", () => {
  const NOW = 1_000_000;

  it("null/undefined 信息按未检测到 dsh 处理（App.test 默认 mock 返回 undefined 不崩）", () => {
    expect(composeIntegrationState(null, true, NOW, NOW)).toBe("no-dsh");
    expect(composeIntegrationState(undefined, true, NOW, NOW)).toBe("no-dsh");
  });

  it("无 dsh 主目录 → no-dsh", () => {
    const info = makeInfo({
      dsh_home_detected: false,
      profile_ready: false,
      plugin_installed: false,
      plugin_activated: false,
    });
    expect(composeIntegrationState(info, true, NOW, NOW)).toBe("no-dsh");
  });

  it("有 dsh 但无 web profile → no-profile", () => {
    const info = makeInfo({
      profile_ready: false,
      plugin_installed: false,
      plugin_activated: false,
    });
    expect(composeIntegrationState(info, true, NOW, NOW)).toBe("no-profile");
  });

  it("profile 就绪但插件未装 → not-installed", () => {
    const info = makeInfo({ plugin_installed: false, plugin_activated: false });
    expect(composeIntegrationState(info, true, NOW, NOW)).toBe("not-installed");
  });

  it("已安装未激活（半成品）→ half-activated", () => {
    const info = makeInfo({ plugin_activated: false });
    expect(composeIntegrationState(info, true, NOW, NOW)).toBe("half-activated");
  });

  it("已激活且心跳新鲜（桥运行中）→ online", () => {
    expect(composeIntegrationState(makeInfo(), true, NOW - 1000, NOW)).toBe("online");
    // 窗口边界内（45s - 1ms）仍在线
    expect(composeIntegrationState(makeInfo(), true, NOW - ONLINE_WINDOW_MS + 1, NOW)).toBe(
      "online",
    );
  });

  it("心跳超窗 → awaiting-restart（dsh 退出 45s 内翻转）", () => {
    expect(composeIntegrationState(makeInfo(), true, NOW - ONLINE_WINDOW_MS, NOW)).toBe(
      "awaiting-restart",
    );
  });

  it("桥未运行时 online 不可达 → awaiting-restart", () => {
    expect(composeIntegrationState(makeInfo(), false, NOW - 1000, NOW)).toBe("awaiting-restart");
  });

  it("无心跳 → awaiting-restart（已激活但插件从未连上桥）", () => {
    expect(composeIntegrationState(makeInfo(), true, null, NOW)).toBe("awaiting-restart");
  });

  it("心跳时间戳来自未来（时钟回拨）按过期算", () => {
    expect(composeIntegrationState(makeInfo(), true, NOW + 60_000, NOW)).toBe("awaiting-restart");
  });
});
