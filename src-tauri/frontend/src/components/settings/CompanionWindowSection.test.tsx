import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CompanionDragMode, CompanionWindowLayer } from "@/types/tauri";
import { CompanionWindowSection } from "./CompanionWindowSection";

const { apiMock, configState } = vi.hoisted(() => ({
  apiMock: {
    getLive2dConfig: vi.fn(),
    setCompanionLayer: vi.fn(async () => undefined),
    setCompanionClickThrough: vi.fn(async () => undefined),
    setCompanionLocked: vi.fn(async () => undefined),
    setCompanionDragMode: vi.fn(async () => undefined),
  },
  /** get_live2d_config 的窗口级字段覆盖值（null = 后端未返回该字段，兜底为关/置顶/direct）。 */
  configState: {
    clickThrough: null as boolean | null,
    locked: null as boolean | null,
    dragMode: null as CompanionDragMode | null,
    layer: "front" as CompanionWindowLayer | null,
  },
}));

vi.mock("@/lib/tauri", () => ({
  api: apiMock,
}));

beforeEach(() => {
  configState.clickThrough = null;
  configState.locked = null;
  configState.dragMode = null;
  configState.layer = "front";
  for (const fn of Object.values(apiMock)) fn.mockClear();
  apiMock.getLive2dConfig.mockImplementation(async () => ({
    click_through: configState.clickThrough,
    window_layer: configState.layer,
    locked: configState.locked,
    drag_mode: configState.dragMode,
  }));
});

describe("CompanionWindowSection（显示层级）", () => {
  it("默认置顶开关为开，点击后置底", async () => {
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "置顶" });
    expect(toggle).toBeChecked();
    expect(screen.getByText(/置顶：悬浮在所有窗口之上/)).toBeInTheDocument();

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionLayer).toHaveBeenCalledWith({ layer: "back" });
    });
    expect(await screen.findByText(/置底：沉到所有窗口之下/)).toBeInTheDocument();
  });

  it("配置恢复为置底时开关为关，点击后置顶", async () => {
    configState.layer = "back";
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "置顶" });
    expect(toggle).not.toBeChecked();

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionLayer).toHaveBeenCalledWith({ layer: "front" });
    });
  });
});

describe("CompanionWindowSection（点击穿透）", () => {
  it("默认关闭，点击后调用 setCompanionClickThrough 开启", async () => {
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "点击穿透" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionClickThrough).toHaveBeenCalledWith({ enabled: true });
    });
  });

  it("从配置恢复为开启，再点击则关闭", async () => {
    configState.clickThrough = true;
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "点击穿透" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionClickThrough).toHaveBeenCalledWith({ enabled: false });
    });
  });
});

describe("CompanionWindowSection（锁定位置）", () => {
  it("默认关闭，点击后调用 setCompanionLocked 开启", async () => {
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "锁定位置" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionLocked).toHaveBeenCalledWith({ enabled: true });
    });
  });

  it("从配置恢复为开启，再点击则关闭", async () => {
    configState.locked = true;
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "锁定位置" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionLocked).toHaveBeenCalledWith({ enabled: false });
    });
  });
});

describe("CompanionWindowSection（修饰键拖动）", () => {
  it("默认关闭，点击后调用 setCompanionDragMode 切到 modifier", async () => {
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "修饰键拖动" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionDragMode).toHaveBeenCalledWith({ mode: "modifier" });
    });
  });

  it("从配置恢复为开启，再点击切回 direct", async () => {
    configState.dragMode = "modifier";
    const user = userEvent.setup();
    render(<CompanionWindowSection />);

    const toggle = await screen.findByRole("switch", { name: "修饰键拖动" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    await user.click(toggle);
    await waitFor(() => {
      expect(apiMock.setCompanionDragMode).toHaveBeenCalledWith({ mode: "direct" });
    });
  });
});
