import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ToastProvider } from "@/components/ui/toast";
import type { CompanionDragMode, CompanionLibraryView, CompanionModelInfo } from "@/types/tauri";
import { CompanionPage } from "./CompanionPage";

type StageCatalog = import("@/components/live2d/previewManager").Live2dCatalog;
const { invokeMock, openMock, stageHandleMock, stageState, configState } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openMock: vi.fn(),
  stageHandleMock: {
    playMotion: vi.fn(async () => true),
    applyExpression: vi.fn(async () => true),
    resetExpression: vi.fn(),
  },
  /** 供 mock 替身注入目录的可变容器（vi.mock 工厂只可靠引用 hoisted 变量）。 */
  stageState: { catalog: null as StageCatalog | null },
  /** get_live2d_config 的 click_through / locked / drag_mode 覆盖值（null = 后端未返回该字段）。 */
  configState: {
    clickThrough: null as boolean | null,
    locked: null as boolean | null,
    dragMode: null as CompanionDragMode | null,
    /** get_live2d_config 返回的有效缩放（模拟「active 伙伴私有 ?? 全局」合并结果）。 */
    windowScale: 1.0,
  },
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: openMock,
}));

// SharedLive2dStage 依赖 pixi / WebGL，jsdom 无法运行；预览容器量测（ResizeObserver）在
// jsdom 是空桩、尺寸保持 0，本组件不会真正渲染 stage。这里 mock 成可注入目录与句柄的替身：
// 挂载时注入外部可控的 stageState.catalog，并把自己的命令式句柄挂到传入的 ref 上。
vi.mock("@/components/live2d/SharedLive2dStage", async () => {
  const { useEffect } = await import("react");
  return {
    SharedLive2dStage: ({
      onModelCatalog,
      ref,
    }: {
      onModelCatalog?: (c: StageCatalog | null) => void;
      ref?: { current: unknown };
    }) => {
      useEffect(() => {
        onModelCatalog?.(stageState.catalog);
        if (ref) ref.current = stageHandleMock;
      }, [onModelCatalog, ref]);
      return <div data-testid="live2d-stage" />;
    },
  };
});

function model(id: string, name: string, valid = true): CompanionModelInfo {
  return {
    id,
    name,
    source_path: `/src/${name}`,
    model_dir: `/zap/.zapmomo/companions/${id}`,
    model_file: `/zap/.zapmomo/companions/${id}/${name}.model3.json`,
    format: "cubism3",
    imported_at: "2026-01-01T00:00:00Z",
    valid,
    cover_image: null,
    has_persona: false,
    has_voice: false,
  };
}

const MODEL_A = model("companion-aaaa", "大月下");
const MODEL_B = model("companion-bbbb", "星语");

/** 可变伙伴库快照（模拟后端 list/import/set_active 语义）。 */
let library: CompanionLibraryView;
/** import_companion mock 用序号生成唯一 id。 */
let importSeq: number;

