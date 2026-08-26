import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/lib/queryClient";
import type { CatalogPage, UnifiedModelItem } from "@/types/catalog";
import type { InstallState, LibraryModel } from "@/types/modelLibrary";

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
  open: vi.fn(() => Promise.resolve(null)),
}));

/** 内置精选（provider=building，走 BuiltinActions 操作区）。 */
function builtinItem(modelId: string, displayName: string): UnifiedModelItem {
  return {
    canonicalKey: `building:${modelId}`,
    modelId,
    provider: "building",
    remote: null,
    builtin: {
      displayName,
      description: `内置描述 ${displayName}`,
      modelType: "llm",
      runtime: "llama.cpp",
      format: "GGUF",
      languages: ["zh"],
      tags: ["qwen3"],
      parameterCount: "0.6B",
      sizeBytes: 396_705_472,
    },
    modelType: "llm",
    compatibility: "compatible",
    compatibilityNotes: null,
    recommendedVariant: null,
    installs: [],
    localSummary: { installedArtifactCount: 0, hasCurrentArtifact: false, activeDownloadCount: 0 },
    confirmed: false,
  };
}

/** 已安装的 managed LibraryModel（BuiltinActions 按 id/repoId 匹配）。 */
function libraryModel(
  id: string,
  displayName: string,
  opts?: { current?: boolean; ownership?: "managed" | "external"; installState?: InstallState },
): LibraryModel {
  return {
    id,
    name: displayName,
    displayName,
    modelType: "llm",
    runtime: "llama.cpp",
    format: "GGUF",
    description: "",
    languages: [],
    tags: [],
    parameterCount: "0.6B",
    quantization: "Q4_K_M",
    version: "instruct",
    sizeBytes: 396_705_472,
    homepage: null,
    downloadable: true,
    source: "registry",
    ownership: opts?.ownership ?? "managed",
    installState: opts?.installState ?? "installed",
    current: opts?.current ?? false,
    runtimeStatus: "inactive",
    localPath: "/home/user/.zapmomo/models/Qwen3-0.6B",
    installedAt: "2026-08-20T00:00:00Z",
    installId: id,
    repoId: null,
    compatibility: "verified",
  };
}

function unifiedItem(
  modelId: string,
  compatibility: UnifiedModelItem["compatibility"] = "compatible",
): UnifiedModelItem {
  return {
    canonicalKey: `huggingface:${modelId.toLowerCase()}`,
    modelId,
    provider: "huggingface",
    remote: {
      repoId: modelId,
      author: "Qwen",
      displayName: modelId.split("/")[1] ?? modelId,
      description: `测试描述 ${modelId}`,
      pipelineTag: "text-generation",
      libraryName: "gguf",
      tags: ["qwen3"],
      downloads: 1000,
      likes: 50,
      trendingScore: null,
      lastModified: "2025-05-20T00:00:00Z",
      createdAt: null,
      license: "apache-2.0",
      languages: ["zh"],
      parameterCount: "4B",
      gated: null,
      private: null,
      sha: null,
    },
    builtin: null,
    modelType: "llm",
    compatibility,
    compatibilityNotes: null,
    recommendedVariant: null,
    installs: [],
    localSummary: { installedArtifactCount: 0, hasCurrentArtifact: false, activeDownloadCount: 0 },
    confirmed: false,
  };
}

let catalogPage: CatalogPage<UnifiedModelItem>;
let libraryModels: LibraryModel[];

function defaultInvoke(cmd: string, args?: Record<string, unknown>) {
  switch (cmd) {
    case "get_app_info":
      return Promise.resolve({ version: "0.1.4", product_name: "ZapMomo" });
    case "list_devices":
      return Promise.resolve(["内置麦克风"]);
    case "get_kws_config":
      return Promise.resolve({ model_dir: "", models_present: false, model_downloading: false });
    case "get_asr_config":
      return Promise.resolve({ model_dir: "", models_present: false, model_downloading: false });
    case "get_tts_config":
      return Promise.resolve({
        model_dir: "",
        models_present: false,
        model_downloading: false,
        enabled: true,
      });
    case "get_llm_config":
      return Promise.resolve({
        model_path: "",
        models_present: false,
        ready: false,
        loaded_model_path: null,
        enabled: false,
        auto_load: false,
        enable_thinking: false,
      });
    case "list_model_library":
      return Promise.resolve(libraryModels);
    case "catalog_search_models": {
      // 支持 category / search 的简单过滤（模拟 HF 服务端）
      const query = args?.query as { category?: string; search?: string };
      let items = catalogPage.items;
      if (query?.category) items = items.filter((i) => i.modelType === query.category);
      if (query?.search) {
        const q = query.search.toLowerCase();
        items = items.filter(
          (i) =>
            i.modelId.toLowerCase().includes(q) ||
            (i.remote?.description ?? "").toLowerCase().includes(q),
        );
      }
      return Promise.resolve({ items, hasMore: false });
    }
    case "catalog_get_model_detail":
      return Promise.resolve({
        repoId: "",
        description: null,
        pipelineTag: null,
        libraryName: null,
        tags: [],
        license: null,
        languages: [],
        downloads: 0,
        likes: 0,
        lastModified: null,
        createdAt: null,
        sha: null,
        gated: null,
        private: null,
        cardData: null,
        siblings: [],
      });
    case "download_snapshot":
      return Promise.resolve([]);
    case "is_listening":
    case "is_asr_listening":
    case "is_tts_synthesizing":
    case "is_llm_ready":
      return Promise.resolve(false);
    case "list_tts_voices":
      return Promise.resolve([]);
    case "get_live2d_config":
      return Promise.resolve({ model_dir: "", models_present: false });
    case "get_microphone":
      return Promise.resolve("");
    default:
      return Promise.resolve(undefined);
  }
}

