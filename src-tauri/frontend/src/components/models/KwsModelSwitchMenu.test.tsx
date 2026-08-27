import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { KwsModelSwitchMenu } from "./KwsModelSwitchMenu";

// stub 选择唤醒词模型弹窗：只记录 props，避免其内部（toast/invoke）整链依赖。
const { dialogProps } = vi.hoisted(() => ({
  dialogProps: { last: null as { open: boolean; onClose: () => void } | null },
}));

vi.mock("@/components/kws/KwsModelDialog", () => ({
  KwsModelDialog: (props: { open: boolean; onClose: () => void }) => {
    dialogProps.last = props;
    return props.open ? <div data-testid="kws-dialog">选择唤醒词模型弹窗</div> : null;
  },
}));

// mock runtime：kws 切片可变（model_dir）。
const { state, navProbe } = vi.hoisted(() => ({
  state: {
    kws: null as {
      config: { config: { model_dir: string; models_present: boolean } | null } | null;
    } | null,
  },
  // 模拟浏览器原生行为：a 内嵌 button 点击的默认动作是跟随祖先 href；
  // jsdom 不实现该行为，这里以 defaultPrevented 为准计数「原生导航」次数。
  navProbe: { count: 0 },
}));

vi.mock("@/providers/RuntimeContext", () => ({
  useRuntime: () => ({ kws: state.kws }),
}));

function makeKwsConfig(modelDir?: string) {
  state.kws = {
    config: {
      config: {
        model_dir:
          modelDir ?? "/home/user/.zapmomo/models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20",
        models_present: true,
      },
    },
  };
}

/** 挂真实链接行验证「不触发导航」；location 探针放在 /models。 */
function Probe() {
  const location = useLocation();
  return (
    <>
      <div data-testid="location">{location.pathname}</div>
      <a
        href="/models/kws"
        data-testid="row-link"
        onClick={(e) => {
          // 模拟原生「激活祖先 a」：拦截层调用了 preventDefault 则视为已阻止导航。
          if (!e.defaultPrevented) navProbe.count++;
        }}
      >
        <KwsModelSwitchMenu />
      </a>
    </>
  );
}

function renderMenu() {
  return render(
    <MemoryRouter initialEntries={["/models"]}>
      <Routes>
        <Route path="/models" element={<Probe />} />
        <Route path="/models/kws" element={<div>配置页</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  dialogProps.last = null;
  navProbe.count = 0;
  makeKwsConfig();
});

describe("KwsModelSwitchMenu 模型快速切换（弹窗版）", () => {
  it("模型名文本 + 「选择模型」按钮", () => {
    renderMenu();
    expect(screen.getByText("sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20")).toBeInTheDocument();
    const button = screen.getByRole("button", { name: "选择唤醒词模型" });
    expect(button).toHaveTextContent("选择模型");
  });

  it("点击切换按钮打开选择唤醒词模型弹窗", async () => {
    const user = userEvent.setup();
    renderMenu();

    await user.click(screen.getByRole("button", { name: "选择唤醒词模型" }));

    expect(dialogProps.last?.open).toBe(true);
    expect(screen.getByTestId("kws-dialog")).toBeInTheDocument();
  });

  it("弹窗 onClose 回调关闭后可再次打开", async () => {
    const user = userEvent.setup();
    renderMenu();

    await user.click(screen.getByRole("button", { name: "选择唤醒词模型" }));
    expect(dialogProps.last?.open).toBe(true);

    // onClose 触发 setState，等待 stub 以 open=false 重渲染。
    act(() => dialogProps.last?.onClose());
    await waitFor(() => expect(dialogProps.last?.open).toBe(false));

    await user.click(screen.getByRole("button", { name: "选择唤醒词模型" }));
    await waitFor(() => expect(dialogProps.last?.open).toBe(true));
  });

  it("点击行内按钮/弹窗不触发所在行的链接导航（含原生 href 默认行为）", async () => {
    const user = userEvent.setup();
    renderMenu();

    await user.click(screen.getByRole("button", { name: "选择唤醒词模型" }));
    expect(screen.getByTestId("kws-dialog")).toBeInTheDocument();
    expect(screen.getByTestId("location")).toHaveTextContent("/models");
    // 回归：拦截层必须 preventDefault，否则浏览器原生跟随 <a href> 整页跳转。
    expect(navProbe.count).toBe(0);
  });

  it("切换模型后显示新模型目录名", () => {
    makeKwsConfig("/home/user/.zapmomo/models/my-custom-kws-model");
    renderMenu();
    expect(screen.getByText("my-custom-kws-model")).toBeInTheDocument();
  });
});