beforeEach(() => {
  invokeMock.mockReset();
  openMock.mockReset();
  stageHandleMock.playMotion.mockReset();
  stageHandleMock.applyExpression.mockReset();
  stageHandleMock.resetExpression.mockReset();
  stageState.catalog = null;
  configState.clickThrough = null;
  configState.locked = null;
  configState.dragMode = null;
  configState.windowScale = 1.0;
  library = { models: [], active_model_id: null };
  importSeq = 0;

  invokeMock.mockImplementation(
    (cmd: string, args?: { source?: string; id?: string; name?: string }) => {
      switch (cmd) {
        case "list_companions":
          return Promise.resolve(library);
        case "get_live2d_config":
          return Promise.resolve({
            model_dir: null,
            model_file: null,
            format: null,
            models_present: false,
            window_scale: configState.windowScale,
            window_opacity: 1.0,
            click_through: configState.clickThrough,
            window_layer: "front",
            locked: configState.locked,
            drag_mode: configState.dragMode,
            settings_path: "/zap/.zapmomo/settings.toml",
          });
        case "import_companion": {
          const source = args?.source ?? "";
          const isGif = source.endsWith(".gif");
          const base = source.split("/").pop() ?? "模型";
          const name = isGif ? base.replace(/\.gif$/i, "") : base;
          const existing = library.models.find((m) => m.source_path === source);
          if (existing) {
            return Promise.resolve({ library, model_id: existing.id, already_imported: true });
          }
          const id = `companion-import-${++importSeq}`;
          const first = library.models.length === 0;
          // GIF 源生成 format=gif 伙伴（与后端 import_gif_from_file 语义一致）。
          const imported: CompanionModelInfo = {
            ...model(id, name),
            source_path: source,
            format: isGif ? "gif" : "cubism3",
            model_file: isGif
              ? `/zap/.zapmomo/companions/${id}/${base}`
              : `/zap/.zapmomo/companions/${id}/${name}.model3.json`,
          };
          library = {
            models: [...library.models, imported],
            // 首次导入自动 active（与后端 import_from_dir 语义一致）。
            active_model_id: first ? id : library.active_model_id,
          };
          return Promise.resolve({ library, model_id: id, already_imported: false });
        }
        case "set_active_companion": {
          library = { ...library, active_model_id: args?.id ?? null };
          return Promise.resolve(library);
        }
        case "rename_companion": {
          library = {
            ...library,
            models: library.models.map((m) =>
              m.id === args?.id ? { ...m, name: args?.name ?? m.name } : m,
            ),
          };
          return Promise.resolve(library);
        }
        case "remove_companion": {
          const id = args?.id ?? "";
          const remaining = library.models.filter((m) => m.id !== id);
          library = {
            models: remaining,
            // 删的是 active → 落到第一个剩余或 null（与后端语义一致）。
            active_model_id:
              library.active_model_id === id ? (remaining[0]?.id ?? null) : library.active_model_id,
          };
          return Promise.resolve(library);
        }
        default:
          return Promise.resolve(undefined);
      }
    },
  );
});

/**
 * 覆盖 vitest.setup 的空 ResizeObserver 桩：observe 后**异步**报告非零 contentRect
 * （真实 ResizeObserver 回调在渲染帧后触发，须异步——同步触发会违反 React/Radix
 * 渲染时序导致 SliderThumbProvider 崩溃）。使预览容器量测产生非 0 尺寸 →
 * `showStage` 成立 → 舞台替身被挂载（面板才能渲染）。仅本文件生效。
 */
function enablePreviewResize() {
  class FakeResizeObserver implements ResizeObserver {
    constructor(private cb: ResizeObserverCallback) {}
    observe(target: Element): void {
      setTimeout(() => {
        const rect = {
          x: 0,
          y: 0,
          width: 300,
          height: 200,
          top: 0,
          right: 300,
          bottom: 200,
          left: 0,
        };
        // Radix react-use-size 读 contentBoxSize[0].inlineSize/blockSize，
        // 空数组会让它崩；按真实浏览器结构填充。
        const sizes = [{ inlineSize: 300, blockSize: 200 }];
        const entry = {
          target,
          contentRect: rect,
          borderBoxSize: sizes,
          contentBoxSize: sizes,
          devicePixelContentBoxSize: sizes,
        };
        this.cb([entry as unknown as ResizeObserverEntry], this);
      }, 0);
    }
    unobserve(): void {}
    disconnect(): void {}
  }
  window.ResizeObserver = FakeResizeObserver;
}

function renderPage() {
  enablePreviewResize();
  return render(
    <ToastProvider>
      <CompanionPage />
    </ToastProvider>,
  );
}

