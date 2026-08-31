import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DshIntegrationCard } from "./DshIntegrationCard";

const { invokeMock, listenMock, eventHandlers, openMock, toastMock } = vi.hoisted(() => {
  const handlers: Record<string, (payload: unknown) => void> = {};
  return {
    invokeMock: vi.fn(),
    listenMock: vi.fn((event: string, cb: (e: { payload: unknown }) => void) => {
      handlers[event] = (payload) => cb({ payload });
      return Promise.resolve(() => {});
    }),
    eventHandlers: handlers,
    openMock: vi.fn(),
    // 稳定的 toast 对象：`useToast()` 每次渲染返回同一引用，避免依赖 [toast]
    // 的 useCallback 变化导致 effect 反复执行。
    toastMock: { success: vi.fn(), error: vi.fn() },
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openMock }));
vi.mock("@/components/ui/toast", () => ({
  useToast: () => toastMock,
}));

function emit(event: string, payload: unknown) {
  act(() => {
    eventHandlers[event]?.(payload);
  });
}

/** get_dsh_config 返回（行为开关全开、桥运行中）。 */
const baseInfo = {
  port: 0,
  voice_enabled: true,
  llm_enabled: true,
  record_to_history: true,
  running: true,
  actual_port: 52341,
  error: null,
  discovery_path: "/tmp/dsh-bridge.json",
};

/** detect_dsh_integration 返回：全装已激活（online 前提）。 */
const fullIntegration = {
  status: {
    dsh_home_detected: true,
    profile_ready: true,
    plugin_installed: true,
    plugin_activated: true,
  },
  manual_command: "dsh plugin --profile web add @zapmomo-ai/dsh-plugin",
};

/** 按命令名分流 mock：未注册命令回 undefined（容忍 App.test 式宽松环境）。 */
function setupInvoke(handlers: Record<string, unknown>) {
  invokeMock.mockImplementation((cmd: string) => {
    if (!(cmd in handlers)) return Promise.resolve(undefined);
    return Promise.resolve(handlers[cmd]);
  });
}

/** 桥状态：心跳新鲜（1s 前）。 */
function freshStatus() {
  return { running: true, port: 52341, error: null, last_heartbeat_at: Date.now() - 1000 };
}

