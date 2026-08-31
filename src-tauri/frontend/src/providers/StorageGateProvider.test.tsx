import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ToastProvider } from "@/components/ui/toast";
import type { StoragePrompt } from "@/types/modelLibrary";
import { StorageGateProvider, useStorageGate } from "./StorageGateProvider";

const { invokeMock, openMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openMock: vi.fn(),
}));

/** 事件处理器缓存：测试可向后端事件订阅者推事件。 */
const listenHandlers = new Map<string, (payload: unknown) => void>();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: { payload: unknown }) => void) => {
    listenHandlers.set(event, (payload) => cb({ payload }));
    return Promise.resolve(() => {});
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: openMock,
}));

/** 探针组件：点击按钮触发 ensureStorageReady（times 次），结果上报给 onResult。 */
function GateProbe({
  id,
  times = 1,
  onResult,
}: {
  id: string;
  times?: number;
  onResult: (v: boolean) => void;
}) {
  const gate = useStorageGate();
  return (
    <button
      type="button"
      onClick={() => {
        for (let i = 0; i < times; i++) {
          void gate.ensureStorageReady().then(onResult);
        }
      }}
    >
      {id}
    </button>
  );
}

function renderGate(
  onResult: (v: boolean) => void,
  probes: { id: string; times?: number }[] = [{ id: "go" }],
) {
  return render(
    <ToastProvider>
      <StorageGateProvider>
        {probes.map(({ id, times }) => (
          <GateProbe key={id} id={id} times={times} onResult={onResult} />
        ))}
      </StorageGateProvider>
    </ToastProvider>,
  );
}

function promptInfo(overrides: Partial<StoragePrompt> = {}): StoragePrompt {
  return {
    promptRecommended: true,
    defaultDir: "/home/u/.zapmomo",
    modelsDir: "/home/u/.zapmomo/models",
    companionsDir: "/home/u/.zapmomo/companions",
    suggestedDir: "/data/ZapMomo",
    suggestedAvailable: 500 * 1024 * 1024 * 1024,
    defaultAvailable: 20 * 1024 * 1024 * 1024,
    ...overrides,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
  openMock.mockReset();
  listenHandlers.clear();
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "get_storage_prompt":
        return Promise.resolve(promptInfo());
      case "acknowledge_storage_prompt":
      case "set_data_dir":
        return Promise.resolve();
      default:
        return Promise.resolve();
    }
  });
});

describe("StorageGateProvider", () => {
  it("无需引导时直接放行且不写标记", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "get_storage_prompt"
        ? Promise.resolve(promptInfo({ promptRecommended: false }))
        : Promise.resolve(),
    );
    const results: boolean[] = [];
    const user = userEvent.setup();
    renderGate((v) => results.push(v));

    await user.click(screen.getByRole("button", { name: "go" }));
    await waitFor(() => expect(results).toEqual([true]));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("acknowledge_storage_prompt");
  });

  it("弹窗后「使用默认位置」写标记并放行", async () => {
    const results: boolean[] = [];
    const user = userEvent.setup();
    renderGate((v) => results.push(v));

    await user.click(screen.getByRole("button", { name: "go" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("选择模型存储位置")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "使用默认位置" }));
    await waitFor(() => expect(results).toEqual([true]));
    expect(invokeMock).toHaveBeenCalledWith("acknowledge_storage_prompt");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("「选择其他位置」走 set_data_dir → acknowledge → 放行", async () => {
    openMock.mockResolvedValue("/data/ZapMomo");
    const results: boolean[] = [];
    const user = userEvent.setup();
    renderGate((v) => results.push(v));

    await user.click(screen.getByRole("button", { name: "go" }));
    await screen.findByRole("dialog");
    await user.click(screen.getByRole("button", { name: "选择其他位置…" }));

    await waitFor(() => expect(results).toEqual([true]));
    expect(openMock).toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledWith("set_data_dir", { path: "/data/ZapMomo" });
    expect(invokeMock).toHaveBeenCalledWith("acknowledge_storage_prompt");
  });

  it("「取消」放行结果为 false 且不写标记", async () => {
    const results: boolean[] = [];
    const user = userEvent.setup();
    renderGate((v) => results.push(v));

    await user.click(screen.getByRole("button", { name: "go" }));
    await screen.findByRole("dialog");
    await user.click(screen.getByRole("button", { name: "取消" }));

    await waitFor(() => expect(results).toEqual([false]));
    expect(invokeMock).not.toHaveBeenCalledWith("acknowledge_storage_prompt");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("查询失败时 fail-open 放行", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "get_storage_prompt" ? Promise.reject(new Error("boom")) : Promise.resolve(),
    );
    const results: boolean[] = [];
    const user = userEvent.setup();
    renderGate((v) => results.push(v));

    await user.click(screen.getByRole("button", { name: "go" }));
    await waitFor(() => expect(results).toEqual([true]));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("并发触发只查一次、只弹一次", async () => {
    const results: boolean[] = [];
    const user = userEvent.setup();
    // 单按钮连点两次 ensureStorageReady：第二次应命中 inflight 去重
    renderGate((v) => results.push(v), [{ id: "go", times: 2 }]);

    await user.click(screen.getByRole("button", { name: "go" }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.filter(([c]) => c === "get_storage_prompt")).toHaveLength(1),
    );

    await user.click(await screen.findByRole("button", { name: "使用默认位置" }));
    await waitFor(() => expect(results).toEqual([true, true]));
  });

  it("storage-dir-changed 后缓存失效重查", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "get_storage_prompt"
        ? Promise.resolve(promptInfo({ promptRecommended: false }))
        : Promise.resolve(),
    );
    const results: boolean[] = [];
    const user = userEvent.setup();
    renderGate((v) => results.push(v));

    // 第一次：无需引导 → 缓存就绪
    await user.click(screen.getByRole("button", { name: "go" }));
    await waitFor(() => expect(results).toEqual([true]));

    // 目录变更事件 → 缓存失效 → 第二次重查
    listenHandlers.get("storage-dir-changed")?.(null);
    await user.click(screen.getByRole("button", { name: "go" }));
    await waitFor(() => expect(results).toEqual([true, true]));

    const promptCalls = invokeMock.mock.calls.filter(([c]) => c === "get_storage_prompt");
    expect(promptCalls).toHaveLength(2);
  });
});