describe("CompanionPage 伙伴模型管理器", () => {
  it("选中伙伴后显示尺寸控制，拖动滑块调用 set_companion_scale", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: /大月下.*使用中/ });
    // 初始从 get_live2d_config 读到 window_scale=1.0 → 100%（异步等待出现）。
    expect(await screen.findByText("尺寸")).toBeInTheDocument();
    // 尺寸与透明度两个滑块初始都是 100%。
    expect(await screen.findAllByText("100%")).toHaveLength(2);

    // 键盘微调滑块（Radix Slider role="slider"）：每次步进 5。
    const slider = screen.getByRole("slider", { name: "尺寸" });
    slider.focus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_scale", { scale: expect.any(Number) });
    });
  });

  it("选中伙伴后显示透明度控制，拖动滑块调用 set_companion_opacity", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: /大月下.*使用中/ });
    expect(await screen.findByText("透明度")).toBeInTheDocument();
    expect(await screen.findAllByText("100%")).toHaveLength(2);

    const slider = screen.getByRole("slider", { name: "透明度" });
    slider.focus();
    await user.keyboard("{ArrowLeft}"); // 100 → 95
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_opacity", {
        opacity: expect.any(Number),
      });
    });
  });

  it("切换显示层级开关调用 set_companion_layer（置顶→置底）", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: /大月下.*使用中/ });
    // 初始默认置顶，开关为开。
    const toggle = await screen.findByRole("switch", { name: "置顶" });
    expect(toggle).toBeChecked();
    expect(screen.getByText(/置顶：悬浮在所有窗口之上/)).toBeInTheDocument();

    // 关闭开关 → 置底。
    await user.click(toggle);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_layer", { layer: "back" });
    });
    expect(await screen.findByText(/置底：沉到所有窗口之下/)).toBeInTheDocument();
  });

  it("空库时显示空态", async () => {
    renderPage();
    expect(await screen.findByText("还没有伙伴")).toBeInTheDocument();
    expect(screen.getAllByText("暂无伙伴").length).toBeGreaterThanOrEqual(1);
    // 顶部有主导入入口（左侧底部按钮已移除）
    expect(screen.getByRole("button", { name: "导入模型 / 角色包" })).toBeInTheDocument();
  });

  it("列出伙伴；默认选中 active；点击其他伙伴仅切换预览不切换 active", async () => {
    library = { models: [MODEL_A, MODEL_B], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    // 默认 selected = active（大月下）：列表项有「使用中」徽标。
    expect(await screen.findByText("使用中")).toBeInTheDocument();

    // 点击星语 → 预览切换为「设为当前使用」，active 仍是 大月下。
    await user.click(screen.getByRole("button", { name: MODEL_B.name }));
    expect(screen.getByRole("button", { name: "设为当前使用" })).toBeEnabled();
    // 蓝色选中边框落到星语（行容器上）。
    expect(screen.getByTestId(`companion-item-${MODEL_B.id}`)).toHaveClass("border-primary/60");
    // 「使用中」徽标仍在大月下列表项上（只有 1 个）。
    expect(screen.getAllByText("使用中")).toHaveLength(1);
    expect(screen.getByTestId(`companion-item-${MODEL_A.id}`)).toHaveTextContent("使用中");
  });

  it("设为当前使用：点击后 active 持久化到后端并更新 Badge", async () => {
    library = { models: [MODEL_A, MODEL_B], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: MODEL_B.name }));
    await user.click(screen.getByRole("button", { name: "设为当前使用" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_active_companion", { id: MODEL_B.id });
    });
    // active 变为星语：右侧 CTA 消失（已是当前使用），「使用中」徽标落到星语。
    await waitFor(() => {
      expect(screen.getByTestId(`companion-item-${MODEL_B.id}`)).toHaveTextContent("使用中");
    });
    expect(screen.queryByRole("button", { name: "设为当前使用" })).not.toBeInTheDocument();
    expect(screen.getAllByText("使用中")).toHaveLength(1);
  });

  it("设为当前使用后尺寸滑杆刷新为新伙伴的缩放", async () => {
    library = { models: [MODEL_A, MODEL_B], active_model_id: MODEL_A.id };
    // 后端视角：A 的有效缩放 0.5 → 滑杆 50%。
    configState.windowScale = 0.5;
    const user = userEvent.setup();
    renderPage();

    expect(await screen.findByText("50%")).toBeInTheDocument();

    // 后端视角：B 的有效缩放 1.5；把 B 设为当前使用后滑杆刷新为 150%。
    configState.windowScale = 1.5;
    await user.click(screen.getByRole("button", { name: MODEL_B.name }));
    await user.click(screen.getByRole("button", { name: "设为当前使用" }));

    expect(await screen.findByText("150%")).toBeInTheDocument();
    expect(screen.queryByText("50%")).not.toBeInTheDocument();
  });

  it("首次导入：自动 selected + active，右侧直接显示「当前使用」", async () => {
    openMock.mockResolvedValue("/Downloads/星语");
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: "导入模型 / 角色包" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("import_companion", {
        source: "/Downloads/星语",
      });
    });
    expect(await screen.findByText("✓ 已导入「星语」")).toBeInTheDocument();
    // 首次导入自动 active：新伙伴带「使用中」徽标，右侧无 CTA。
    expect(await screen.findByText("使用中")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "设为当前使用" })).not.toBeInTheDocument();
  });

  it("导入 GIF：gif 过滤器文件选择器 + format=gif 伙伴以 img 预览（无动作面板）", async () => {
    openMock.mockResolvedValue("/Downloads/舞.gif");
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: "导入 GIF" }));

    await waitFor(() => {
      expect(openMock).toHaveBeenCalledWith(
        expect.objectContaining({
          filters: [{ name: "GIF 动图", extensions: ["gif"] }],
        }),
      );
      expect(invokeMock).toHaveBeenCalledWith("import_companion", {
        source: "/Downloads/舞.gif",
      });
    });
    // GIF 伙伴选中（首次导入自动选中）：预览为 img 而非 Live2D 舞台。
    expect(await screen.findByAltText("舞")).toBeInTheDocument();
    expect(screen.queryByTestId("live2d-stage")).not.toBeInTheDocument();
    // 动作/表情目录面板不渲染（GIF 无动作概念）。
    expect(screen.queryByTestId("motion-catalog")).not.toBeInTheDocument();
  });

  it("第二次导入（已有 active）：selected 变新模型，active 不变", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    openMock.mockResolvedValue("/Downloads/星语");
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: "导入模型 / 角色包" }));

    // selected = 新模型（星语），active 仍是大月下 → 预览显示「设为当前使用」。
    expect(await screen.findByRole("button", { name: "设为当前使用" })).toBeEnabled();
    // 「使用中」徽标仍在大月下（列表项）上。
    expect(screen.getAllByText("使用中")).toHaveLength(1);
    expect(screen.getByTestId(`companion-item-${MODEL_A.id}`)).toHaveTextContent("使用中");
  });

  it("重复导入同一目录：提示已导入，不新增伙伴", async () => {
    openMock.mockResolvedValue("/Downloads/星语");
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: "导入模型 / 角色包" }));
    await screen.findByText("✓ 已导入「星语」");

    await user.click(screen.getByRole("button", { name: "导入模型 / 角色包" }));
    expect(await screen.findByText("该伙伴已经导入")).toBeInTheDocument();
    expect(library.models).toHaveLength(1);
  });

  it("模型不可用：列表显示「模型不可用」，预览提示无法加载，禁止设为当前", async () => {
    const broken = model("companion-broken", "莉娅", false);
    library = { models: [broken], active_model_id: null };
    const user = userEvent.setup();
    renderPage();

    expect(await screen.findByText("模型不可用")).toBeInTheDocument();
    // 列表项选择按钮的可访问名 = 「莉娅 模型不可用」（重命名铅笔的 aria-label 也含「莉娅」，
    // 需用更具体的正则区分）。
    await user.click(screen.getByRole("button", { name: /莉娅.*模型不可用/ }));
    expect(screen.getByText("无法加载该 Live2D 模型")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "设为当前使用" })).toBeDisabled();
  });

  it("取消目录选择时不导入", async () => {
    openMock.mockResolvedValue(null);
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: "导入模型 / 角色包" }));
    await waitFor(() => {
      expect(openMock).toHaveBeenCalled();
    });
    expect(invokeMock).not.toHaveBeenCalledWith("import_companion", expect.anything());
  });

  it("重命名伙伴：铅笔编辑后调用 rename_companion 并更新列表", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    // active 模型的选择按钮可访问名 = 「大月下 使用中」。
    await screen.findByRole("button", { name: /大月下.*使用中/ });
    // 铅笔（hover 图标始终在 DOM）→ 行内输入框。
    await user.click(screen.getByRole("button", { name: `重命名「${MODEL_A.name}」` }));
    const input = await screen.findByDisplayValue(MODEL_A.name);
    await user.clear(input);
    await user.type(input, "大月下改名");
    await user.keyboard("{Enter}");

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("rename_companion", {
        id: MODEL_A.id,
        name: "大月下改名",
      });
    });
    expect(await screen.findByText("✓ 已重命名为「大月下改名」")).toBeInTheDocument();
    // 列表更新为新名字（徽标不变，仍使用中）。
    expect(screen.getByTestId(`companion-item-${MODEL_A.id}`)).toHaveTextContent("大月下改名");
    expect(screen.getByTestId(`companion-item-${MODEL_A.id}`)).toHaveTextContent("使用中");
  });

  it("有封面图时列表显示封面，无封面图用占位图标", async () => {
    const withCover = {
      ...MODEL_A,
      cover_image: "/zap/.zapmomo/companions/companion-aaaa/preview.png",
    };
    library = { models: [withCover, MODEL_B], active_model_id: null };
    // 封面图 alt="" 属装饰性图片，无障碍树不带 img role，用 container 直接查 img 元素。
    const { container } = renderPage();

    await screen.findByRole("button", { name: MODEL_B.name });
    const imgs = container.querySelectorAll("img");
    expect(imgs).toHaveLength(1);
    expect(imgs[0]).toHaveAttribute("src", expect.stringContaining("asset://localhost"));
  });

  it("移除伙伴：垃圾桶 → 确认对话框 → remove_companion，列表移除且 active 不变", async () => {
    library = { models: [MODEL_A, MODEL_B], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: /大月下.*使用中/ });
    // 移除非使用中的星语。
    await user.click(screen.getByRole("button", { name: `移除「${MODEL_B.name}」` }));
    const dialog = screen.getByRole("dialog", { name: "移除伙伴" });
    expect(dialog).toBeInTheDocument();
    expect(within(dialog).getByText(/确定要移除/)).toBeInTheDocument();
    expect(within(dialog).getByText("星语")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "移除" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("remove_companion", { id: MODEL_B.id });
    });
    // 确认后弹窗应真正关闭（LibraryDialog open→false 会退出动画后卸载）。
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "移除伙伴" })).not.toBeInTheDocument();
    });
    // 星语从列表消失，大月下仍是使用中。
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: MODEL_B.name })).not.toBeInTheDocument();
    });
    expect(await screen.findByText("✓ 已移除「星语」")).toBeInTheDocument();
    expect(screen.getByTestId(`companion-item-${MODEL_A.id}`)).toHaveTextContent("使用中");
    expect(screen.getAllByText("使用中")).toHaveLength(1);
  });

  it("使用中的伙伴删除按钮被禁用，点击不弹确认框", async () => {
    library = { models: [MODEL_A, MODEL_B], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: /大月下.*使用中/ });
    // 大月下（使用中）删除按钮禁用；星语可用。
    expect(screen.getByRole("button", { name: `移除「${MODEL_A.name}」` })).toBeDisabled();
    expect(screen.getByRole("button", { name: `移除「${MODEL_B.name}」` })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: `移除「${MODEL_A.name}」` }));
    expect(screen.queryByRole("dialog", { name: "移除伙伴" })).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("remove_companion", expect.anything());
  });

  it("移除时点取消，不调用 remove_companion", async () => {
    library = { models: [MODEL_A, MODEL_B], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: /大月下.*使用中/ });
    await user.click(screen.getByRole("button", { name: `移除「${MODEL_B.name}」` }));
    await user.click(screen.getByRole("button", { name: "取消" }));

    expect(invokeMock).not.toHaveBeenCalledWith("remove_companion", expect.anything());
    // 取消后弹窗应真正关闭（不再残留空内容弹窗）。
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "移除伙伴" })).not.toBeInTheDocument();
    });
  });

  it("重命名按 Escape 取消，不调用后端", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: /大月下.*使用中/ });
    await user.click(screen.getByRole("button", { name: `重命名「${MODEL_A.name}」` }));
    await user.keyboard("{Escape}");

    expect(invokeMock).not.toHaveBeenCalledWith("rename_companion", expect.anything());
  });

  it("展示动作与表情目录：点击动作播放、点击表情应用、重置恢复", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    stageState.catalog = {
      motionGroups: [
        {
          group: "Extra",
          motions: [
            { index: 0, name: "睡觉动画" },
            { index: 1, name: "循环动画" },
          ],
        },
      ],
      expressions: [
        { index: 0, name: "03 生气" },
        { index: 1, name: "07 星星眼" },
      ],
    };
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("tab", { name: "动作" }));
    await user.click(screen.getByRole("button", { name: "播放动作 睡觉动画" }));
    expect(stageHandleMock.playMotion).toHaveBeenCalledWith("Extra", 0);

    await user.click(screen.getByRole("tab", { name: "表情" }));
    await user.click(screen.getByRole("button", { name: "应用表情 07 星星眼" }));
    expect(stageHandleMock.applyExpression).toHaveBeenCalledWith(1);
    await user.click(screen.getByRole("button", { name: "重置表情" }));
    expect(stageHandleMock.resetExpression).toHaveBeenCalledTimes(1);
  });

  it("模型没有动作与表情时显示空态；目录未就绪时不渲染面板", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    stageState.catalog = { motionGroups: [], expressions: [] };
    renderPage();
    // 舞台替身挂载（showStage 成立）是面板渲染的前提；等它出现再断言空态。
    expect(await screen.findByTestId("live2d-stage")).toBeInTheDocument();
    expect(await screen.findByText("此模型未提供动作或表情")).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "动作" })).not.toBeInTheDocument();
  });

  it("点击穿透开关默认关闭，点击后调用 set_companion_click_through 开启", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    const toggle = await screen.findByRole("switch", { name: "点击穿透" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await user.click(toggle);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_click_through", { enabled: true });
    });
  });

  it("点击穿透开关从配置恢复为开启，再点击则关闭", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    configState.clickThrough = true;
    const user = userEvent.setup();
    renderPage();

    const toggle = await screen.findByRole("switch", { name: "点击穿透" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    await user.click(toggle);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_click_through", { enabled: false });
    });
  });

  it("未选中伙伴时点击穿透开关仍然可见（窗口级行为）", async () => {
    library = { models: [], active_model_id: null };
    renderPage();

    expect(await screen.findByRole("switch", { name: "点击穿透" })).toBeInTheDocument();
    expect(screen.queryByText("尺寸")).not.toBeInTheDocument();
  });

  it("锁定位置开关默认关闭，点击后调用 set_companion_locked 开启", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    const toggle = await screen.findByRole("switch", { name: "锁定位置" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await user.click(toggle);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_locked", { enabled: true });
    });
  });

  it("锁定位置开关从配置恢复为开启，再点击则关闭", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    configState.locked = true;
    const user = userEvent.setup();
    renderPage();

    const toggle = await screen.findByRole("switch", { name: "锁定位置" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    await user.click(toggle);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_locked", { enabled: false });
    });
  });

  it("未选中伙伴时锁定位置开关仍然可见（窗口级行为）", async () => {
    library = { models: [], active_model_id: null };
    renderPage();

    expect(await screen.findByRole("switch", { name: "锁定位置" })).toBeInTheDocument();
  });

  it("修饰键拖动开关默认关闭，点击后调用 set_companion_drag_mode 切到 modifier", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    const user = userEvent.setup();
    renderPage();

    const toggle = await screen.findByRole("switch", { name: "修饰键拖动" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await user.click(toggle);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_drag_mode", { mode: "modifier" });
    });
  });

  it("修饰键拖动开关从配置恢复为开启，再点击切回 direct", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    configState.dragMode = "modifier";
    const user = userEvent.setup();
    renderPage();

    const toggle = await screen.findByRole("switch", { name: "修饰键拖动" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    await user.click(toggle);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("set_companion_drag_mode", { mode: "direct" });
    });
  });

  it("未选中伙伴时修饰键拖动开关仍然可见（窗口级行为）", async () => {
    library = { models: [], active_model_id: null };
    renderPage();

    expect(await screen.findByRole("switch", { name: "修饰键拖动" })).toBeInTheDocument();
  });

  it("角色包伙伴：静态立绘走 img 预览（无动作面板），列表显示人设/音色徽标", async () => {
    const furina: CompanionModelInfo = {
      ...model("companion-furina", "芙宁娜"),
      format: "character",
      model_file: "/zap/.zapmomo/companions/companion-furina/character.png",
      has_persona: true,
      has_voice: true,
    };
    library = { models: [furina], active_model_id: furina.id };
    renderPage();

    // 预览为 img（静态立绘）而非 Live2D 舞台。
    expect(await screen.findByAltText("芙宁娜")).toBeInTheDocument();
    expect(screen.queryByTestId("live2d-stage")).not.toBeInTheDocument();
    expect(screen.queryByTestId("motion-catalog")).not.toBeInTheDocument();
    // 列表项显示人设/音色徽标。
    const item = screen.getByTestId(`companion-item-${furina.id}`);
    expect(item).toHaveTextContent("人设");
    expect(item).toHaveTextContent("音色");
  });

  it("普通 Live2D 伙伴不显示人设/音色徽标", async () => {
    library = { models: [MODEL_A], active_model_id: MODEL_A.id };
    renderPage();

    const item = await screen.findByTestId(`companion-item-${MODEL_A.id}`);
    expect(item).not.toHaveTextContent("人设");
    expect(item).not.toHaveTextContent("音色");
  });
});
