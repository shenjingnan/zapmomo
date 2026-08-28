import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import type { AsrConfigInfo } from "@/types/tauri";

const { invokeMock, listeners, dialogOpenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listeners: new Map<string, (e: { payload: unknown }) => void>(),
  dialogOpenMock: vi.fn(),
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

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: dialogOpenMock,
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

const ASR_CONFIG: AsrConfigInfo = {
  enabled: false,
  model_type: "zipformer",
  backend: "sherpa",
  model_dir:
    "/home/user/.zapmomo/models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20",
  provider: "cpu",
  num_threads: 4,
  sample_rate: 16000,
  chunk_size: 3200,
  decoding_method: "greedy_search",
  enable_endpoint: true,
  rule1_min_trailing_silence: 2.4,
  rule2_min_trailing_silence: 1.2,
  rule3_min_utterance_length: 20,
  blank_penalty: 0,
  hotwords: null,
  enable_punctuation: true,
  debug: false,
  models_present: false,
  punctuation_present: false,
  vad_present: false,
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

/** 可变 ASR 配置：单个用例可翻转 models_present 等字段（贴近真实后端）。 */
let asrConfig: typeof ASR_CONFIG;

/** 模拟后端正在识别的运行时标志（is_asr_listening）。 */
let asrListening = false;

/** 模拟后端正在听写的运行时标志（is_asr_dictating）。 */
let asrDictating = false;

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

/** 默认模型库桩：双语已装为当前，zh-14m 未装（弹窗只依赖这两条）。 */
function defaultAsrModelLibrary(): LibraryStub[] {
  return [
    {
      id: "asr-streaming-bilingual-zh-en",
      displayName: "Streaming Zipformer ASR zh-en",
      modelType: "asr",
      installState: "installed",
      current: true,
      localPath:
        "/home/user/.zapmomo/models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20",
      installId: "asr-streaming-bilingual-zh-en",
      repoId: null,
      ownership: "managed",
    },
    {
      id: "asr-streaming-zh-14m",
      displayName: "Streaming Zipformer ASR zh 14M",
      modelType: "asr",
      installState: "not_installed",
      current: false,
      localPath: null,
      installId: null,
      repoId: null,
      ownership: "managed",
    },
  ];
}

let modelLibrary: LibraryStub[] = defaultAsrModelLibrary();

/** 默认 command 桩：非 ASR 测试用例直接复用。 */
function defaultInvoke(cmd: string, args?: Record<string, unknown>) {
  switch (cmd) {
    case "get_app_info":
      return Promise.resolve({ version: "0.1.4", product_name: "ZapMomo" });
    case "list_devices":
      return Promise.resolve(["内置麦克风", "USB 麦克风"]);
    case "get_kws_config":
      return Promise.resolve({ ...KWS_CONFIG });
    case "get_microphone":
      return Promise.resolve(mic);
    case "set_microphone":
      mic = String(args?.mic ?? "");
      return Promise.resolve(undefined);
    case "is_listening":
      return Promise.resolve(false);
    case "get_asr_config":
      return Promise.resolve({ ...asrConfig });
    case "is_asr_listening":
      return Promise.resolve(asrListening);
    case "start_asr_listen":
      asrListening = true;
      return Promise.resolve(undefined);
    case "stop_asr_listen":
      asrListening = false;
      return Promise.resolve(undefined);
    case "is_asr_dictating":
      return Promise.resolve(asrDictating);
    case "start_asr_dictate":
      asrDictating = true;
      return Promise.resolve(undefined);
    case "stop_asr_dictate":
      asrDictating = false;
      return Promise.resolve(undefined);
    case "set_asr_params":
      asrConfig = { ...asrConfig, ...(args?.params ?? {}) };
      return Promise.resolve(undefined);
    case "download_asr_model":
      asrConfig = { ...asrConfig, models_present: true };
      return Promise.resolve(undefined);
    case "transcribe_audio":
      return Promise.resolve({
        text: "你好，世界",
        model_type: "sensevoice",
        model_dir: "/home/user/.zapmomo/models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8",
      });
    case "list_model_library":
      return Promise.resolve(modelLibrary);
    case "set_current_model":
      return Promise.resolve({
        modelType: "asr",
        modelId: "asr-streaming-zh-14m",
        path: "/home/user/.zapmomo/models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23",
        runtimeAction: "restart_required",
        effectiveImmediately: false,
        message: "已将 Streaming Zipformer ASR zh 14M 设为 ASR 当前模型，将在下次启动识别时生效",
      });
    case "get_tts_config":
      return Promise.resolve({ ...TTS_CONFIG });
    case "list_tts_voices":
      return Promise.resolve([]);
    case "get_llm_config":
      return Promise.resolve({ ...LLM_CONFIG });
    case "is_llm_ready":
      return Promise.resolve(false);
    default:
      return Promise.resolve(undefined);
  }
}

function renderAsrPage() {
  return render(
    <MemoryRouter initialEntries={["/models/asr"]}>
      <App />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  asrConfig = { ...ASR_CONFIG };
  asrListening = false;
  asrDictating = false;
  mic = "";
  modelLibrary = defaultAsrModelLibrary();
  invokeMock.mockImplementation(defaultInvoke);
});

describe("AsrPage（语音识别配置）", () => {
  it("页面标题与返回链接正确", async () => {
    renderAsrPage();
    expect(await screen.findByText("语音识别（ASR）配置")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /模型与能力/ })).toBeInTheDocument();
  });

  it("当前模型 basename 正确（不硬编码，取自 model_dir）", async () => {
    renderAsrPage();
    expect(
      await screen.findByText("sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20"),
    ).toBeInTheDocument();
  });

  it("模型缺失：Switch 禁用、未下载 Badge、下载按钮可用、测试禁用、警告提示", async () => {
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");
    expect(screen.getByText("未识别")).toBeInTheDocument();
    expect(screen.getByText("未下载")).toBeInTheDocument();

    const runSwitch = screen.getByRole("switch", { name: "语音识别开关" });
    expect(runSwitch).toBeDisabled();
    expect(runSwitch).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("button", { name: "下载模型" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "测试识别" })).toBeDisabled();
    expect(screen.getByText("模型文件缺失")).toBeInTheDocument();
  });

  it("模型就绪：已就绪 Badge、Switch 可用、无下载按钮、测试可用", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");
    expect(screen.getByText("已就绪")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "语音识别开关" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "下载模型" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "测试识别" })).toBeEnabled();
  });

  it("离线模型（SenseVoice）：听写开关可用、测试识别/转写文件可用", async () => {
    asrConfig = { ...asrConfig, models_present: true, model_type: "sensevoice" };
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");
    expect(screen.getByText("已就绪")).toBeInTheDocument();
    // 离线模型下顶部是「离线听写开关」（可开启听写），不再是「语音识别开关」
    expect(screen.getByRole("switch", { name: "离线听写开关" })).toBeEnabled();
    expect(screen.queryByRole("switch", { name: "语音识别开关" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "测试识别" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "转写文件" })).toBeEnabled();
  });

  it("离线模型：顶部开关 ON 调 start_asr_dictate、OFF 调 stop_asr_dictate，听写面板渲染", async () => {
    asrConfig = { ...asrConfig, models_present: true, model_type: "sensevoice" };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");
    expect(screen.getByText("免提连续听写")).toBeInTheDocument();
    // 顶部开关状态与面板状态各显示一处「未听写」
    expect(screen.getAllByText("未听写").length).toBeGreaterThan(0);

    await user.click(screen.getByRole("switch", { name: "离线听写开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_dictate", { device: null });
    });

    await user.click(screen.getByRole("switch", { name: "离线听写开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_asr_dictate");
    });
  });

  it("离线模型测试识别：自动转写自带示例音频（wavPath=null）", async () => {
    asrConfig = { ...asrConfig, models_present: true, model_type: "sensevoice" };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: "测试识别" }));
    expect(await screen.findByText("转写音频文件")).toBeInTheDocument();

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("transcribe_audio", { wavPath: null });
    });
    expect(await screen.findByText("你好，世界")).toBeInTheDocument();
  });

  it("转写文件：选择 wav → transcribe_audio → 展示结果", async () => {
    asrConfig = { ...asrConfig, models_present: true, model_type: "sensevoice" };
    dialogOpenMock.mockResolvedValue("/tmp/input.wav");
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: "转写文件" }));
    expect(await screen.findByText("转写音频文件")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "选择音频文件…" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("transcribe_audio", { wavPath: "/tmp/input.wav" });
    });
    expect(await screen.findByText("你好，世界")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制" })).toBeInTheDocument();
  });

  it("顶部开关 ON 调用 start_asr_listen（默认 null 设备）", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
  });

  it("选择麦克风后顶部开关 ON 携带该设备", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("combobox", { name: "麦克风来源" }));
    await user.click(await screen.findByRole("option", { name: "内置麦克风" }));

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: "内置麦克风" });
    });
  });

  it("从后端记忆恢复麦克风：顶部开关直接使用记忆设备", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    mic = "内置麦克风";
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: "内置麦克风" });
    });
  });

  it("顶部开关 OFF 调用 stop_asr_listen", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_asr_listen");
    });
  });

  it("start 在途（pending）时顶部开关禁用，完成后恢复", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    let resolveStart!: () => void;
    const deferred = new Promise<void>((res) => {
      resolveStart = res;
    });
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) =>
      cmd === "start_asr_listen" ? deferred : defaultInvoke(cmd, args),
    );

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));

    // 在途：isListening 未落盘，但开关禁用防重复
    expect(screen.getByRole("switch", { name: "语音识别开关" })).toBeDisabled();

    await act(async () => {
      resolveStart();
    });
    await waitFor(() => {
      const sw = screen.getByRole("switch", { name: "语音识别开关" });
      expect(sw).toHaveAttribute("aria-checked", "true");
      expect(sw).toBeEnabled();
    });
  });

  it("asr-stopped 事件使开关回落 OFF", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));
    await waitFor(() => {
      expect(screen.getByRole("switch", { name: "语音识别开关" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
    });

    act(() => {
      listeners.get("asr-stopped")?.({ payload: { error: null } });
    });

    await waitFor(() => {
      expect(screen.getByRole("switch", { name: "语音识别开关" })).toHaveAttribute(
        "aria-checked",
        "false",
      );
    });
  });

  it("start_asr_listen 报错：状态「错误」并显示错误详情", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) =>
      cmd === "start_asr_listen"
        ? Promise.reject("缺少模型文件: encoder-epoch-99-avg-1.int8.onnx")
        : defaultInvoke(cmd, args),
    );

    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));

    expect(await screen.findByText("错误")).toBeInTheDocument();
    expect(screen.getByText("缺少模型文件: encoder-epoch-99-avg-1.int8.onnx")).toBeInTheDocument();
  });

  it("麦克风选择持久化到后端（set_microphone）", async () => {
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("combobox", { name: "麦克风来源" }));
    await user.click(await screen.findByRole("option", { name: "内置麦克风" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_microphone", { mic: "内置麦克风" });
    });
  });

  it("刷新设备按钮重新调用 list_devices", async () => {
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: "刷新设备列表" }));

    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter((c) => c[0] === "list_devices");
      expect(calls.length).toBeGreaterThanOrEqual(2);
    });
  });

  it("下载模型：调用 download_asr_model、显示进度、完成后 Badge 翻转", async () => {
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByRole("button", { name: /下载模型/ });

    let resolveDownload!: () => void;
    const deferred = new Promise<void>((res) => {
      resolveDownload = res;
    });
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) =>
      cmd === "download_asr_model" ? deferred : defaultInvoke(cmd, args),
    );

    await user.click(screen.getByRole("button", { name: /下载模型/ }));
    expect(screen.getByRole("button", { name: /下载中/ })).toBeDisabled();

    act(() => {
      listeners.get("asr-model-download-progress")?.({
        payload: { stage: "downloading", percent: 42, message: "下载中 42%" },
      });
    });
    expect(await screen.findByText("下载中 42%")).toBeInTheDocument();

    await act(async () => {
      // 模拟下载完成：后端模型文件就绪，get_asr_config 返回 models_present=true
      asrConfig = { ...asrConfig, models_present: true };
      resolveDownload();
    });

    await waitFor(() => {
      expect(screen.getByText("已就绪")).toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: /下载模型/ })).not.toBeInTheDocument();
  });

  it("测试对话框：打开显示标题与关闭提示", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();

    await user.click(await screen.findByRole("button", { name: /测试识别/ }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "测试语音识别" })).toBeInTheDocument();
    expect(screen.getByText("在本窗口内开启的识别，关闭时自动停止。")).toBeInTheDocument();
  });

  it("测试对话框：未在识别时打开自动开始识别", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();

    await user.click(await screen.findByRole("button", { name: /测试识别/ }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
    expect(await screen.findByText("正在识别")).toBeInTheDocument();
  });

  it("asr-result 部分结果显示为临时文本，最终结果显示为段", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();

    await user.click(await screen.findByRole("button", { name: /测试识别/ }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });

    act(() => {
      listeners.get("asr-result")?.({
        payload: {
          text: "你好",
          tokens: [],
          timestamps: null,
          start_time: null,
          is_final: false,
        },
      });
    });
    expect(await screen.findByText("你好")).toBeInTheDocument();

    act(() => {
      listeners.get("asr-result")?.({
        payload: {
          text: "你好，世界。",
          tokens: [],
          timestamps: null,
          start_time: null,
          is_final: true,
        },
      });
    });
    expect(await screen.findByText("你好，世界。")).toBeInTheDocument();
  });

  it("对话框自动启动的识别：关闭时自动停止（startedByDialog）", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();

    await user.click(await screen.findByRole("button", { name: /测试识别/ }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });

    await user.keyboard("{Escape}");

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_asr_listen");
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });

  it("打开前已在识别：不重复 start、关闭不停止", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    // 顶部开关开启识别
    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
    invokeMock.mockClear();

    // 打开对话框：正在识别，且不重复 start
    await user.click(screen.getByRole("button", { name: /测试识别/ }));
    expect(await screen.findByText("正在识别")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("start_asr_listen", expect.anything());

    // 关闭对话框：绝不停止（识别由顶部开关发起）
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invokeMock).not.toHaveBeenCalledWith("stop_asr_listen");
  });

  it("关闭后重新打开对话框：再次仅一次 start_asr_listen（复用同一 runtime）", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();

    await user.click(await screen.findByRole("button", { name: /测试识别/ }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });

    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_asr_listen");
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    invokeMock.mockClear();

    await user.click(screen.getByRole("button", { name: /测试识别/ }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
    const startCalls = invokeMock.mock.calls.filter((c) => c[0] === "start_asr_listen");
    expect(startCalls.length).toBe(1);
  });

  it("模型信息默认展开显示只读字段", async () => {
    asrConfig = { ...asrConfig, punctuation_present: true };
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    const trigger = screen.getByRole("button", { name: /模型信息/ });
    expect(trigger).toHaveAttribute("aria-expanded", "true");

    expect(screen.getByText("运行时")).toBeInTheDocument();
    expect(screen.getByText("sherpa-onnx")).toBeInTheDocument();
    expect(screen.getByText("执行 Provider")).toBeInTheDocument();
    expect(screen.getByText("cpu")).toBeInTheDocument();
    expect(screen.getByText("采样率")).toBeInTheDocument();
    expect(screen.getByText("16000")).toBeInTheDocument();
    expect(screen.getByText("识别模式")).toBeInTheDocument();
    expect(screen.getByText("流式")).toBeInTheDocument();
    expect(screen.getByText("支持语言")).toBeInTheDocument();
    expect(screen.getByText("中文、English")).toBeInTheDocument();
    expect(screen.getByText("标点模型")).toBeInTheDocument();
    expect(screen.getByText("已就绪")).toBeInTheDocument();
    expect(screen.getByText("模型目录")).toBeInTheDocument();
    expect(screen.getByText("配置路径")).toBeInTheDocument();
  });

  it("高级参数默认折叠", async () => {
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");
    const trigger = screen.getByRole("button", { name: /高级参数/ });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("高级参数展开回显真实参数值", async () => {
    asrConfig = {
      ...asrConfig,
      hotwords: "文森特卡索",
      enable_endpoint: false,
      enable_punctuation: true,
      debug: true,
    };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    expect(screen.getByRole("button", { name: /高级参数/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    await waitFor(() => {
      expect(screen.getByRole("textbox", { name: "线程数" })).toHaveValue("4");
    });
    expect(screen.getByRole("textbox", { name: "采样块大小" })).toHaveValue("3200");
    expect(screen.getByRole("textbox", { name: "断句·尾随静音 1" })).toHaveValue("2.4");
    expect(screen.getByRole("textbox", { name: "断句·尾随静音 2" })).toHaveValue("1.2");
    expect(screen.getByRole("textbox", { name: "断句·最大句长" })).toHaveValue("20");
    expect(screen.getByRole("textbox", { name: "空白符惩罚" })).toHaveValue("0");
    expect(screen.getByRole("textbox", { name: "热词增强" })).toHaveValue("文森特卡索");
    expect(screen.getByRole("switch", { name: "端点检测" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    expect(screen.getByRole("switch", { name: "自动标点" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("switch", { name: "调试输出" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("修改线程数保存调用 set_asr_params", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const threadsInput = await screen.findByRole("textbox", { name: "线程数" });
    await waitFor(() => {
      expect(threadsInput).toHaveValue("4");
    });
    await user.clear(threadsInput);
    await user.type(threadsInput, "8");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "set_asr_params",
        expect.objectContaining({
          params: expect.objectContaining({ num_threads: 8 }),
        }),
      );
    });
  });

  it("非法参数值保存显示内联错误且不调用 set_asr_params", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const threadsInput = await screen.findByRole("textbox", { name: "线程数" });
    await waitFor(() => {
      expect(threadsInput).toHaveValue("4");
    });
    await user.clear(threadsInput);
    await user.type(threadsInput, "100");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    expect(await screen.findByText("线程数 需在 1~32 之间")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("set_asr_params", expect.anything());
  });

  it("保存参数时正在识别会重启识别使改动生效", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    // 顶部开关开启识别
    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
    invokeMock.mockClear();

    // 展开高级参数修改线程数并保存
    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const threadsInput = await screen.findByRole("textbox", { name: "线程数" });
    await waitFor(() => {
      expect(threadsInput).toHaveValue("4");
    });
    await user.clear(threadsInput);
    await user.type(threadsInput, "8");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_asr_params", expect.anything());
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_asr_listen");
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
  });

  it("保存热词包含 hotwords 字符串", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const hotwordsInput = await screen.findByRole("textbox", { name: "热词增强" });
    await waitFor(() => {
      expect(hotwordsInput).toHaveValue("");
    });
    await user.type(hotwordsInput, "文森特卡索");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "set_asr_params",
        expect.objectContaining({
          params: expect.objectContaining({ hotwords: "文森特卡索" }),
        }),
      );
    });
  });

  it("保存自动标点开关包含 enable_punctuation", async () => {
    asrConfig = { ...asrConfig, models_present: true, punctuation_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const punctSwitch = await screen.findByRole("switch", { name: "自动标点" });
    await waitFor(() => {
      expect(punctSwitch).toHaveAttribute("aria-checked", "true");
    });
    await user.click(punctSwitch);
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "set_asr_params",
        expect.objectContaining({
          params: expect.objectContaining({ enable_punctuation: false }),
        }),
      );
    });
  });

  it("保存后回读配置：模型信息线程数同步更新", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("语音识别（ASR）配置");

    // 模型信息默认展开，当前线程数 4
    expect(screen.getByText("4")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    const threadsInput = await screen.findByRole("textbox", { name: "线程数" });
    await waitFor(() => {
      expect(threadsInput).toHaveValue("4");
    });
    await user.clear(threadsInput);
    await user.type(threadsInput, "8");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    // set_asr_params 写入 mock 的 asrConfig → get_asr_config 回读 → 模型信息线程数更新为 8
    await waitFor(() => {
      expect(screen.getByText("8")).toBeInTheDocument();
    });
  });

  it("切换页面后 ASR runtime 状态不丢失，返回 /models 正常", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    mic = "内置麦克风";
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    // 切到概览页
    await user.click(screen.getByRole("link", { name: /模型与能力/ }));
    expect(await screen.findByText("模型摘要")).toBeInTheDocument();

    // 切回 ASR 页：记忆设备仍在
    await user.click(screen.getByRole("link", { name: "配置语音识别（ASR）" }));
    expect(await screen.findByText("语音识别（ASR）配置")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "麦克风来源" })).toHaveTextContent("内置麦克风");
  });

  it("当前模型行有「切换模型」入口，点击打开选择模型弹窗", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("button", { name: "切换识别模型" }));
    expect(await screen.findByText("选择识别模型")).toBeInTheDocument();
    // 弹窗内展示内置预设：双语为当前，zh-14m 未装 → 下载按钮
    expect(screen.getByText("Streaming Zipformer ASR zh-en")).toBeInTheDocument();
    expect(screen.getByText("Streaming Zipformer ASR zh 14M")).toBeInTheDocument();
    // 「当前模型」徽标依赖 list_model_library 异步返回（列表加载前两行都是下载按钮）；
    // 页面行标签与弹窗徽标同名，取全部匹配断言两者都在
    const currentBadges = await screen.findAllByText("当前模型");
    expect(currentBadges.length).toBeGreaterThanOrEqual(2);
    expect(
      await screen.findByRole("button", { name: "下载Streaming Zipformer ASR zh 14M" }),
    ).toBeInTheDocument();
  });

  it("正在识别时切换模型：set_current_model 后自动重启识别（stop → start）", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    // zh-14m 已安装，可设为当前
    modelLibrary = defaultAsrModelLibrary().map((m) =>
      m.id === "asr-streaming-zh-14m"
        ? {
            ...m,
            installState: "installed",
            localPath:
              "/home/user/.zapmomo/models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23",
            installId: m.id,
          }
        : m,
    );
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    // 开启识别
    await user.click(screen.getByRole("switch", { name: "语音识别开关" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
    invokeMock.mockClear();

    // 打开切换弹窗，把 zh-14m 设为当前
    await user.click(screen.getByRole("button", { name: "切换识别模型" }));
    await user.click(await screen.findByRole("button", { name: "设为当前" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_current_model", {
        id: "asr-streaming-zh-14m",
      });
    });
    // 后端返回 restart_required → 前端重启识别使新模型立即生效（只带 device，无 keywords）
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_asr_listen");
    });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("start_asr_listen", { device: null });
    });
  });

  it("未识别时切换模型：只写配置，不重启识别", async () => {
    asrConfig = { ...asrConfig, models_present: true };
    modelLibrary = defaultAsrModelLibrary().map((m) =>
      m.id === "asr-streaming-zh-14m"
        ? {
            ...m,
            installState: "installed",
            localPath:
              "/home/user/.zapmomo/models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23",
            installId: m.id,
          }
        : m,
    );
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("button", { name: "切换识别模型" }));
    await user.click(await screen.findByRole("button", { name: "设为当前" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_current_model", {
        id: "asr-streaming-zh-14m",
      });
    });
    expect(invokeMock).not.toHaveBeenCalledWith("stop_asr_listen");
    expect(invokeMock).not.toHaveBeenCalledWith("start_asr_listen");
  });

  it("模型族切换后高级参数草稿按新键集重建，不崩溃（qwen3→zipformer 回归）", async () => {
    // 初始：qwen3 离线族 → 高级参数数字草稿仅 num_threads 一个键
    asrConfig = { ...asrConfig, model_type: "qwen3_asr", models_present: true };
    // zh-14m（zipformer 族）已装非当前，供「设为当前」；切换后 get_asr_config 返回
    // zipformer → numericKeys 从 1 个扩到 6 个（补 blank_penalty/断句等）
    modelLibrary = defaultAsrModelLibrary().map((m) =>
      m.id === "asr-streaming-zh-14m"
        ? {
            ...m,
            installState: "installed",
            localPath:
              "/home/user/.zapmomo/models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23",
            installId: m.id,
          }
        : m,
    );
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "set_current_model") {
        asrConfig = { ...asrConfig, model_type: "zipformer", backend: "sherpa" };
      }
      return defaultInvoke(cmd, args);
    });
    const user = userEvent.setup();
    renderAsrPage();
    await screen.findByText("未识别");

    await user.click(screen.getByRole("button", { name: "切换识别模型" }));
    await user.click(await screen.findByRole("button", { name: "设为当前" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_current_model", {
        id: "asr-streaming-zh-14m",
      });
    });

    // 修复前：新 config 键集扩展后，hydrate 用旧草稿（1 键）跑 isPristine →
    // parseNumericDraft 读 draft["blank_penalty"]=undefined → .trim() 崩溃白屏。
    // 修复后：草稿按新键集整体重建；展开高级参数，6 项齐全且回读后端现值
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByText("选择识别模型")).not.toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: /高级参数/ }));
    expect(await screen.findByLabelText("空白符惩罚")).toHaveValue("0");
    expect(screen.getByLabelText("线程数")).toHaveValue("4");
  });
});
