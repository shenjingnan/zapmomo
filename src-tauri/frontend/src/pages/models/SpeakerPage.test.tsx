import { render, screen, waitFor } from "@testing-library/react";
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

const SPEAKER_CONFIG = {
  enabled: false,
  threshold: 0.6,
  min_audio_duration_secs: 1.0,
  provider: "cpu",
  num_threads: 1,
  debug: false,
  model_dir: "/home/user/.zapmomo/models/3dspeaker-speech-campplus-sv-zh-cn-16k-common",
  model_present: false,
  model_downloading: false,
  speaker_profiles_dir: "/home/user/.zapmomo/speaker_profiles",
  settings_path: "/home/user/.zapmomo/settings.toml",
};

let speakerConfig: typeof SPEAKER_CONFIG;
let speakers: {
  speaker_id: string;
  sample_count: number;
  model: string;
  dim: number;
  updated_at: string;
}[];
let mic = "";

/** 默认 command 桩：只覆盖 Provider 与 SpeakerPage 用到的命令。 */
function defaultInvoke(
  cmd: string,
  args?: { enabled?: boolean; mic?: string; speakerId?: string; params?: Record<string, unknown> },
) {
  switch (cmd) {
    case "get_app_info":
      return Promise.resolve({ version: "0.1.4", product_name: "ZapMomo" });
    case "list_devices":
      return Promise.resolve(["内置麦克风"]);
    case "get_microphone":
      return Promise.resolve(mic);
    case "set_microphone":
      mic = args?.mic ?? "";
      return Promise.resolve(undefined);
    case "get_kws_config":
      return Promise.resolve({
        enabled: false,
        custom_keywords: "",
        model_dir: "/m/kws",
        provider: "cpu",
        num_threads: 2,
        sample_rate: 16000,
        chunk_size: 3200,
        keywords_score: 1.0,
        keywords_threshold: 0.25,
        debug: false,
        keywords: [],
        models_present: true,
        model_downloading: false,
        settings_path: "/s.toml",
      });
    case "get_asr_config":
      return Promise.resolve({
        model_dir: "/m/asr",
        provider: "cpu",
        num_threads: 2,
        sample_rate: 16000,
        models_present: true,
        punctuation_present: true,
        model_downloading: false,
        settings_path: "/s.toml",
      });
    case "get_tts_config":
      return Promise.resolve({
        model_dir: "/m/tts",
        provider: "cpu",
        num_threads: 2,
        enabled: true,
        models_present: true,
        model_downloading: false,
        settings_path: "/s.toml",
      });
    case "list_tts_voices":
      return Promise.resolve([]);
    case "get_llm_config":
      return Promise.resolve({
        enabled: false,
        provider: "local",
        model_path: "/m/qwen.gguf",
        models_present: true,
        ready: false,
        enable_thinking: false,
        auto_load: false,
        settings_path: "/s.toml",
        system_prompt: "",
        params: {},
      });
    case "list_model_library":
      return Promise.resolve([]);
    case "is_listening":
    case "is_asr_listening":
    case "is_asr_dictating":
    case "is_llm_ready":
    case "is_voice_session_running":
    case "is_tts_synthesizing":
      return Promise.resolve(false);
    case "get_speaker_config":
      return Promise.resolve({ ...speakerConfig });
    case "set_speaker_enabled":
      speakerConfig = { ...speakerConfig, enabled: args?.enabled ?? false };
      return Promise.resolve(undefined);
    case "set_speaker_params":
      speakerConfig = { ...speakerConfig, ...(args?.params ?? {}) };
      return Promise.resolve(undefined);
    case "download_speaker_model":
      speakerConfig = { ...speakerConfig, model_present: true };
      return Promise.resolve(undefined);
    case "list_speakers":
      return Promise.resolve(speakers);
    case "remove_speaker":
      speakers = speakers.filter((s) => s.speaker_id !== args?.speakerId);
      return Promise.resolve(true);
    default:
      return Promise.resolve(undefined);
  }
}

function renderSpeakerPage() {
  return render(
    <MemoryRouter initialEntries={["/models/speaker"]}>
      <App />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  speakerConfig = { ...SPEAKER_CONFIG };
  speakers = [];
  mic = "";
  invokeMock.mockImplementation(defaultInvoke);
});

describe("SpeakerPage（声纹识别配置）", () => {
  it("模型未下载：显示「未下载」Badge、缺模型 Alert 与「下载模型」按钮", async () => {
    renderSpeakerPage();
    expect(await screen.findByText("声纹识别（Speaker Recognition）")).toBeInTheDocument();
    expect(screen.getByText("未下载")).toBeInTheDocument();
    expect(screen.getByText("模型未下载")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /下载模型/ })).toBeInTheDocument();
    expect(screen.getByText(/尚未注册任何说话人/)).toBeInTheDocument();
  });

  it("模型就绪且有注册说话人：显示「已就绪」与说话人列表", async () => {
    speakerConfig = { ...speakerConfig, model_present: true };
    speakers = [
      {
        speaker_id: "owner",
        sample_count: 3,
        model: "campplus.onnx",
        dim: 192,
        updated_at: "2026-08-30T10:00:00+08:00",
      },
    ];
    renderSpeakerPage();
    expect(await screen.findByText("已就绪")).toBeInTheDocument();
    expect(screen.getByText("owner")).toBeInTheDocument();
    expect(screen.getByText(/3 段样本/)).toBeInTheDocument();
  });

  it("启用开关写入后端并回读", async () => {
    speakerConfig = { ...speakerConfig, model_present: true };
    const user = userEvent.setup();
    renderSpeakerPage();
    const enableSwitch = await screen.findByRole("switch", { name: "启用声纹识别" });
    await user.click(enableSwitch);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_speaker_enabled", { enabled: true });
    });
    await waitFor(() => {
      expect(screen.getByRole("switch", { name: "启用声纹识别" })).toBeChecked();
    });
  });

  it("注册弹窗：非法 id 提示、无样本时「完成注册」禁用", async () => {
    const user = userEvent.setup();
    renderSpeakerPage();
    await user.click(await screen.findByRole("button", { name: /添加说话人/ }));
    const idInput = await screen.findByLabelText("说话人 ID");
    await user.type(idInput, "张三");
    expect(screen.getByText(/仅允许英文字母、数字、下划线/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /完成注册/ })).toBeDisabled();
    await user.clear(idInput);
    await user.type(idInput, "owner");
    expect(screen.queryByText(/仅允许英文字母、数字、下划线/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /完成注册/ })).toBeDisabled();
  });

  it("下载按钮触发 download_speaker_model，完成后状态刷新为已就绪", async () => {
    const user = userEvent.setup();
    renderSpeakerPage();
    await user.click(await screen.findByRole("button", { name: /下载模型/ }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("download_speaker_model");
    });
    await waitFor(() => {
      expect(screen.getByText("已就绪")).toBeInTheDocument();
    });
  });
});
