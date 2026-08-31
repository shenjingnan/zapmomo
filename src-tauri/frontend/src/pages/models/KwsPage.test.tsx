import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";

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

const KWS_CONFIG = {
  enabled: false,
  custom_keywords: "",
  model_dir: "/home/user/.zapmomo/models/sherpa-onnx-kws-zipformer-zh-en-3M",
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

const LLM_CONFIG = {
  enabled: false,
  provider: "local",
  model_path: "/home/user/.zapmomo/models/qwen3-4b.gguf",
  models_present: false,
  ready: false,
  enable_thinking: false,
  auto_load: false,
  settings_path: "/home/user/.zapmomo/settings.toml",
  system_prompt: "你是 ZapMomo 的 AI 大脑。",
  params: {
    context_size: 8192,
    batch_size: 512,
    max_tokens: 512,
    temperature: 0.7,
    top_p: 0.8,
    top_k: 20,
    min_p: 0.05,
    repeat_penalty: 1.05,
    seed: 0,
    threads: 8,
    gpu_layers: 0,
    enable_thinking: false,
  },
};

/** 可变 KWS 配置：单个用例可翻转 models_present 等字段（贴近真实后端）。 */
let kwsConfig: typeof KWS_CONFIG;

/** 模拟后端持久化的麦克风（get_microphone / set_microphone）。 */
let mic = "";

/** 模型库列表桩的最小形状（list_model_library 返回条目的测试子集）。 */
type LibraryStub = {
  id: string;
  displayName: string;
  modelType: string;
  installState: string;
  current: boolean;
  localPath: string | null;
  installId: string | null;
  repoId: string | null;
  ownership: string;
};

/** 默认模型库桩：唯一内置 KWS registry 条目（zh-en 已装为当前）。 */
function defaultModelLibrary(): LibraryStub[] {
  return [
    {
      id: "kws-zipformer-zh-en-3m",
      displayName: "Zipformer KWS zh-en 3M",
      modelType: "kws",
      installState: "installed",
      current: true,
      localPath: "/home/user/.zapmomo/models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20",
      installId: "kws-zipformer-zh-en-3m",
      repoId: null,
      ownership: "managed",
    },
  ];
}

let modelLibrary: LibraryStub[] = defaultModelLibrary();

/** 默认 command 桩：非 KWS 测试用例直接复用。 */
function defaultInvoke(
  cmd: string,
  args?: {
    enabled?: boolean;
    mic?: string;
    keywords?: string;
    params?: Partial<typeof KWS_CONFIG>;
  },
) {
  switch (cmd) {
    case "get_app_info":
      return Promise.resolve({ version: "0.1.4", product_name: "ZapMomo" });
    case "list_devices":
      return Promise.resolve(["内置麦克风", "USB 麦克风"]);
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
      return Promise.resolve(false);
    case "list_model_library":
      return Promise.resolve(modelLibrary);
    case "set_current_model":
      return Promise.resolve({
        modelType: "kws",
        modelId: "kws-zipformer-zh-en-3m",
        path: "/home/user/.zapmomo/models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20",
        runtimeAction: "restart_required",
        effectiveImmediately: false,
        message: "已将 Zipformer KWS zh-en 3M 设为 KWS 当前模型，将在下次启动监听时生效",
      });
    case "get_asr_config":
      return Promise.resolve({ ...ASR_CONFIG });
    case "get_tts_config":
      return Promise.resolve({ ...TTS_CONFIG });
    case "list_tts_voices":
      return Promise.resolve([]);
    case "get_llm_config":
      return Promise.resolve({ ...LLM_CONFIG });
    case "is_asr_listening":
      return Promise.resolve(false);
    case "is_llm_ready":
      return Promise.resolve(false);
    default:
      return Promise.resolve(undefined);
  }
}

function renderKwsPage() {
  return render(
    <MemoryRouter initialEntries={["/models/kws"]}>
      <App />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  kwsConfig = { ...KWS_CONFIG };
  mic = "";
  modelLibrary = defaultModelLibrary();
  invokeMock.mockImplementation(defaultInvoke);
});

describe("KwsPage（唤醒词配置）", () => {
  it("未下载模型：开关禁用、状态「未启用」、显示模型名与「未下载」Badge", async () => {
    renderKwsPage();
    expect(await screen.findByText("唤醒词（KWS）配置")).toBeInTheDocument();
    expect(screen.getByText("sherpa-onnx-kws-zipformer-zh-en-3M")).toBeInTheDocument();
    expect(screen.getByText("未启用")).toBeInTheDocument();
    expect(screen.getByText("未下载")).toBeInTheDocument();

    const runSwitch = screen.getByRole("switch", { name: "唤醒词监听开关" });
    expect(runSwitch).toBeDisabled();
    expect(runSwitch).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("button", { name: "下载模型" })).toBeEnabled();
  });

  it("模型就绪：状态「未启用」、显示「已就绪」Badge、开关可用、无下载按钮", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    renderKwsPage();
    expect(await screen.findByText("未启用")).toBeInTheDocument();
    expect(screen.getByText("已就绪")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "唤醒词监听开关" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
  });

  it("顶部开关 ON 携带麦克风调用 start_listen 并持久化 enabled", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    // 选择麦克风
    await user.click(screen.getByRole("combobox", { name: "麦克风来源" }));
    await user.click(await screen.findByRole("option", { name: "内置麦克风" }));

    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_kws_enabled", { enabled: true });
    });
    await waitFor(() => {
      // 全局自定义唤醒词输入已移除（唤醒词由伙伴页按角色设置），此处始终传 null
      expect(invokeMock).toHaveBeenCalledWith("start_listen", {
        device: "内置麦克风",
        keywords: null,
      });
    });
  });

  it("从后端记忆恢复麦克风：顶部开关直接使用记忆的设备", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    mic = "内置麦克风";
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", {
        device: "内置麦克风",
        keywords: null,
      });
    });
  });

  it("顶部开关 OFF 调用 stop_listen 并持久化 enabled=false", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", { device: null, keywords: null });
    });

    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_listen");
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_kws_enabled", { enabled: false });
    });
  });

  it("start/stop 在途（pending）时顶部开关禁用，完成后恢复", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    let resolveStart!: () => void;
    const deferred = new Promise<void>((res) => {
      resolveStart = res;
    });
    invokeMock.mockImplementation(
      (cmd: string, args?: { enabled?: boolean; mic?: string; keywords?: string }) =>
        cmd === "start_listen" ? deferred : defaultInvoke(cmd, args),
    );

    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));

    // 在途：isListening 未落盘，但开关禁用防重复
    expect(screen.getByRole("switch", { name: "唤醒词监听开关" })).toBeDisabled();

    await act(async () => {
      resolveStart();
    });
    await waitFor(() => {
      const sw = screen.getByRole("switch", { name: "唤醒词监听开关" });
      expect(sw).toHaveAttribute("aria-checked", "true");
      expect(sw).toBeEnabled();
    });
  });

  it("刷新设备按钮重新调用 list_devices", async () => {
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("sherpa-onnx-kws-zipformer-zh-en-3M");

    await user.click(screen.getByRole("button", { name: "刷新设备列表" }));

    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter((c) => c[0] === "list_devices");
      expect(calls.length).toBeGreaterThanOrEqual(2);
    });
  });

  it("模型信息默认折叠，点击展开显示只读字段", async () => {
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("sherpa-onnx-kws-zipformer-zh-en-3M");

    const trigger = screen.getByRole("button", { name: /模型信息/ });
    expect(trigger).toHaveAttribute("aria-expanded", "false");

    await user.click(trigger);
    expect(screen.getByRole("button", { name: /模型信息/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByText("推理后端")).toBeInTheDocument();
    expect(screen.getByText("采样率")).toBeInTheDocument();
    expect(screen.getByText("文森特卡索")).toBeInTheDocument();
  });

  it("高级参数：默认折叠，修改灵敏度保存调用 set_kws_params", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    // 展开高级参数
    const trigger = screen.getByRole("button", { name: /高级参数/ });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    await user.click(trigger);
    expect(screen.getByRole("button", { name: /高级参数/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    // 回显解析后的参数
    const thresholdInput = screen.getByRole("textbox", { name: "灵敏度 / 阈值" });
    await waitFor(() => {
      expect(thresholdInput).toHaveValue("0.25");
    });

    // 修改灵敏度并保存
    await user.clear(thresholdInput);
    await user.type(thresholdInput, "0.5");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "set_kws_params",
        expect.objectContaining({ params: expect.objectContaining({ keywords_threshold: 0.5 }) }),
      );
    });
  });

  it("高级参数：正在监听时保存参数会重启监听使改动生效", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    // 开启监听
    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", { device: null, keywords: null });
    });
    invokeMock.mockClear();

    // 展开高级参数并修改灵敏度保存
    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const thresholdInput = screen.getByRole("textbox", { name: "灵敏度 / 阈值" });
    await waitFor(() => {
      expect(thresholdInput).toHaveValue("0.25");
    });
    await user.clear(thresholdInput);
    await user.type(thresholdInput, "0.5");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_kws_params", expect.anything());
    });
    // 保存后重启监听使引擎参数生效
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_listen");
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", { device: null, keywords: null });
    });
  });

  it("模型下载进度事件显示进度消息", async () => {
    renderKwsPage();
    await screen.findByRole("button", { name: /下载模型/ });

    act(() => {
      listeners.get("kws-model-download-progress")?.({
        payload: { stage: "downloading", percent: 42, message: "下载中 42%" },
      });
    });

    expect(await screen.findByText("下载中 42%")).toBeInTheDocument();
  });

  it("当前模型行有「切换模型」入口，点击打开选择模型弹窗", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    await user.click(screen.getByRole("button", { name: "切换唤醒词模型" }));
    expect(await screen.findByText("选择唤醒词模型")).toBeInTheDocument();
    // 弹窗内展示唯一内置预设 zh-en（已装为当前 → 「当前模型」徽标）
    expect(screen.getByText("Zipformer KWS zh-en 3M")).toBeInTheDocument();
    // 「当前模型」徽标依赖 list_model_library 异步返回；
    // 页面行标签与弹窗徽标同名，取全部匹配断言两者都在
    const currentBadges = await screen.findAllByText("当前模型");
    expect(currentBadges.length).toBeGreaterThanOrEqual(2);
  });

  it("内置预设未安装时弹窗显示下载按钮", async () => {
    kwsConfig = { ...kwsConfig, models_present: true };
    modelLibrary = defaultModelLibrary().map((m) => ({
      ...m,
      installState: "not_installed",
      current: false,
      localPath: null,
      installId: null,
    }));
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    await user.click(screen.getByRole("button", { name: "切换唤醒词模型" }));
    expect(
      await screen.findByRole("button", { name: "下载Zipformer KWS zh-en 3M" }),
    ).toBeInTheDocument();
  });

  it("正在监听时切换模型：set_current_model 后自动重启监听（stop → start）", async () => {
    // 当前使用外部导入模型；内置 zh-en 已安装但非当前，可设为当前
    kwsConfig = {
      ...kwsConfig,
      models_present: true,
      model_dir: "/home/user/.zapmomo/models/my-external-kws",
    };
    modelLibrary = defaultModelLibrary().map((m) => ({ ...m, current: false }));
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    // 开启监听
    await user.click(screen.getByRole("switch", { name: "唤醒词监听开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", { device: null, keywords: null });
    });
    invokeMock.mockClear();

    // 打开切换弹窗，把 zh-en 设为当前
    await user.click(screen.getByRole("button", { name: "切换唤醒词模型" }));
    await user.click(await screen.findByRole("button", { name: "设为当前" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_current_model", {
        id: "kws-zipformer-zh-en-3m",
      });
    });
    // 后端返回 restart_required → 前端重启监听使新模型立即生效
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_listen");
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_listen", { device: null, keywords: null });
    });
  });

  it("未监听时切换模型：只写配置，不重启监听", async () => {
    kwsConfig = {
      ...kwsConfig,
      models_present: true,
      model_dir: "/home/user/.zapmomo/models/my-external-kws",
    };
    modelLibrary = defaultModelLibrary().map((m) => ({ ...m, current: false }));
    const user = userEvent.setup();
    renderKwsPage();
    await screen.findByText("未启用");

    await user.click(screen.getByRole("button", { name: "切换唤醒词模型" }));
    await user.click(await screen.findByRole("button", { name: "设为当前" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_current_model", {
        id: "kws-zipformer-zh-en-3m",
      });
    });
    expect(invokeMock).not.toHaveBeenCalledWith("stop_listen");
    expect(invokeMock).not.toHaveBeenCalledWith("start_listen");
  });
});
