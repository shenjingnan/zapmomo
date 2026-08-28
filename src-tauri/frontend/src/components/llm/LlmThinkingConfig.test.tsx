import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ToastProvider } from "@/components/ui/toast";
import type { LlmConfigInfo } from "@/types/tauri";
import { LlmThinkingConfig } from "./LlmThinkingConfig";

const { setThinkingParamsMock, useRuntimeMock } = vi.hoisted(() => ({
  setThinkingParamsMock: vi.fn(),
  useRuntimeMock: vi.fn(),
}));

vi.mock("@/providers/RuntimeContext", () => ({
  useRuntime: useRuntimeMock,
}));

function makeConfig(overrides?: Partial<LlmConfigInfo>): LlmConfigInfo {
  return {
    enabled: true,
    provider: "anthropic",
    ready: false,
    settings_path: "/home/user/.zapmomo/settings.toml",
    system_prompt: "",
    params: {
      max_tokens: 512,
      temperature: 0.7,
      top_p: 0.8,
      top_k: 20,
      min_p: 0.05,
      repeat_penalty: 1.05,
      seed: 0,
    },
    base_url: null,
    api_key: null,
    model: "claude-haiku-4-5",
    thinking: false,
    reasoning_effort: null,
    ...overrides,
  };
}

function renderWith(config: LlmConfigInfo) {
  useRuntimeMock.mockReturnValue({
    llm: { config, setThinkingParams: setThinkingParamsMock },
  });
  return render(
    <ToastProvider>
      <LlmThinkingConfig />
    </ToastProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  setThinkingParamsMock.mockResolvedValue(undefined);
});

describe("LlmThinkingConfig", () => {
  it("thinking 关闭时：开关未选、下拉置灰保留所选值", async () => {
    renderWith(makeConfig({ thinking: false, reasoning_effort: "high" }));
    const toggle = screen.getByRole("switch", { name: "启用深度思考" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    const trigger = screen.getByRole("combobox", { name: "推理强度" });
    expect(trigger).toBeDisabled();
    // 置灰但保留所选档位
    expect(withinSelectTrigger(trigger)).toHaveTextContent("高");
    expect(screen.queryByText(/本设置对其不生效/)).not.toBeInTheDocument();
  });

  it("打开开关：调用 setThinkingParams(true) 且下拉解禁", async () => {
    const user = userEvent.setup();
    renderWith(makeConfig({ thinking: false }));
    await waitFor(() => {
      expect(screen.getByRole("switch", { name: "启用深度思考" })).toBeEnabled();
    });
    await user.click(screen.getByRole("switch", { name: "启用深度思考" }));
    await waitFor(() => {
      expect(setThinkingParamsMock).toHaveBeenCalledWith(true, undefined);
    });
    await waitFor(() => {
      expect(screen.getByRole("combobox", { name: "推理强度" })).toBeEnabled();
    });
  });

  it("切换强度：以当前开关状态 + 新档位保存", async () => {
    const user = userEvent.setup();
    renderWith(makeConfig({ thinking: true, reasoning_effort: "medium" }));
    const trigger = await screen.findByRole("combobox", { name: "推理强度" });
    await user.click(trigger);
    await user.click(await screen.findByRole("option", { name: "低（最快）" }));
    await waitFor(() => {
      expect(setThinkingParamsMock).toHaveBeenCalledWith(true, "low");
    });
  });

  it("非 anthropic provider 时显示适用范围提示", () => {
    renderWith(makeConfig({ provider: "openai-compatible", thinking: false }));
    expect(screen.getByText(/当前 provider 为「openai-compatible」/)).toBeInTheDocument();
  });
});

/** Radix SelectTrigger 渲染出的值文本在其内部 span 中 */
function withinSelectTrigger(trigger: HTMLElement): HTMLElement {
  return trigger;
}