describe("DshIntegrationCard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeMock.mockReset();
    openMock.mockReset();
  });

  it("载入配置并在线：显示端口与在线徽章", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: fullIntegration,
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("detect_dsh_integration"));
    expect(screen.getByTestId("dsh-integration-state").textContent).toContain("在线");
    expect(screen.getByTestId("dsh-online-hint").textContent).toContain("52341");
  });

  it("未安装 → 显示一键安装按钮，点击调用 install_dsh_plugin（自动发现）", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: {
        status: {
          dsh_home_detected: true,
          profile_ready: true,
          plugin_installed: false,
          plugin_activated: false,
        },
        manual_command: "dsh plugin --profile web add @zapmomo-ai/dsh-plugin",
      },
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await screen.findByTestId("dsh-integration-state");
    expect(screen.getByTestId("dsh-integration-state").textContent).toContain("插件未安装");
    await userEvent.click(screen.getByRole("button", { name: /一键安装/ }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("install_dsh_plugin", { path: null }),
    );
  });

  it("安装失败事件 → 展示手动兜底（文件选择 + 复制命令），选中路径后代跑安装", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: {
        status: {
          dsh_home_detected: true,
          profile_ready: true,
          plugin_installed: false,
          plugin_activated: false,
        },
        manual_command: "dsh plugin --profile web add @zapmomo-ai/dsh-plugin",
      },
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await screen.findByTestId("dsh-integration-state");
    await userEvent.click(screen.getByRole("button", { name: /一键安装/ }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("install_dsh_plugin", { path: null }),
    );
    emit("dsh-install-progress", { state: "failed", message: "自动定位 dsh 失败" });
    expect(screen.getByTestId("dsh-install-progress").textContent).toContain("手动选择");
    // 文件选择器兜底：选中路径后带 path 重试
    openMock.mockResolvedValue("/usr/local/bin/dsh");
    await userEvent.click(screen.getByRole("button", { name: /选择 dsh 可执行文件/ }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("install_dsh_plugin", {
        path: "/usr/local/bin/dsh",
      }),
    );
  });

  it("半成品态（已安装未激活）→ 提示修复并提供复制命令", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: {
        status: {
          dsh_home_detected: true,
          profile_ready: true,
          plugin_installed: true,
          plugin_activated: false,
        },
        manual_command: "dsh plugin --profile web add @zapmomo-ai/dsh-plugin",
      },
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await screen.findByTestId("dsh-integration-state");
    expect(screen.getByTestId("dsh-integration-state").textContent).toContain("未激活");
    expect(screen.getByRole("button", { name: /复制修复命令/ })).toBeTruthy();
  });

  it("心跳过期（dsh 退出超 45s）→ 翻转为等待在线", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: fullIntegration,
      get_dsh_bridge_status: {
        running: true,
        port: 52341,
        error: null,
        last_heartbeat_at: Date.now() - 60_000,
      },
    });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_dsh_bridge_status"));
    expect(screen.getByTestId("dsh-integration-state").textContent).toContain("等待在线");
  });

  it("检测到 dsh 但未初始化 profile → 引导先跑一次 dsh web", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: {
        status: {
          dsh_home_detected: true,
          profile_ready: false,
          plugin_installed: false,
          plugin_activated: false,
        },
        manual_command: "dsh plugin --profile web add @zapmomo-ai/dsh-plugin",
      },
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await screen.findByTestId("dsh-integration-state");
    expect(screen.getByTestId("dsh-integration-state").textContent).toContain("未初始化");
  });

  it("detect 返回 undefined（宽松宿主）不崩溃，按未检测到 dsh 展示", async () => {
    setupInvoke({ get_dsh_config: baseInfo });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("detect_dsh_integration"));
    expect(screen.getByTestId("dsh-integration-state").textContent).toContain("未检测到 dsh");
  });

  it("已安装 → 显示卸载按钮，点击调用 uninstall_dsh_plugin", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: fullIntegration,
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_dsh_config"));
    await userEvent.click(screen.getByRole("button", { name: /卸载/ }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("uninstall_dsh_plugin"));
  });

  it("未安装 → 无卸载按钮，行为开关与测试播报禁用（桥跟随安装状态）", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: {
        status: {
          dsh_home_detected: true,
          profile_ready: true,
          plugin_installed: false,
          plugin_activated: false,
        },
        manual_command: "dsh plugin --profile web add @zapmomo-ai/dsh-plugin",
      },
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await screen.findByTestId("dsh-integration-state");
    expect(screen.queryByRole("button", { name: /卸载/ })).toBeNull();
    expect(screen.getByRole("switch", { name: "LLM 播报文案" })).toBeDisabled();
    expect(screen.getByRole("button", { name: /测试播报/ })).toBeDisabled();
  });

  it("卸载失败事件 → 展示后端消息，不出安装重试按钮", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: fullIntegration,
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_dsh_config"));
    await userEvent.click(screen.getByRole("button", { name: /卸载/ }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("uninstall_dsh_plugin"));
    emit("dsh-install-progress", {
      state: "failed",
      message: "自动定位 dsh 失败，可在终端执行手动卸载命令。",
    });
    expect(screen.getByTestId("dsh-install-progress").textContent).toContain(
      "可在终端执行手动卸载命令",
    );
    expect(screen.queryByRole("button", { name: /选择 dsh 可执行文件/ })).toBeNull();
  });

  it("测试播报按钮调用 test_dsh_announce", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: fullIntegration,
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_dsh_config"));
    await userEvent.click(screen.getByRole("button", { name: /测试播报/ }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("test_dsh_announce"));
  });

  it("LLM 播报开关调用 set_dsh_params", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: fullIntegration,
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_dsh_config"));
    const toggle = screen.getByRole("switch", { name: "LLM 播报文案" });
    await userEvent.click(toggle);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_dsh_params", {
        params: { llm_enabled: false },
      }),
    );
  });

  it("桥错误经 status 事件渲染", async () => {
    setupInvoke({
      get_dsh_config: baseInfo,
      detect_dsh_integration: fullIntegration,
      get_dsh_bridge_status: freshStatus(),
    });
    render(<DshIntegrationCard />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_dsh_config"));
    emit("dsh-bridge-status", {
      running: false,
      port: null,
      error: "绑定 127.0.0.1:47800 失败",
      last_heartbeat_at: null,
    });
    expect(screen.getByTestId("dsh-bridge-error").textContent).toContain(
      "绑定 127.0.0.1:47800 失败",
    );
  });
});
