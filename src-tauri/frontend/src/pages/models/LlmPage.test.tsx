import { act, render, screen, waitFor, within } from "@testing-library/react";
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
  model_dir: "/home/user/.zapmomo/models/sherpa-onnx-kws",
  provider: "cpu",
  num_threads: 4,
  sample_rate: 16000,
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

/** 采样参数（远程语义：仅 7 项随请求发送的参数）。 */
const DEFAULT_PARAMS = {
  max_tokens: 512,
  temperature: 0.7,
  top_p: 0.8,
  top_k: 20,
  min_p: 0.05,
  repeat_penalty: 1.05,
  seed: 0,
};

function makeLlmConfig() {
  return {
    enabled: false,
    provider: "openai-compatible",
    ready: false,
    settings_path: "/home/user/.zapmomo/settings.toml",
    system_prompt: "你是 ZapMomo 的 AI 伙伴。",
    base_url: null as string | null,
    api_key_masked: null as string | null,
    model: null as string | null,
    params: { ...DEFAULT_PARAMS },
  };
}

let llmConfig: ReturnType<typeof makeLlmConfig>;

/** 把 mock 配置标记为「已填写远程连接三要素」。 */
function configureConnection() {
  llmConfig.base_url = "https://open.bigmodel.cn/api/paas/v4";
  llmConfig.api_key_masked = "sk-***1234";
  llmConfig.model = "glm-4.7-flash";
}

function renderLlmPage() {
  return render(
    <MemoryRouter initialEntries={["/models/llm"]}>
      <App />
    </MemoryRouter>,
  );
}

/** 触发 `llm-status` 事件，把模型置为已连接（模拟后台连接完成 / 启动自动连接）。 */
function fireReady() {
  llmConfig.ready = true; // 同步 mock 状态，使后续 refreshConfig 也读到已就绪（贴近真实后端）
  act(() => {
    listeners.get("llm-status")?.({ payload: { ready: true } });
  });
}

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  llmConfig = makeLlmConfig();

  invokeMock.mockImplementation(
    (
      cmd: string,
      args?: {
        baseUrl?: string;
        apiKey?: string | null;
        model?: string;
        params?: Record<string, number>;
        prompt?: string;
      },
    ) => {
      switch (cmd) {
        case "get_app_info":
          return Promise.resolve({ version: "0.1.4", product_name: "ZapMomo" });
        case "list_devices":
          return Promise.resolve(["内置麦克风"]);
        case "get_kws_config":
          return Promise.resolve(KWS_CONFIG);
        case "is_listening":
          return Promise.resolve(false);
        case "get_asr_config":
          return Promise.resolve(ASR_CONFIG);
        case "get_tts_config":
          return Promise.resolve(TTS_CONFIG);
        case "list_tts_voices":
          return Promise.resolve([]);
        case "get_llm_config":
          // 返回新对象引用，保证 refreshConfig 触发重渲染（mock 中 setter 就地改 llmConfig）
          return Promise.resolve({ ...llmConfig, params: { ...llmConfig.params } });
        case "is_asr_listening":
          return Promise.resolve(false);
        case "is_llm_ready":
          return Promise.resolve(false);
        case "is_voice_session_running":
          return Promise.resolve(false);
        case "list_model_library":
          return Promise.resolve([]);
        case "set_llm_connection":
          llmConfig.base_url = args?.baseUrl ?? null;
          llmConfig.model = args?.model ?? null;
          // apiKey 留空（null）= 不修改已保存的 Key
          if (args?.apiKey) llmConfig.api_key_masked = "sk-***masked";
          return Promise.resolve(undefined);
        case "set_llm_params":
          // 替换为新对象引用（贴近真实后端：保存后 resolve 出新 params）
          llmConfig.params = { ...llmConfig.params, ...args?.params };
          return Promise.resolve(undefined);
        case "set_llm_system_prompt":
          llmConfig.system_prompt = args?.prompt ?? "";
          return Promise.resolve(undefined);
        // load/unload/chat/stop 命令直接返回 undefined（由 default 兜底）
        default:
          return Promise.resolve(undefined);
      }
    },
  );
});

