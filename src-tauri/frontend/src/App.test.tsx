import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";

const { invokeMock, listeners } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listeners: new Map<string, (e: { payload: unknown }) => void>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: (e: { payload: unknown }) => void) => {
    listeners.set(event, handler);
    return Promise.resolve(() => {});
  }),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    close: vi.fn(),
  })),
}));

const DEFAULT_CONFIG = {
  enabled: false,
  custom_keywords: "",
  model_dir: "/home/user/.zapmomo/models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20",
  provider: "cpu",
  num_threads: 4,
  sample_rate: 16000,
  chunk_size: 3200,
  keywords_score: 1.0,
  keywords_threshold: 0.25,
  debug: false,
  keywords: ["文森特卡索"],
  models_present: false,
  model_downloading: false,
  settings_path: "/home/user/.zapmomo/settings.toml",
};

/** 可变 KWS 配置：单个用例可翻转 models_present 等字段（贴近真实后端）。 */
let kwsConfig: typeof DEFAULT_CONFIG;
/** 模拟后端持久化的麦克风（get_microphone / set_microphone）。 */
let mic = "";
/** 模拟后端可枚举的输入设备（置空以测试 macOS 未授权场景）。 */
let devices: string[];
/** 模拟 KWS 监听运行状态（置 true 以验证监听中仍可切换设备）。 */
let kwsListening = false;
/** 可变 LLM 配置：单个用例可置 ready/base_url+model 以开启能力链路开关。 */
let llmConfig: typeof LLM_CONFIG;
/** 模拟后端拒绝卸载 LLM（如语音会话占用），null = 卸载成功。 */
let llmUnloadReject: string | null = null;

const ASR_CONFIG = {
  model_dir: "/home/user/.zapmomo/models/sherpa-onnx-streaming-zipformer",
  provider: "cpu",
  num_threads: 4,
  sample_rate: 16000,
  models_present: false,
  punctuation_present: false,
  model_downloading: false,
  settings_path: "/home/user/.zapmomo/settings.toml",
};

const TTS_CONFIG = {
  model_dir: "/home/user/.zapmomo/models/sherpa-onnx-zipvoice",
  provider: "cpu",
  num_threads: 4,
  enabled: true,
  models_present: false,
  model_downloading: false,
  settings_path: "/home/user/.zapmomo/settings.toml",
};

/** 可变 ASR/TTS 配置（引导卡「全部正常不渲染」用例需置 models_present）。 */
let asrConfig: typeof ASR_CONFIG;
let ttsConfig: typeof TTS_CONFIG;

const LLM_CONFIG = {
  enabled: false,
  provider: "openai-compatible",
  ready: false,
  settings_path: "/home/user/.zapmomo/settings.toml",
  system_prompt: "你是 ZapMomo 的 AI 大脑。",
  base_url: null as string | null,
  api_key: null as string | null,
  model: null as string | null,
  params: {
    max_tokens: 512,
    temperature: 0.7,
    top_p: 0.8,
    top_k: 20,
    min_p: 0.05,
    repeat_penalty: 1.05,
    seed: 0,
  },
};

/** 把 LLM 配置标记为「已填写远程连接三要素」（等价旧 models_present=true）。 */
function llmConfigured(cfg: typeof LLM_CONFIG): typeof LLM_CONFIG {
  return { ...cfg, base_url: "https://open.bigmodel.cn/api/paas/v4", model: "glm-4.7-flash" };
}

/** 渲染 App 并定位到指定路由（默认 KWS 详情页）。 */
function renderApp(initialPath = "/models/kws") {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <App />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  kwsConfig = { ...DEFAULT_CONFIG };
  llmConfig = { ...LLM_CONFIG };
  asrConfig = { ...ASR_CONFIG };
  ttsConfig = { ...TTS_CONFIG };
  llmUnloadReject = null;
  mic = "";
  devices = ["内置麦克风", "USB 麦克风"];
  kwsListening = false;

  invokeMock.mockImplementation(
    (
      cmd: string,
      args?: {
        enabled?: boolean;
        mic?: string;
        keywords?: string;
        params?: Partial<typeof DEFAULT_CONFIG>;
        id?: string;
      },
    ) => {
      switch (cmd) {
        case "get_app_info":
          return Promise.resolve({ version: "0.1.4", product_name: "ZapMomo" });
        case "list_devices":
          return Promise.resolve(devices);
        case "request_mic_permission":
          return Promise.resolve(true);
        case "get_kws_config":
          return Promise.resolve({ ...kwsConfig });
        case "set_kws_enabled":
          kwsConfig = { ...kwsConfig, enabled: args?.enabled ?? false };
          return Promise.resolve(undefined);
        case "set_kws_custom_keywords":
          kwsConfig = { ...kwsConfig, custom_keywords: args?.keywords ?? "" };
          return Promise.resolve(undefined);
        case "set_kws_params":
          kwsConfig = { ...kwsConfig, ...(args?.params ?? {}) };
          return Promise.resolve(undefined);
        case "get_microphone":
          return Promise.resolve(mic);
        case "set_microphone":
          mic = args?.mic ?? "";
          return Promise.resolve(undefined);
        case "is_listening":
          return Promise.resolve(kwsListening);
        case "get_asr_config":
          return Promise.resolve({ ...asrConfig });
        case "get_tts_config":
          return Promise.resolve({ ...ttsConfig });
        case "list_tts_voices":
          return Promise.resolve([]);
        case "get_llm_config":
          return Promise.resolve(llmConfig);
        case "unload_llm_model":
          return llmUnloadReject ? Promise.reject(llmUnloadReject) : Promise.resolve(undefined);
        case "is_asr_listening":
          return Promise.resolve(false);
        case "is_llm_ready":
          return Promise.resolve(false);
        case "list_model_library":
          return Promise.resolve([]);
        case "start_listen":
        case "stop_listen":
        case "download_kws_model":
          return Promise.resolve(undefined);
        default:
          return Promise.resolve(undefined);
      }
    },
  );
});

