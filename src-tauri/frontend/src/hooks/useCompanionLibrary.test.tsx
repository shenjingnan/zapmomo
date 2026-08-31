import { act, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ToastProvider } from "@/components/ui/toast";
import type { CompanionLibraryView } from "@/types/tauri";
import { useCompanionLibrary } from "./useCompanionLibrary";

type SetWakeWord = (id: string, wakeWord: string | null) => Promise<void>;
type SetWelcomeText = (id: string, text: string | null) => Promise<void>;

// listen mock 捕获 handler 以便手动派发事件；unlisten 必须 delete handler，
// 否则「卸载后不刷新」用例假失败（unlisten 空实现会让事件继续命中已卸载组件）。
const { invokeMock, listenMock, eventHandlers } = vi.hoisted(() => {
  const handlers: Record<string, (payload: unknown) => void> = {};
  return {
    invokeMock: vi.fn(),
    listenMock: vi.fn((event: string, cb: (e: { payload: unknown }) => void) => {
      handlers[event] = (payload) => cb({ payload });
      return Promise.resolve(() => {
        delete handlers[event];
      });
    }),
    eventHandlers: handlers,
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

function emit(event: string, payload: unknown) {
  act(() => {
    eventHandlers[event]?.(payload);
  });
}

function listCallCount(): number {
  return invokeMock.mock.calls.filter((c) => c[0] === "list_companions").length;
}

const library: CompanionLibraryView = { models: [], active_model_id: null };

function Probe({
  onSetWakeWord,
  onSetWelcomeText,
}: {
  onSetWakeWord?: (fn: SetWakeWord) => void;
  onSetWelcomeText?: (fn: SetWelcomeText) => void;
}) {
  const { loading, setWakeWord, setWelcomeText } = useCompanionLibrary();
  onSetWakeWord?.(setWakeWord);
  onSetWelcomeText?.(setWelcomeText);
  return <span data-testid="loading">{String(loading)}</span>;
}

beforeEach(() => {
  vi.clearAllMocks();
  invokeMock.mockImplementation((cmd: string) =>
    cmd === "list_companions" ? Promise.resolve(library) : Promise.resolve(undefined),
  );
});

describe("useCompanionLibrary", () => {
  it("挂载拉取伙伴库，live2d-model-changed 触发 refresh", async () => {
    render(
      <ToastProvider>
        <Probe />
      </ToastProvider>,
    );
    await waitFor(() => expect(listCallCount()).toBe(1));
    expect(listenMock).toHaveBeenCalledWith("live2d-model-changed", expect.any(Function));

    emit("live2d-model-changed", { model_dir: null, model_file: null, format: null });
    await waitFor(() => expect(listCallCount()).toBe(2));
  });

  it("卸载后事件不再触发 refresh（取消订阅生效）", async () => {
    const { unmount } = render(
      <ToastProvider>
        <Probe />
      </ToastProvider>,
    );
    await waitFor(() => expect(listCallCount()).toBe(1));
    unmount();
    // unlisten 是 Promise：先冲刷任务队列让清理生效，再派发事件（真实 IPC 中
    // unlisten 必先于后续事件生效；同步 emit 会抢在微任务前造成假失败）。
    await new Promise((r) => setTimeout(r, 0));
    emit("live2d-model-changed", {});
    await new Promise((r) => setTimeout(r, 0));
    expect(listCallCount()).toBe(1);
  });

  it("setWakeWord：调用 set_companion_wake_word 并以返回视图更新库", async () => {
    let captured: SetWakeWord | undefined;
    const updated: CompanionLibraryView = { models: [], active_model_id: null };
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_companions") return Promise.resolve(library);
      if (cmd === "set_companion_wake_word") return Promise.resolve(updated);
      return Promise.resolve(undefined);
    });
    render(
      <ToastProvider>
        <Probe onSetWakeWord={(fn) => (captured = fn)} />
      </ToastProvider>,
    );
    await waitFor(() => expect(listCallCount()).toBe(1));

    await act(async () => {
      await captured?.("companion-a", "小月");
    });
    // 载荷键名钉死 camelCase（wake_word 会被后端静默丢参）。
    expect(invokeMock).toHaveBeenCalledWith("set_companion_wake_word", {
      id: "companion-a",
      wakeWord: "小月",
    });
  });

  it("setWelcomeText：text 传 null 恢复默认模板", async () => {
    let captured: SetWelcomeText | undefined;
    const updated: CompanionLibraryView = { models: [], active_model_id: null };
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_companions") return Promise.resolve(library);
      if (cmd === "set_companion_welcome_text") return Promise.resolve(updated);
      return Promise.resolve(undefined);
    });
    render(
      <ToastProvider>
        <Probe onSetWelcomeText={(fn) => (captured = fn)} />
      </ToastProvider>,
    );
    await waitFor(() => expect(listCallCount()).toBe(1));

    await act(async () => {
      await captured?.("companion-a", null);
    });
    expect(invokeMock).toHaveBeenCalledWith("set_companion_welcome_text", {
      id: "companion-a",
      text: null,
    });
  });
});
