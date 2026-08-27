import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
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

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => "/fake/path/ref.wav"),
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
  chunk_size: 3200,
  decoding_method: "greedy_search",
  enable_endpoint: true,
  rule1_min_trailing_silence: 2.4,
  rule2_min_trailing_silence: 1.2,
  rule3_min_utterance_length: 0,
  blank_penalty: 0,
  hotwords: null,
  enable_punctuation: false,
  debug: false,
  models_present: false,
  punctuation_present: false,
  model_downloading: false,
  settings_path: "/home/user/.zapmomo/settings.toml",
};

const TTS_CONFIG = {
  model_type: "zipvoice",
  model_dir: "/home/user/.zapmomo/models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia",
  provider: "cpu",
  num_threads: 4,
  enabled: true,
  models_present: false,
  model_downloading: false,
  settings_path: "/home/user/.zapmomo/settings.toml",
  num_steps: 4,
  speed: 1.0,
  debug: false,
  voice: null as string | null,
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

/** 可变 TTS 配置：单个用例可翻转 models_present / enabled 等字段（贴近真实后端）。 */
let ttsConfig: typeof TTS_CONFIG;

/** 音色（内置 + 自定义；对应 list_tts_voices 合并返回）。 */
let ttsVoices: {
  id: string;
  name: string;
  wav_path: string;
  reference_text: string;
  custom: boolean;
}[];

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

/** 默认模型库桩：zipvoice 已装为当前（弹窗展示「当前模型」徽标依赖此条目）。 */
function defaultTtsModelLibrary(): LibraryStub[] {
  return [
    {
      id: "tts-zipvoice-distill-int8",
      displayName: "ZipVoice TTS zh-en",
      modelType: "tts",
      installState: "installed",
      current: true,
      localPath: "/home/user/.zapmomo/models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia",
      installId: "tts-zipvoice-distill-int8",
      repoId: null,
      ownership: "managed",
    },
  ];
}

const ttsModelLibrary: LibraryStub[] = defaultTtsModelLibrary();

/** 默认 command 桩：非 TTS 测试用例直接复用。 */
function defaultInvoke(
  cmd: string,
  args?: {
    enabled?: boolean;
    name?: string;
    sourceWavPath?: string;
    referenceText?: string;
    id?: string;
    seconds?: number;
    voice?: string | null;
  },
) {
  switch (cmd) {
    case "get_app_info":
      return Promise.resolve({ version: "0.1.4", product_name: "ZapMomo" });
    case "list_devices":
      return Promise.resolve(["内置麦克风"]);
    case "get_kws_config":
      return Promise.resolve({ ...KWS_CONFIG });
    case "get_asr_config":
      return Promise.resolve({ ...ASR_CONFIG });
    case "get_tts_config":
      return Promise.resolve({ ...ttsConfig });
    case "list_tts_voices":
      return Promise.resolve(ttsVoices);
    case "set_tts_enabled":
      ttsConfig = { ...ttsConfig, enabled: args?.enabled ?? false };
      return Promise.resolve(undefined);
    case "set_tts_params":
      return Promise.resolve(undefined);
    case "set_tts_voice":
      ttsConfig = { ...ttsConfig, voice: args?.voice ?? null };
      return Promise.resolve(undefined);
    case "save_tts_voice": {
      const saved = {
        id: "custom-1",
        name: args?.name ?? "我的声音",
        wav_path: "/home/user/.zapmomo/voices/custom-1.wav",
        reference_text: args?.referenceText ?? "",
        custom: true,
      };
      ttsVoices = [...ttsVoices, saved];
      return Promise.resolve(saved);
    }
    case "delete_tts_voice":
      ttsVoices = ttsVoices.filter((v) => v.id !== (args?.id ?? ""));
      return Promise.resolve(undefined);
    case "record_tts_voice":
      return Promise.resolve("/home/user/.zapmomo/tts/rec-1.wav");
    case "synthesize_tts":
      return Promise.resolve(undefined);
    case "stop_tts":
      return Promise.resolve(undefined);
    case "list_model_library":
      return Promise.resolve(ttsModelLibrary);
    case "transcribe_reference_audio":
      return Promise.resolve("参考音频的逐字转写文本");
    case "get_microphone":
      return Promise.resolve("");
    case "get_llm_config":
      return Promise.resolve({ ...LLM_CONFIG });
    case "is_listening":
      return Promise.resolve(false);
    case "is_asr_listening":
      return Promise.resolve(false);
    case "is_llm_ready":
      return Promise.resolve(false);
    default:
      return Promise.resolve(undefined);
  }
}

function renderTtsPage() {
  return render(
    <MemoryRouter initialEntries={["/models/tts"]}>
      <App />
    </MemoryRouter>,
  );
}

beforeAll(() => {
  // jsdom 未实现 HTMLMediaElement 的播放方法（play 返回 undefined 且记录 "Not implemented"），
  // TTS「合成并播放」的自动播放依赖 play()，这里覆盖为正常实现，避免 .catch 崩溃。
  const mediaProps: Record<"play" | "pause" | "load", () => unknown> = {
    play: () => Promise.resolve(),
    pause: () => undefined,
    load: () => undefined,
  };
  for (const [name, value] of Object.entries(mediaProps)) {
    Object.defineProperty(HTMLMediaElement.prototype, name, {
      configurable: true,
      value,
    });
  }
});

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  ttsConfig = { ...TTS_CONFIG };
  ttsVoices = [
    {
      id: "leijun-1",
      name: "雷军（男）",
      wav_path:
        "/home/user/.zapmomo/models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/test_wavs/leijun-1.wav",
      reference_text: "那还是36年前, 1987年.",
      custom: false,
    },
    {
      id: "news-female",
      name: "新闻女声",
      wav_path:
        "/home/user/.zapmomo/models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/test_wavs/news-female.wav",
      reference_text: "各位村民, 大家新年好!",
      custom: false,
    },
  ];
  invokeMock.mockImplementation(defaultInvoke);
});