beforeEach(() => {
  catalogPage = {
    items: [unifiedItem("Qwen/Qwen3-4B-GGUF"), unifiedItem("Qwen/Qwen3-0.6B-GGUF")],
    hasMore: false,
  };
  libraryModels = [];
  queryClient.clear(); // 隔离 React Query 缓存（单例跨测试共享）
  invokeMock.mockReset();
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) =>
    defaultInvoke(cmd, args),
  );
});

describe("LibraryPage", () => {
  it("渲染在线目录（HF 真实数据形态）", async () => {
    render(
      <MemoryRouter initialEntries={["/models/library"]}>
        <App />
      </MemoryRouter>,
    );
    await waitFor(
      () => {
        expect(screen.getByText("Qwen3-4B-GGUF")).toBeInTheDocument();
      },
      { timeout: 3000 },
    );
    expect(screen.getByText("Qwen3-0.6B-GGUF")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "添加本地模型" })).toBeInTheDocument();
    expect(screen.getByText("Hugging Face")).toBeInTheDocument();
  });

  it("搜索触发远程查询（debounce 后）", async () => {
    render(
      <MemoryRouter initialEntries={["/models/library"]}>
        <App />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText("Qwen3-4B-GGUF")).toBeInTheDocument(), {
      timeout: 3000,
    });
    await userEvent.type(screen.getByPlaceholderText("搜索模型名称、描述、标签或作者..."), "0.6B");
    // 等 debounce 后的远程查询：0.6B 出现且 4B 消失（服务端过滤）
    await waitFor(
      () => {
        expect(screen.getByText("Qwen3-0.6B-GGUF")).toBeInTheDocument();
        expect(screen.queryByText("Qwen3-4B-GGUF")).not.toBeInTheDocument();
      },
      { timeout: 3000 },
    );
  });

  it("分类 Tab 存在且切换不崩溃（LLM 已改远程连接，分类入口不再提供）", async () => {
    render(
      <MemoryRouter initialEntries={["/models/library"]}>
        <App />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText("Qwen3-4B-GGUF")).toBeInTheDocument(), {
      timeout: 3000,
    });
    // LLM 分类 tab 已移除（本地 LLM 能力下线）
    expect(screen.queryByRole("button", { name: "LLM" })).not.toBeInTheDocument();
    // 切到 ASR：fixture 均为 llm 类型，被服务端过滤为空，列表展示空态不崩溃
    const asrTab = screen.getByRole("button", { name: "ASR" });
    expect(asrTab).toBeInTheDocument();
    await userEvent.click(asrTab);
    await waitFor(() => {
      expect(screen.queryByText("Qwen3-4B-GGUF")).not.toBeInTheDocument();
    });
  });

  it("空结果显示空状态", async () => {
    catalogPage = { items: [], hasMore: false };
    render(
      <MemoryRouter initialEntries={["/models/library"]}>
        <App />
      </MemoryRouter>,
    );
    await waitFor(
      () => {
        expect(screen.getByText("没有找到符合条件的模型")).toBeInTheDocument();
      },
      { timeout: 3000 },
    );
  });

  it("默认只显示可用模型；打开「显示全部模型」后展示可能兼容/不兼容", async () => {
    catalogPage = {
      items: [
        unifiedItem("Qwen/Qwen3-4B-GGUF", "compatible"),
        unifiedItem("Some/Transformers", "possible"),
        unifiedItem("Some/Whisper", "unsupported"),
      ],
      hasMore: false,
    };
    render(
      <MemoryRouter initialEntries={["/models/library"]}>
        <App />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText("Qwen3-4B-GGUF")).toBeInTheDocument(), {
      timeout: 3000,
    });
    // 默认：只显示可用（compatible），possible / unsupported 项隐藏
    expect(screen.queryByText("Transformers")).not.toBeInTheDocument();
    expect(screen.queryByText("Whisper")).not.toBeInTheDocument();
    // 打开「显示全部模型」→ 所有兼容级别出现
    await userEvent.click(screen.getByText("显示全部模型"));
    await waitFor(() => expect(screen.getByText("Transformers")).toBeInTheDocument(), {
      timeout: 3000,
    });
    expect(screen.getByText("Whisper")).toBeInTheDocument();
  });

  describe("已安装模型卸载", () => {
    async function selectBuiltinInstalled(displayName: string) {
      render(
        <MemoryRouter initialEntries={["/models/library"]}>
          <App />
        </MemoryRouter>,
      );
      await waitFor(() => expect(screen.getByText(displayName)).toBeInTheDocument(), {
        timeout: 3000,
      });
      await userEvent.click(screen.getByText(displayName));
      await waitFor(() => expect(screen.getByText("已安装")).toBeInTheDocument(), {
        timeout: 3000,
      });
    }

    it("已安装内置精选显示卸载按钮；确认后调 delete_model", async () => {
      libraryModels = [libraryModel("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M")];
      catalogPage = {
        items: [builtinItem("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M")],
        hasMore: false,
      };
      await selectBuiltinInstalled("Qwen3 0.6B Instruct Q4_K_M");

      await userEvent.click(screen.getByRole("button", { name: "卸载" }));
      expect(await screen.findByText(/确定要卸载/)).toBeInTheDocument();
      expect(screen.getByText("模型文件将从本地删除。")).toBeInTheDocument();

      await userEvent.click(screen.getByRole("button", { name: "确认卸载" }));

      await waitFor(() => {
        expect(invokeMock).toHaveBeenCalledWith("delete_model", { id: "qwen3-0.6b-q4-k-m" });
      });
    });

    it("确认框取消不调用 delete_model", async () => {
      libraryModels = [libraryModel("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M")];
      catalogPage = {
        items: [builtinItem("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M")],
        hasMore: false,
      };
      await selectBuiltinInstalled("Qwen3 0.6B Instruct Q4_K_M");

      await userEvent.click(screen.getByRole("button", { name: "卸载" }));
      await userEvent.click(await screen.findByRole("button", { name: "取消" }));

      // 确认框带退出动画，DOM 延迟移除，用 waitFor 等待
      await waitFor(() => {
        expect(screen.queryByText(/确定要卸载/)).not.toBeInTheDocument();
      });
      expect(invokeMock).not.toHaveBeenCalledWith("delete_model", expect.anything());
    });

    it("当前使用中的模型不显示卸载按钮", async () => {
      libraryModels = [
        libraryModel("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M", { current: true }),
      ];
      catalogPage = {
        items: [builtinItem("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M")],
        hasMore: false,
      };
      await selectBuiltinInstalled("Qwen3 0.6B Instruct Q4_K_M");

      expect(screen.getByText("已安装")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "卸载" })).not.toBeInTheDocument();
      // 当前模型也不显示「设为当前模型」（本来就是当前）
      expect(screen.queryByRole("button", { name: "设为当前模型" })).not.toBeInTheDocument();
    });

    it("未安装（not_installed 记录）内置精选显示下载而非已安装/卸载", async () => {
      // 回归：list_model_library 对未安装 registry 模型也返回记录，仅按 id 匹配会误判为已安装
      libraryModels = [
        libraryModel("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M", {
          installState: "not_installed",
        }),
      ];
      catalogPage = {
        items: [builtinItem("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M")],
        hasMore: false,
      };
      render(
        <MemoryRouter initialEntries={["/models/library"]}>
          <App />
        </MemoryRouter>,
      );
      await waitFor(
        () => expect(screen.getByText("Qwen3 0.6B Instruct Q4_K_M")).toBeInTheDocument(),
        {
          timeout: 3000,
        },
      );
      await userEvent.click(screen.getByText("Qwen3 0.6B Instruct Q4_K_M"));

      await waitFor(() => {
        expect(screen.getByRole("button", { name: "下载" })).toBeInTheDocument();
      });
      expect(screen.queryByText("已安装")).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "卸载" })).not.toBeInTheDocument();
    });

    it("external 本地导入模型显示「移除」语义（不删原始文件）", async () => {
      libraryModels = [
        libraryModel("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M", {
          ownership: "external",
        }),
      ];
      catalogPage = {
        items: [builtinItem("qwen3-0.6b-q4-k-m", "Qwen3 0.6B Instruct Q4_K_M")],
        hasMore: false,
      };
      await selectBuiltinInstalled("Qwen3 0.6B Instruct Q4_K_M");

      await userEvent.click(screen.getByRole("button", { name: "卸载" }));
      expect(await screen.findByText(/确定要从模型库移除/)).toBeInTheDocument();
      expect(
        screen.getByText("只会取消 ZapMomo 中的登记，不会删除你的原始模型文件。"),
      ).toBeInTheDocument();

      await userEvent.click(screen.getByRole("button", { name: "确认移除" }));
      await waitFor(() => {
        expect(invokeMock).toHaveBeenCalledWith("delete_model", { id: "qwen3-0.6b-q4-k-m" });
      });
    });
  });
});