describe("App（KWS 控制面板）", () => {
  it("渲染 Sidebar 导航与模型概览页（模型摘要）", async () => {
    renderApp("/models");
    expect(screen.getByAltText("ZapMomo")).toBeInTheDocument();
    expect(screen.getByText("概览")).toBeInTheDocument();
    expect(screen.getByText("模型摘要")).toBeInTheDocument();
    expect(screen.getByText("管理模型")).toBeInTheDocument();
  });

  it("概览页 ASR 开关调用 start_asr_listen", async () => {
    const user = userEvent.setup();
    renderApp("/models");

    await user.click(await screen.findByRole("switch", { name: "语音识别（ASR）开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
  });

  it("点击唤醒词开关直接调用 start_listen（不再联动 ASR）", async () => {
    const user = userEvent.setup();
    renderApp("/models");

    const kwsSwitch = await screen.findByRole("switch", { name: "唤醒词（KWS）开关" });
    expect(kwsSwitch).not.toBeDisabled();

    await user.click(kwsSwitch);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", { device: null, keywords: null });
    });
    // 不联动 ASR：不弹确认框、不启动 ASR
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("start_asr_listen", expect.anything());
  });

  it("概览页语音合成开关调用 set_tts_enabled", async () => {
    const user = userEvent.setup();
    renderApp("/models");

    await user.click(await screen.findByRole("switch", { name: "语音合成（TTS）开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_tts_enabled", { enabled: false });
    });
  });

  it("LLM 卸载失败：右上角通知展示真实原因（如语音会话占用）", async () => {
    llmConfig = { ...llmConfigured(llmConfig), ready: true };
    llmUnloadReject = "语音会话正在使用 LLM。请先在「对话记录」页停止会话后再卸载。";
    const user = userEvent.setup();
    renderApp("/models");

    // 开启的 LLM 开关（toggled 绑 ready），点击触发 unload
    await user.click(await screen.findByRole("switch", { name: "AI 大脑（LLM）开关" }));

    // 真实错误经右上角 Toast 透出（而非仅红色「错误」）
    expect(
      await screen.findByText("语音会话正在使用 LLM。请先在「对话记录」页停止会话后再卸载。"),
    ).toBeInTheDocument();
  });

  it("概览页引导卡：默认全未配置 → 去模型库", async () => {
    renderApp("/models");
    expect(await screen.findByText("4 项能力尚未配置模型")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "去模型库" })).toHaveAttribute(
      "href",
      "/models/library",
    );
  });

  it("概览页引导卡：全部配置正常时不渲染", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    asrConfig = { ...asrConfig, models_present: true };
    ttsConfig = { ...ttsConfig, models_present: true };
    llmConfig = llmConfigured(llmConfig);
    renderApp("/models");
    // 等待配置加载完成（「未配置模型」span 消失）后再断言无引导卡。
    await waitFor(() => {
      expect(screen.queryByText("尚未配置模型")).not.toBeInTheDocument();
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("LLM 已配置：摘要行展示远程模型名，状态为未连接", async () => {
    llmConfig = llmConfigured(llmConfig);
    renderApp("/models");

    // LLM 行模型名区域回显远程模型名（等 llm config 异步加载完成）
    expect(await screen.findByText("glm-4.7-flash")).toBeInTheDocument();
    // 未连接时行内开关可用（点击即连接）
    const llmRow = screen.getByRole("link", { name: "配置AI 大脑（LLM）" });
    expect(within(llmRow).getByText("未连接", { selector: "span" })).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "AI 大脑（LLM）开关" })).toBeEnabled();
  });

  it("渲染 KWS 配置项", async () => {
    const user = userEvent.setup();
    renderApp();
    // 基础配置显示当前模型 basename + 未下载 Badge
    expect(
      await screen.findByText("sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20"),
    ).toBeInTheDocument();
    expect(screen.getByText("未下载")).toBeInTheDocument();
    // 模型信息默认折叠；展开后显示只读字段
    await user.click(screen.getByRole("button", { name: /模型信息/ }));
    expect(await screen.findByText("推理后端")).toBeInTheDocument();
    expect(screen.getByText("cpu")).toBeInTheDocument();
    expect(screen.getByText("16000")).toBeInTheDocument();
    expect(screen.getByText("文森特卡索")).toBeInTheDocument();
  });

  it("模型缺失时显示警告与下载按钮", async () => {
    renderApp();
    expect(await screen.findByText("模型文件缺失")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /下载模型/ })).toBeInTheDocument();
  });

  it("点击顶部开关开启监听：持久化 enabled 并调用 start_listen，开关置 ON", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderApp();

    await user.click(await screen.findByRole("switch", { name: "唤醒词监听开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_kws_enabled", { enabled: true });
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", {
        device: null,
        keywords: null,
      });
    });
    // 开关绑持久化 enabled：set_kws_enabled 回读后置 ON
    await waitFor(() => {
      expect(screen.getByRole("switch", { name: "唤醒词监听开关" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
    });
  });

  it("点击顶部开关关闭监听：停止监听并持久化 enabled=false", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderApp();

    const sw = await screen.findByRole("switch", { name: "唤醒词监听开关" });
    await user.click(sw);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", {
        device: null,
        keywords: null,
      });
    });

    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_listen");
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_kws_enabled", { enabled: false });
    });
  });

  it("检测到唤醒词后把结果追加到测试对话框", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderApp();

    await user.click(await screen.findByRole("button", { name: /测试唤醒词/ }));
    expect(await screen.findByText("尚未检测到唤醒词")).toBeInTheDocument();

    act(() => {
      listeners.get("kws-detected")?.({
        payload: {
          keyword: "文森特卡索",
          tokens: "",
          tokens_arr: [],
          timestamps: [],
          start_time: 0.64,
          json: "{}",
        },
      });
    });

    expect(await screen.findByText("“文森特卡索”")).toBeInTheDocument();
  });

  it("点击下载模型调用 download_kws_model 并刷新配置", async () => {
    const user = userEvent.setup();
    renderApp();
    const button = await screen.findByRole("button", { name: /下载模型/ });

    await user.click(button);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("download_kws_model");
    });
    // 下载完成后会重新拉取配置（models_present 变 true 后按钮消失）
    await waitFor(() => {
      const calls = invokeMock.mock.calls.map((c) => c[0]);
      expect(calls.filter((c) => c === "get_kws_config").length).toBeGreaterThanOrEqual(2);
    });
  });

  it("设置页可切换是否隐藏 Dock / Cmd+Tab 图标", async () => {
    const user = userEvent.setup();
    renderApp("/settings");

    const toggle = await screen.findByRole("switch", { name: "隐藏应用图标" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await user.click(toggle);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_hide_dock_icon", { hide: true });
    });
  });

  it("设置页可选择麦克风并持久化到后端", async () => {
    const user = userEvent.setup();
    renderApp("/settings");

    await user.click(await screen.findByRole("combobox", { name: "麦克风来源" }));
    await user.click(await screen.findByRole("option", { name: "USB 麦克风" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_microphone", { mic: "USB 麦克风" });
    });
  });

  it("设置页刷新设备按钮重新调用 list_devices", async () => {
    const user = userEvent.setup();
    renderApp("/settings");

    await user.click(await screen.findByRole("button", { name: "刷新设备列表" }));

    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter((c) => c[0] === "list_devices");
      expect(calls.length).toBeGreaterThanOrEqual(2);
    });
  });

  it("设置页无设备时（macOS 未授权）显示授权按钮并触发权限请求", async () => {
    const user = userEvent.setup();
    devices = [];
    // 模拟 macOS WebView 的 userAgent（授权按钮仅 macOS 显示）
    const uaDesc = Object.getOwnPropertyDescriptor(navigator, "userAgent");
    Object.defineProperty(navigator, "userAgent", {
      value: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
      configurable: true,
    });
    try {
      renderApp("/settings");

      const grantBtn = await screen.findByRole("button", { name: "授权麦克风" });
      expect(screen.getByRole("combobox", { name: "麦克风来源" })).toBeDisabled();

      await user.click(grantBtn);

      await waitFor(() => {
        expect(invokeMock).toHaveBeenCalledWith("request_mic_permission");
      });
      // 授权后重新拉取设备列表
      await waitFor(() => {
        const calls = invokeMock.mock.calls.filter((c) => c[0] === "list_devices");
        expect(calls.length).toBeGreaterThanOrEqual(2);
      });
    } finally {
      if (uaDesc) Object.defineProperty(navigator, "userAgent", uaDesc);
    }
  });

  it("KWS 监听中仍可切换麦克风来源（后端自动重启监听）", async () => {
    const user = userEvent.setup();
    kwsListening = true;
    renderApp("/settings");

    // 监听运行中下拉不再被禁用（切换后由后端用新设备重启监听）
    const combobox = await screen.findByRole("combobox", { name: "麦克风来源" });
    expect(combobox).not.toBeDisabled();

    await user.click(combobox);
    await user.click(await screen.findByRole("option", { name: "USB 麦克风" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_microphone", { mic: "USB 麦克风" });
    });
  });

  it("设置页点击「重启」按钮调用 restart_app", async () => {
    const user = userEvent.setup();
    renderApp("/settings");

    await user.click(await screen.findByRole("button", { name: "重启" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("restart_app");
    });
  });
});