describe("TtsPage（语音合成 TTS）", () => {
  it("标题正确、返回 /models 链接可用", async () => {
    renderTtsPage();
    expect(await screen.findByText("语音合成（TTS）配置")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /模型与能力/ })).toHaveAttribute("href", "/models");
  });

  it("未下载模型：显示模型名与「未下载」Badge、顶部状态「未下载模型」、「选择模型」可用、测试禁用", async () => {
    renderTtsPage();
    expect(
      await screen.findByText("sherpa-onnx-zipvoice-distill-int8-zh-en-emilia"),
    ).toBeInTheDocument();
    expect(await screen.findByText("未下载模型")).toBeInTheDocument();
    expect(screen.getByText("未下载")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "选择模型" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "测试语音" })).toBeDisabled();
  });

  it("模型已就绪：显示「已就绪」、顶部状态「已就绪」、选择模型可用、测试可用、开关可用", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    expect((await screen.findAllByText("已就绪")).length).toBeGreaterThanOrEqual(2);
    expect(screen.getByRole("button", { name: "选择模型" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "测试语音" })).toBeEnabled();
    expect(screen.getByRole("switch", { name: "语音合成开关" })).toBeEnabled();
  });

  it("点击「选择模型」打开选择合成模型弹窗，展示内置预设", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");

    await user.click(screen.getByRole("button", { name: "选择模型" }));
    expect(await screen.findByText("选择合成模型")).toBeInTheDocument();
    // 弹窗内展示内置预设：zipvoice 已装为当前 → 「当前模型」徽标
    expect(screen.getByText("ZipVoice TTS zh-en")).toBeInTheDocument();
    const currentBadges = await screen.findAllByText("当前模型");
    expect(currentBadges.length).toBeGreaterThanOrEqual(1);
  });

  it("顶部开关切换持久化 enabled（set_tts_enabled）", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");

    await user.click(screen.getByRole("switch", { name: "语音合成开关" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_tts_enabled", { enabled: false });
    });
    expect(await screen.findByText("已关闭")).toBeInTheDocument();
  });

  it("enabled=false 显示「已关闭」", async () => {
    ttsConfig = { ...ttsConfig, models_present: true, enabled: false };
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    expect(await screen.findByText("已关闭")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "语音合成开关" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  });

  it("模型缺失且关闭时仍显示「未下载模型」（模型缺失优先于 enabled）", async () => {
    ttsConfig = { ...ttsConfig, models_present: false, enabled: false };
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    expect(await screen.findByText("未下载模型")).toBeInTheDocument();
    expect(screen.queryByText("已关闭")).not.toBeInTheDocument();
  });

  it("无硬编码假模型（MOSS/Kokoro/ZipVoice 独立名不出现）", async () => {
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    expect(screen.queryByText(/MOSS-TTS-Nano/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Kokoro/)).not.toBeInTheDocument();
    expect(screen.queryByText(/^ZipVoice$/)).not.toBeInTheDocument();
  });

  it("高级参数默认折叠，展开后回显参数并提示下次合成生效", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");

    const trigger = screen.getByRole("button", { name: /高级参数/ });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    await user.click(trigger);
    expect(screen.getByRole("button", { name: /高级参数/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    // 回显解析后的参数
    expect(await screen.findByRole("textbox", { name: "扩散步数" })).toHaveValue("4");
    expect(screen.getByRole("textbox", { name: "默认语速" })).toHaveValue("1");
    expect(screen.getByRole("textbox", { name: "线程数" })).toHaveValue("4");
    expect(screen.getByText("修改保存后，下一次合成自动生效。")).toBeInTheDocument();
    // 无「重启」误导文案（TTS 引擎每次合成新建，无需重启）
    expect(screen.queryByText(/重启/)).not.toBeInTheDocument();
  });

  it("修改扩散步数并保存调用 set_tts_params", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    await user.click(screen.getByRole("button", { name: /高级参数/ }));

    const numSteps = await screen.findByRole("textbox", { name: "扩散步数" });
    await user.clear(numSteps);
    await user.type(numSteps, "8");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_tts_params", {
        params: { num_steps: 8, speed: 1, num_threads: 4, debug: false },
      });
    });
  });

  it("扩散步数越界：保存失败并显示内联错误，不调用 set_tts_params", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    await user.click(screen.getByRole("button", { name: /高级参数/ }));

    const numSteps = await screen.findByRole("textbox", { name: "扩散步数" });
    await user.clear(numSteps);
    await user.type(numSteps, "100");
    await user.click(screen.getByRole("button", { name: "保存参数" }));

    expect(await screen.findByText(/扩散步数 需在 1~32/)).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("set_tts_params", expect.anything());
  });

  it("TestDialog 语速初始化用高级配置的默认语速", async () => {
    ttsConfig = { ...ttsConfig, models_present: true, speed: 1.5 };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    // 语速 Slider 显示值同步为全局默认 1.5x
    expect(await screen.findByText("1.5x")).toBeInTheDocument();
  });

  it("不显示 LLM→TTS 联动误导文案", async () => {
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    expect(screen.queryByText(/语音回复/)).not.toBeInTheDocument();
  });

  it("模型信息默认展开，展示真实只读字段", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    const trigger = screen.getByRole("button", { name: /模型信息/ });
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("运行时")).toBeInTheDocument();
    expect(screen.getByText("sherpa-onnx")).toBeInTheDocument();
    expect(screen.getByText("执行 Provider")).toBeInTheDocument();
    expect(screen.getByText("cpu")).toBeInTheDocument();
    // 「线程数」同时出现在模型信息与高级参数（折叠区仍在 DOM），用 getAll 断言存在
    expect(screen.getAllByText("线程数").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("支持语言")).toBeInTheDocument();
    expect(
      screen.getByText("/home/user/.zapmomo/models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia"),
    ).toBeInTheDocument();
    expect(screen.getByText("配置路径")).toBeInTheDocument();
  });

  it("测试 Dialog 正常打开，音色来自 list_tts_voices", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "测试语音合成" })).toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "音色" }));
    expect(await screen.findByRole("option", { name: "雷军（男）" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "新闻女声" })).toBeInTheDocument();
  });

  it("主页「默认音色」选择器渲染并持久化（set_tts_voice）", async () => {
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");

    const combobox = await screen.findByRole("combobox", { name: "默认音色" });
    await waitFor(() => expect(combobox).toBeEnabled());
    await user.click(combobox);
    expect(await screen.findByRole("option", { name: "雷军（男）" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "新闻女声" })).toBeInTheDocument();

    // 选择内置音色 → 持久化为全局默认音色
    await user.click(screen.getByRole("option", { name: "雷军（男）" }));
    expect(invokeMock).toHaveBeenCalledWith("set_tts_voice", { voice: "leijun-1" });
  });

  it("主页默认音色显示 get_tts_config 已持久化的音色", async () => {
    ttsConfig = { ...ttsConfig, models_present: true, voice: "news-female" };
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    const combobox = await screen.findByRole("combobox", { name: "默认音色" });
    await waitFor(() => expect(combobox).toHaveTextContent("新闻女声"));
  });

  it("文本输入生效，合成调用 synthesize_tts 携带真实 text / voice / speed", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    // 选择内置音色
    await user.click(screen.getByRole("combobox", { name: "音色" }));
    await user.click(await screen.findByRole("option", { name: "雷军（男）" }));

    // 输入测试文本
    const textarea = screen.getByLabelText("测试文本");
    await user.clear(textarea);
    await user.type(textarea, "你好世界");

    await user.click(screen.getByRole("button", { name: "合成并播放" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("synthesize_tts", {
        text: "你好世界",
        speed: 1,
        sid: null,
        voice: "leijun-1",
        referenceWav: null,
        referenceText: null,
      });
    });
  });

  it("sid 模型（vits）：音色固定禁用、无音色管理入口，合成携带 sid=0 且不传 voice/reference", async () => {
    ttsConfig = { ...ttsConfig, model_type: "vits", models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    // sid 模型下音色下拉禁用、无「音色管理」入口
    expect(screen.getByRole("combobox", { name: "音色" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "音色管理" })).not.toBeInTheDocument();

    const textarea = screen.getByLabelText("测试文本");
    await user.clear(textarea);
    await user.type(textarea, "测试vits模型");
    await user.click(screen.getByRole("button", { name: "合成并播放" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("synthesize_tts", {
        text: "测试vits模型",
        speed: 1,
        sid: 0,
        voice: null,
        referenceWav: null,
        referenceText: null,
      });
    });
  });

  it("qwen3_tts（强制克隆族）：主页音色下拉可选、音色管理可见、占位提示必须选择克隆音色", async () => {
    ttsConfig = {
      ...ttsConfig,
      model_type: "qwen3_tts_06",
      model_dir: "/home/user/.zapmomo/models/qwen3-tts-12hz-0.6b-base-q8_0",
      models_present: true,
    };
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");

    // 克隆族入口：音色管理可见（区别于 sid 固定模型）
    expect(screen.getByRole("button", { name: "音色管理" })).toBeInTheDocument();

    // 音色下拉可选（非禁用占位），占位提示「必须选择克隆音色」（无 auto voice 兜底）
    const combobox = await screen.findByRole("combobox", { name: "默认音色" });
    await waitFor(() => expect(combobox).toBeEnabled());
    expect(combobox).toHaveTextContent("必须选择克隆音色");
    await user.click(combobox);
    expect(await screen.findByRole("option", { name: "雷军（男）" })).toBeInTheDocument();
    // 强制克隆族无「自动音色」/内置默认占位项（空音色会被后端拦截报错）
    expect(screen.queryByRole("option", { name: /自动音色/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /内置 leijun/ })).not.toBeInTheDocument();

    // 选择音色持久化为全局默认音色
    await user.click(screen.getByRole("option", { name: "雷军（男）" }));
    expect(invokeMock).toHaveBeenCalledWith("set_tts_voice", { voice: "leijun-1" });
  });

  it("qwen3_tts：音色库为空时主页音色下拉禁用（需先在音色管理添加）", async () => {
    ttsVoices = [];
    ttsConfig = { ...ttsConfig, model_type: "qwen3_tts_17", models_present: true };
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");
    const combobox = await screen.findByRole("combobox", { name: "默认音色" });
    await waitFor(() => expect(combobox).toBeDisabled());
    // 音色管理入口仍可见（空音色库时的唯一出路）
    expect(screen.getByRole("button", { name: "音色管理" })).toBeInTheDocument();
  });

  it("qwen3_tts：TestDialog 音色下拉可选且有管理音色入口（同克隆族）", async () => {
    ttsConfig = { ...ttsConfig, model_type: "qwen3_tts_06", models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    // 克隆族：音色下拉不禁用（区别于 sid 固定占位）、管理音色入口可见
    expect(screen.getByRole("combobox", { name: "音色" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "管理音色" })).toBeInTheDocument();

    // 强制克隆族占位提示必须选择克隆音色，且无「默认音色」空值项
    await user.click(screen.getByRole("combobox", { name: "音色" }));
    expect(await screen.findByRole("option", { name: "雷军（男）" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "默认音色" })).not.toBeInTheDocument();
  });

  it("管理音色：从页面基础配置打开，上传音频 + 命名 + 参考文本保存调 save_tts_voice", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await screen.findByText("语音合成（TTS）配置");

    // 页面入口「音色管理」打开管理对话框
    await user.click(screen.getByRole("button", { name: "音色管理" }));
    expect(screen.getByRole("dialog", { name: "管理音色" })).toBeInTheDocument();

    // 添加音色 → 上传音频（plugin-dialog mock 返回固定路径）
    await user.click(screen.getByRole("button", { name: "添加音色" }));
    await user.click(screen.getByRole("button", { name: "上传音频" }));
    expect(await screen.findByText("/fake/path/ref.wav")).toBeInTheDocument();

    await user.type(screen.getByLabelText("音色名称"), "我的声音");
    await user.type(screen.getByLabelText("参考文本"), "这是参考文本");
    await user.click(screen.getByRole("button", { name: "保存音色" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("save_tts_voice", {
        name: "我的声音",
        sourceWavPath: "/fake/path/ref.wav",
        referenceText: "这是参考文本",
      });
    });
    // 保存成功回到列表并显示新音色
    expect(await screen.findByText("我的声音")).toBeInTheDocument();
  });

  it("管理音色：在线录音调 record_tts_voice 并回填 wav 路径", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "音色管理" }));
    await user.click(screen.getByRole("button", { name: "添加音色" }));

    await user.click(screen.getByRole("button", { name: "开始录音" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("record_tts_voice", { seconds: 5, device: null });
    });
    // 录音完成后回填 wav 路径（供保存为音色）
    expect(await screen.findByText("/home/user/.zapmomo/tts/rec-1.wav")).toBeInTheDocument();
  });

  it("TestDialog 内「管理音色」入口：保存后下拉含自定义音色，选中合成携带 referenceWav/referenceText", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    // TestDialog 内「管理音色」按钮打开管理对话框（叠层）
    await user.click(screen.getByRole("button", { name: "管理音色" }));
    expect(screen.getByRole("dialog", { name: "管理音色" })).toBeInTheDocument();

    // 添加自定义音色
    await user.click(screen.getByRole("button", { name: "添加音色" }));
    await user.click(screen.getByRole("button", { name: "上传音频" }));
    await screen.findByText("/fake/path/ref.wav");
    await user.type(screen.getByLabelText("音色名称"), "我的声音");
    await user.type(screen.getByLabelText("参考文本"), "克隆参考文本");
    await user.click(screen.getByRole("button", { name: "保存音色" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("save_tts_voice", expect.anything());
    });

    // Esc 只关管理对话框，不关 TestDialog
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "管理音色" })).not.toBeInTheDocument();
    });
    expect(screen.getByRole("dialog", { name: "测试语音合成" })).toBeInTheDocument();

    // TestDialog 音色下拉现在包含自定义音色，选中后合成携带 referenceWav/referenceText
    await user.click(screen.getByRole("combobox", { name: "音色" }));
    await user.click(await screen.findByRole("option", { name: "我的声音" }));
    const textarea = screen.getByLabelText("测试文本");
    await user.clear(textarea);
    await user.type(textarea, "用我的声音");
    await user.click(screen.getByRole("button", { name: "合成并播放" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("synthesize_tts", {
        text: "用我的声音",
        speed: 1,
        sid: null,
        voice: null,
        referenceWav: "/home/user/.zapmomo/voices/custom-1.wav",
        referenceText: "克隆参考文本",
      });
    });
  });

  it("管理音色：删除自定义音色调 delete_tts_voice", async () => {
    ttsVoices = [
      {
        id: "leijun-1",
        name: "雷军（男）",
        wav_path:
          "/home/user/.zapmomo/models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/test_wavs/leijun-1.wav",
        reference_text: "那还是36年前, 1987年.",
        custom: false,
      },
      {
        id: "custom-1",
        name: "我的声音",
        wav_path: "/home/user/.zapmomo/voices/custom-1.wav",
        reference_text: "克隆参考文本",
        custom: true,
      },
    ];
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "音色管理" }));

    expect(screen.getByText("我的声音")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "删除音色 我的声音" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("delete_tts_voice", { id: "custom-1" });
    });
  });

  it("TestDialog 不再有临时的「自定义…」选项（被音色库取代）", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    await user.click(screen.getByRole("combobox", { name: "音色" }));
    expect(screen.queryByRole("option", { name: "自定义…" })).not.toBeInTheDocument();
    expect(await screen.findByRole("option", { name: "雷军（男）" })).toBeInTheDocument();
  });

  it("合成中显示「合成中」状态与进度，完成后显示结果", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    await user.click(screen.getByRole("button", { name: "合成并播放" }));
    expect((await screen.findAllByText("合成中")).length).toBeGreaterThanOrEqual(1);

    act(() => {
      listeners.get("tts-progress")?.({
        payload: { percent: 0.5 },
      });
    });
    expect(await screen.findByText("合成中 50%")).toBeInTheDocument();

    act(() => {
      listeners.get("tts-result")?.({
        payload: { path: "/tmp/tts-1.wav", duration: 2.5, sample_rate: 24000 },
      });
      listeners.get("tts-stopped")?.({ payload: { error: null } });
    });
    expect(await screen.findByText(/已生成音频/)).toBeInTheDocument();
  });

  it("合成期间顶部开关禁用（不承担停止语义）", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));
    await user.click(screen.getByRole("button", { name: "合成并播放" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("synthesize_tts", expect.anything());
    });
    expect(screen.getByRole("switch", { name: "语音合成开关" })).toBeDisabled();
  });

  it("点击「停止」调用 stop_tts", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));
    await user.click(screen.getByRole("button", { name: "合成并播放" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("synthesize_tts", expect.anything());
    });

    await user.click(screen.getByRole("button", { name: "停止" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_tts");
    });
  });

  it("合成错误（tts-stopped error）在 Dialog 内显示", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));
    await user.click(screen.getByRole("button", { name: "合成并播放" }));

    act(() => {
      listeners.get("tts-stopped")?.({ payload: { error: "语音合成失败。" } });
    });
    expect(await screen.findByText("语音合成失败。")).toBeInTheDocument();
  });

  it("未发起合成时关闭 Dialog：不调用 stop_tts", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));

    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invokeMock).not.toHaveBeenCalledWith("stop_tts");
  });

  it("Dialog 发起的合成：关闭时调用 stop_tts 清理", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));
    await user.click(screen.getByRole("button", { name: "合成并播放" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("synthesize_tts", expect.anything());
    });

    // 合成仍在进行中，关闭 Dialog
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_tts");
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });

  it("合成已完成时关闭 Dialog：不调用 stop_tts（无在途任务）", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));
    await user.click(screen.getByRole("button", { name: "合成并播放" }));
    act(() => {
      listeners.get("tts-result")?.({
        payload: { path: "/tmp/tts-1.wav", duration: 2.5, sample_rate: 24000 },
      });
      listeners.get("tts-stopped")?.({ payload: { error: null } });
    });
    await screen.findByText(/已生成音频/);

    invokeMock.mockClear();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invokeMock).not.toHaveBeenCalledWith("stop_tts");
  });

  it("页面切换后合成结果不丢（runtime 常驻路由外）", async () => {
    ttsConfig = { ...ttsConfig, models_present: true };
    const user = userEvent.setup();
    renderTtsPage();
    await user.click(await screen.findByRole("button", { name: "测试语音" }));
    await user.click(screen.getByRole("button", { name: "合成并播放" }));
    act(() => {
      listeners.get("tts-result")?.({
        payload: { path: "/tmp/tts-1.wav", duration: 2.5, sample_rate: 24000 },
      });
      listeners.get("tts-stopped")?.({ payload: { error: null } });
    });
    await screen.findByText(/已生成音频/);
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    // 切到概览页再回来，重新打开 Dialog：结果仍在
    await user.click(screen.getByRole("link", { name: /模型与能力/ }));
    expect(await screen.findByText("模型摘要")).toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "配置语音合成（TTS）" }));
    await screen.findByText("语音合成（TTS）配置");
    await user.click(screen.getByRole("button", { name: "测试语音" }));
    expect(await screen.findByText(/已生成音频/)).toBeInTheDocument();
  });
});
