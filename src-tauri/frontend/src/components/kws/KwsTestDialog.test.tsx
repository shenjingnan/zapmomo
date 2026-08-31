import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { KwsTestDialog } from "./KwsTestDialog";

const { state } = vi.hoisted(() => ({
  state: {
    isListening: false,
    startArgs: [] as Array<[string | null, string | null]>,
    stopped: 0,
    results: [] as Array<{ id: number; keyword: string; startTime: number; at: string }>,
    sessionKeywords: "",
  },
}));

// mock 用 React state 驱动 isListening，start/stop 触发重渲（贴近真实 useListening 语义）
vi.mock("@/providers/RuntimeContext", async () => {
  const { useState } = await import("react");
  return {
    useRuntime: () => {
      const [isListening, setIsListening] = useState(state.isListening);
      return {
        kws: {
          listening: {
            isListening,
            pending: false,
            error: null,
            start: (device: string | null, keywords: string | null) => {
              state.startArgs.push([device, keywords]);
              state.isListening = true;
              setIsListening(true);
              return Promise.resolve();
            },
            stop: () => {
              state.stopped += 1;
              state.isListening = false;
              setIsListening(false);
              return Promise.resolve();
            },
          },
          results: state.results,
        },
        device: null,
        sessionKeywords: state.sessionKeywords,
      };
    },
  };
});

beforeEach(() => {
  state.isListening = false;
  state.startArgs = [];
  state.stopped = 0;
  state.results = [];
  state.sessionKeywords = "";
});

describe("KwsTestDialog", () => {
  it("打开自动用指定关键词开始监听、展示检测结果、Esc 关闭自动停止", async () => {
    state.results = [{ id: 0, keyword: "月下", startTime: 0.64, at: "12:00:00" }];
    const user = userEvent.setup();
    render(<KwsTestDialog open onClose={() => {}} keywords="月下" />);
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await waitFor(() => {
      expect(state.startArgs).toEqual([[null, "月下"]]);
    });
    expect(await screen.findByText("正在监听")).toBeInTheDocument();
    expect(screen.getByText("“月下”")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(state.stopped).toBe(1);
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });

  it("打开前已在监听：不重复 start，关闭不停止（监听归属外部）", async () => {
    const user = userEvent.setup();
    state.isListening = true;
    render(<KwsTestDialog open onClose={() => {}} />);

    expect(await screen.findByText("正在监听")).toBeInTheDocument();
    expect(state.startArgs).toEqual([]);

    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(state.stopped).toBe(0);
  });

  it("keywords 缺省回退全局自定义唤醒词；标题可自定义", async () => {
    state.sessionKeywords = "全局词";
    render(<KwsTestDialog open onClose={() => {}} title="测试唤醒词 · 大月下" />);

    await waitFor(() => {
      expect(state.startArgs).toEqual([[null, "全局词"]]);
    });
    expect(screen.getByRole("heading", { name: "测试唤醒词 · 大月下" })).toBeInTheDocument();
  });
});