describe("LlmPage（AI 大脑配置）", () => {
  it("渲染页面标题与远程连接表单", async () => {
    renderLlmPage();
    expect(await screen.findByText("AI 大脑（LLM）配置")).toBeInTheDocument();
    expect(screen.getByText("模型与能力")).toBeInTheDocument();
    expect(screen.getByLabelText("API 地址")).toBeInTheDocument();
    expect(screen.getByLabelText("API Key")).toBeInTheDocument();
    expect(screen.getByLabelText("模型名")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存连接配置" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "测试模型" })).toBeInTheDocument();
  });

  it("未配置：状态显示未配置，连接开关与测试模型禁用", async () => {
    renderLlmPage();
    expect(await screen.findByText("未配置")).toBeInTheDocument();

    const runSwitch = screen.getByRole("switch", { name: "连接开关" });
    expect(runSwitch).toBeDisabled();
    expect(runSwitch).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("button", { name: "测试模型" })).toBeDisabled();
  });

  it("已有配置回填：API 地址/模型名填入输入框，API Key 以掩码 placeholder 展示", async () => {
    configureConnection();
    renderLlmPage();

    expect(
      await screen.findByDisplayValue("https://open.bigmodel.cn/api/paas/v4"),
    ).toBeInTheDocument();
    expect(screen.getByDisplayValue("glm-4.7-flash")).toBeInTheDocument();
    // Key 不回显明文：输入框为空，placeholder 展示掩码
    const keyInput = screen.getByLabelText("API Key");
    expect(keyInput).toHaveValue("");
    expect(keyInput).toHaveAttribute("placeholder", "已保存（sk-***1234）");
    // 已配置但未连接：开关可用（点击即连接）
    expect(screen.getByRole("switch", { name: "连接开关" })).toBeEnabled();
  });

  it("未填 API 地址保存：不调用 invoke 且显示内联错误", async () => {
    const user = userEvent.setup();
    renderLlmPage();
    await screen.findByText("AI 大脑（LLM）配置");

    await user.type(screen.getByLabelText("模型名"), "glm-4.7-flash");
    await user.click(screen.getByRole("button", { name: "保存连接配置" }));

    expect(await screen.findByText(/请填写 API 地址/)).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("set_llm_connection", expect.anything());
  });

  it("未填模型名保存：不调用 invoke 且显示内联错误", async () => {
    const user = userEvent.setup();
    renderLlmPage();
    await screen.findByText("AI 大脑（LLM）配置");

    await user.type(screen.getByLabelText("API 地址"), "https://open.bigmodel.cn/api/paas/v4");
    await user.click(screen.getByRole("button", { name: "保存连接配置" }));

    expect(await screen.findByText(/请填写模型名/)).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("set_llm_connection", expect.anything());
  });

  it("填写三要素保存：调用 set_llm_connection，成功后清空 API Key 输入框", async () => {
    const user = userEvent.setup();
    renderLlmPage();
    await screen.findByText("AI 大脑（LLM）配置");

    await user.type(screen.getByLabelText("API 地址"), "https://open.bigmodel.cn/api/paas/v4");
    await user.type(screen.getByLabelText("API Key"), "sk-real-key");
    await user.type(screen.getByLabelText("模型名"), "glm-4.7-flash");
    await user.click(screen.getByRole("button", { name: "保存连接配置" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_llm_connection", {
        baseUrl: "https://open.bigmodel.cn/api/paas/v4",
        apiKey: "sk-real-key",
        model: "glm-4.7-flash",
      });
    });
    // 保存成功后 Key 输入框清空（不持久展示明文）
    await waitFor(() => {
      expect(screen.getByLabelText("API Key")).toHaveValue("");
    });
  });

  it("API Key 留空保存：apiKey 传 null（不修改已保存的 Key）", async () => {
    configureConnection();
    const user = userEvent.setup();
    renderLlmPage();
    // 等回填完成后再改模型名（否则 hydrate 覆盖编辑）
    await screen.findByDisplayValue("glm-4.7-flash");

    const modelInput = screen.getByLabelText("模型名");
    await user.clear(modelInput);
    await user.type(modelInput, "glm-4.7-air");
    await user.click(screen.getByRole("button", { name: "保存连接配置" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_llm_connection", {
        baseUrl: "https://open.bigmodel.cn/api/paas/v4",
        apiKey: null,
        model: "glm-4.7-air",
      });
    });
  });

  it("保存连接配置失败：内联展示后端错误", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "set_llm_connection") return Promise.reject("invalid api key");
      if (cmd === "get_llm_config")
        return Promise.resolve({ ...llmConfig, params: { ...llmConfig.params } });
      return Promise.resolve(undefined);
    });
    const user = userEvent.setup();
    renderLlmPage();
    await screen.findByText("AI 大脑（LLM）配置");

    await user.type(screen.getByLabelText("API 地址"), "https://open.bigmodel.cn/api/paas/v4");
    await user.type(screen.getByLabelText("模型名"), "glm-4.7-flash");
    await user.click(screen.getByRole("button", { name: "保存连接配置" }));

    // 表单内联错误 + 标题行状态错误同时透出，断言至少一处展示
    expect((await screen.findAllByText(/invalid api key/)).length).toBeGreaterThan(0);
  });

  it("点击连接开关调用 load_llm_model 并进入连接中，llm-status ready 后显示已连接", async () => {
    configureConnection();
    const user = userEvent.setup();
    renderLlmPage();

    const runSwitch = await screen.findByRole("switch", { name: "连接开关" });
    await waitFor(() => expect(runSwitch).toBeEnabled());
    await user.click(runSwitch);

    expect(invokeMock).toHaveBeenCalledWith("load_llm_model");
    expect(await screen.findByText("连接中")).toBeInTheDocument();
    // 连接中开关禁用，防止重复连接
    expect(runSwitch).toBeDisabled();

    fireReady();
    expect(await screen.findByText("已连接")).toBeInTheDocument();
    expect(runSwitch).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("button", { name: "测试模型" })).toBeEnabled();
  });

  it("已连接时点击开关调用 unload_llm_model 并回到未连接", async () => {
    configureConnection();
    const user = userEvent.setup();
    renderLlmPage();

    const runSwitch = await screen.findByRole("switch", { name: "连接开关" });
    await waitFor(() => expect(runSwitch).toBeEnabled());
    fireReady();
    expect(await screen.findByText("已连接")).toBeInTheDocument();

    await user.click(runSwitch);

    expect(invokeMock).toHaveBeenCalledWith("unload_llm_model");
    expect(await screen.findByText("未连接")).toBeInTheDocument();
    expect(runSwitch).toHaveAttribute("aria-checked", "false");
  });

  it("测试对话框：发送文本、流式接收 token、生成结束后隐藏停止按钮", async () => {
    configureConnection();
    const user = userEvent.setup();
    renderLlmPage();

    const runSwitch = await screen.findByRole("switch", { name: "连接开关" });
    await waitFor(() => expect(runSwitch).toBeEnabled());
    fireReady();

    await user.click(screen.getByRole("button", { name: "测试模型" }));
    const dialog = await screen.findByRole("dialog", { name: "测试模型" });
    expect(dialog).toHaveTextContent("当前模型：glm-4.7-flash");

    await user.type(screen.getByLabelText("测试消息"), "你好");
    await user.click(screen.getByRole("button", { name: "发送" }));

    expect(invokeMock).toHaveBeenCalledWith("chat_llm", { text: "你好" });
    expect(await screen.findByText("生成中…")).toBeInTheDocument();

    act(() => {
      listeners.get("llm-token")?.({ payload: { text: "你" } });
    });
    act(() => {
      listeners.get("llm-token")?.({ payload: { text: "好" } });
    });
    expect(await screen.findByText("你好")).toBeInTheDocument();

    act(() => {
      listeners.get("llm-finished")?.({ payload: {} });
    });
    await waitFor(() => {
      expect(screen.queryByText("生成中…")).not.toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: "停止" })).not.toBeInTheDocument();
  });

  it("生成中点击「停止」调用 stop_llm", async () => {
    configureConnection();
    const user = userEvent.setup();
    renderLlmPage();

    const runSwitch = await screen.findByRole("switch", { name: "连接开关" });
    await waitFor(() => expect(runSwitch).toBeEnabled());
    fireReady();

    await user.click(screen.getByRole("button", { name: "测试模型" }));
    await screen.findByRole("dialog", { name: "测试模型" });
    await user.type(screen.getByLabelText("测试消息"), "你好");
    await user.click(screen.getByRole("button", { name: "发送" }));

    await user.click(await screen.findByRole("button", { name: "停止" }));
    expect(invokeMock).toHaveBeenCalledWith("stop_llm");
  });

  it("关闭测试对话框不断开连接", async () => {
    configureConnection();
    const user = userEvent.setup();
    renderLlmPage();

    const runSwitch = await screen.findByRole("switch", { name: "连接开关" });
    await waitFor(() => expect(runSwitch).toBeEnabled());
    fireReady();

    await user.click(screen.getByRole("button", { name: "测试模型" }));
    const dialog = await screen.findByRole("dialog", { name: "测试模型" });
    // 「关闭」按钮限定在对话框内（标题栏窗口关闭按钮同名）
    await user.click(within(dialog).getByRole("button", { name: "关闭" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "测试模型" })).not.toBeInTheDocument();
    });
    expect(invokeMock).not.toHaveBeenCalledWith("unload_llm_model");
    // 关闭只关 UI：连接状态保持
    expect(screen.getByText("已连接")).toBeInTheDocument();
  });

  it("高级参数：展开后回显解析后的参数值", async () => {
    const user = userEvent.setup();
    renderLlmPage();
    await screen.findByText("AI 大脑（LLM）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));

    await waitFor(() => {
      expect(screen.getByRole("textbox", { name: "温度" })).toHaveValue("0.7");
    });
    expect(screen.getByRole("textbox", { name: "最大生成 Tokens" })).toHaveValue("512");
    expect(screen.getByRole("textbox", { name: "Top-P" })).toHaveValue("0.8");
    expect(screen.getByRole("textbox", { name: "Top-K" })).toHaveValue("20");
    expect(screen.getByRole("textbox", { name: "Min-P" })).toHaveValue("0.05");
    expect(screen.getByRole("textbox", { name: "重复惩罚" })).toHaveValue("1.05");
    expect(screen.getByRole("textbox", { name: "随机种子" })).toHaveValue("0");
  });

  it("修改温度保存：调用 set_llm_params", async () => {
    const user = userEvent.setup();
    renderLlmPage();
    await screen.findByText("AI 大脑（LLM）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const temp = await screen.findByRole("textbox", { name: "温度" });
    await waitFor(() => expect(temp).toHaveValue("0.7"));
    await user.clear(temp);
    await user.type(temp, "0.9");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_llm_params", {
        params: { ...DEFAULT_PARAMS, temperature: 0.9 },
      });
    });
  });

  it("越界参数保存：不调用 invoke 且显示内联错误", async () => {
    const user = userEvent.setup();
    renderLlmPage();
    await screen.findByText("AI 大脑（LLM）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const maxTokens = await screen.findByRole("textbox", { name: "最大生成 Tokens" });
    await waitFor(() => expect(maxTokens).toHaveValue("512"));
    await user.clear(maxTokens);
    await user.type(maxTokens, "1");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    expect(await screen.findByText(/最大生成 Tokens 需在 16~262144 之间/)).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("set_llm_params", expect.anything());
  });

  it("保存系统提示词调用 set_llm_system_prompt；已连接时自动重新连接生效", async () => {
    configureConnection();
    const user = userEvent.setup();
    renderLlmPage();

    const runSwitch = await screen.findByRole("switch", { name: "连接开关" });
    await waitFor(() => expect(runSwitch).toBeEnabled());
    fireReady();
    expect(await screen.findByText("已连接")).toBeInTheDocument();

    const textarea = screen.getByLabelText("系统提示词");
    await waitFor(() => expect(textarea).toHaveValue("你是 ZapMomo 的 AI 伙伴。"));
    await user.clear(textarea);
    await user.type(textarea, "你是测试伙伴。");
    await user.click(screen.getByRole("button", { name: "保存提示词" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_llm_system_prompt", {
        prompt: "你是测试伙伴。",
      });
    });
    // 提示词在 provider 创建时固化：已连接且内容变化时自动 reload
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("load_llm_model");
    });
  });
});
