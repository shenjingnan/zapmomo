# 角色窗口智能穿透技术方案（SMART_CLICK_THROUGH_DESIGN）

> 状态：已评审通过，随本分支实施
> 日期：2026-08-28
> 关联代码：`src/companion_click_through.rs`（新增，纯逻辑）、`src-tauri/src/lib.rs`、`src-tauri/frontend/src/components/live2d/modelLayout.ts`、`src-tauri/frontend/src/lib/companionHitRegion.ts`（新增）

---

## 1. 背景与目标

### 1.1 背景

companion 角色窗口是透明置顶窗口（Live2D 模型 / GIF 立绘）。窗口的 alpha 通道不参与
OS 命中测试——对系统来说它就是一个不透明矩形，鼠标落在矩形内就归该窗口。因此**角色
画面之外的透明区域会挡住下层窗口的点击**。

现状只有「点击穿透」手动开关（`[live2d].click_through`，设置页/托盘/右键菜单），
是全窗口二值的：开了整个角色不可交互（拖动/缩放/右键全部失效），关了透明区挡下层，
二选一。

### 1.2 业界方案对照（为什么选区域级动态穿透）

| 路线 | 机制 | 适用 | 结论 |
|---|---|---|---|
| OS 级逐像素命中 | Windows `WS_EX_LAYERED`+`UpdateLayeredWindow`（alpha=0 像素自动穿透）；X11 `XShape` 输入区域；WPF/Unity 桌宠即此 | 要求应用自己产出带 alpha 的最终位图 | WebView 合成器不暴露像素 alpha 给 OS，**走不通**（tauri#13070 未实现） |
| 区域级动态穿透 | 全局光标位置判定 + 整窗穿透动态开关（Electron `setIgnoreMouseEvents(forward)` / 桌宠生态标准做法） | WebView 架构 | **本方案采用** |
| 手动开关兜底 | 托盘/设置二值切换 | 所有桌宠均保留 | 现有 `click_through` 语义升级为「强制穿透」保留 |

macOS 无窗口级逐像素命中 API（`NSWindow` 只能整窗 `ignoresMouseEvents`），三平台统一走
区域级方案反而消除了平台分支。

### 1.3 目标

- 光标落在角色不透明区域上时：窗口接收鼠标（拖动/滚轮缩放/右键菜单不变）；
- 光标在其余位置（含顶部 72px 条带、GIF letterbox、模型间隙）：窗口穿透，下层可点；
- 现有「点击穿透」升级为**强制穿透**（优先级最高，兜底不变）；新增**智能穿透**开关，
  **默认开启**；入口：设置页 + 托盘/右键菜单；
- 智能穿透关闭时行为与现状完全一致（整窗可交互）。

## 2. 现状分析（穿透写点的问题）

穿透状态当前有**四个写入点**，存在竞争与 bug：

| 写入点 | 位置 | 问题 |
|---|---|---|
| `apply_companion_click_through` | lib.rs `window.set_ignore_cursor_events(enabled)` | 手动开关路径 |
| macOS 层级 Front 分支 | `panel.set_ignores_mouse_events(false)` 硬编码 | **现有 bug**：手动穿透开启时切换置底↔置顶，穿透被静默清除但 settings 仍为 true，直到重启才恢复 |
| macOS 层级 Back 分支 | `panel.set_ignores_mouse_events(true)` | 同上 |
| Windows 层级分支 | `window.set_ignore_cursor_events(!front)` | 与手动开关双写竞争 |

本方案把这些全部收敛到**单一权威写点** `sync_companion_ignore_cursor_events`：
任何输入变化（开关/层级/显隐/光标）都重算目标值，值不变时跳过系统调用。
tauri 的 `window.set_ignore_cursor_events` 与 tauri-nspanel 的 `set_ignores_mouse_events`
打到同一个 `NSWindow` selector，companion 窗口转 panel 后前者依然有效（现有代码已双用），
故单一写点可全平台无 `#[cfg]` 统一。

## 3. 坐标推导（全链路，含 macOS 混合 DPI 陷阱）

```
前端上报 rects      ：窗口内逻辑像素，原点 = 窗口左上角（stage 坐标 + BUBBLE_STRIP 72px 顶带偏移）
Rust 轮询每 tick    ：
  cursor  = app.cursor_position()            // 全局物理像素（OS 直读，无主线程往返）
  origin  = window.outer_position()          // 窗口外框左上角，全局物理像素（Moved 事件缓存）
  local   = cursor − origin                  // 窗口内物理像素
  clamp   : local 超出窗口外框 ± EXIT_MARGIN（物理）→ 直接判 no-hit
  logical = local / window.scale_factor()    // 窗口内逻辑像素 → 点查 rects
```

**macOS 混合 DPI 陷阱**：`cursor_position()` 的实现按**主显示器** scale 折算
（tao `NSEvent.mouseLocation` + `CGDisplay::main().pixels_high()` 翻转），
`outer_position()` 按**窗口所在屏** scale。两式相减时 `pixels_high` 常数项抵消，
单屏下 `local = scale × 真实偏移` 精确成立；混布不同 DPI 显示器时差值失真，
光标在另一屏可能误判命中 → 用「窗口盒 clamp」防御（超窗即 no-hit，结果仍正确）。

Windows `GetCursorPos` / `GetWindowRect` 同为左上原点物理像素，无此问题。

## 4. 状态机（单一权威决策）

### 4.1 决策函数

```rust
desired_ignore_cursor_events(policy, cursor_hit, holding) -> bool  // true = 穿透
```

优先级（自上而下，先命中先生效）：

| # | 条件 | 结果 | 说明 |
|---|---|---|---|
| 1 | 窗口不可见 | 穿透 | 无副作用，恢复显示即重算 |
| 2 | `layer == Back`（置底） | 穿透 | 现状不变，无条件 |
| 3 | 强制穿透（`click_through`） | 穿透 | 用户最高优先级，兜底 |
| 4a | 智能开 + holding（拖动/菜单保护期） | 可交互 | 防拖动/菜单中途被切穿 |
| 4b | 智能开 + `cursor_hit` | 可交互 | 光标在角色区域上 |
| 4c | 智能开 + 未命中 | 穿透 | 透明区让路 |
| 5 | 智能关 | 可交互 | = 现状行为 |

### 4.2 命中判定（三态区域 + 迟滞）

| region 状态 | 语义 | 判定 |
|---|---|---|
| `None` | 前端未就绪（启动/加载中/加载失败） | **fail-open 判命中**（角色永远可点，不会丢桌宠） |
| `Some([])` | 清屏（模型卸载） | 判未命中（穿透） |
| `Some(rects)` | 就绪 | 当前已穿透 → 任一 rect 外扩 `ENTER_MARGIN` 含点即命中；未穿透 → `EXIT_MARGIN`（外扩更大形成迟滞带，防边缘抖动） |

### 4.3 hold（保护期）

- **拖动保护**：`WindowEvent::Moved` 持续触发期间顺延 `hold_until = now + DRAG_HOLD_MS`；
  `startDragging` 的系统拖动循环一旦被中途切穿透会直接打断，这是此类实现翻车头号原因；
- **菜单保护**：`show_companion_menu` 置 `hold_until = now + MENU_HOLD_MS`
  （原生菜单无关闭回调，只能定时；Windows 弹出菜单还会模态阻塞 dispatcher，线程停顿无损害）。

### 4.4 参数

| 参数 | 值 | 说明 |
|---|---|---|
| `ENTER_MARGIN_PX` | 10 逻辑 px | 进入阈值外扩；同时覆盖 Live2D 呼吸等小幅姿态外溢 |
| `EXIT_MARGIN_PX` | 24 逻辑 px | 离开阈值，> 进入阈值形成迟滞 |
| `TICK_MS` | 33（≈30Hz） | 每 tick 仅 1 次 OS 直读 + 线程本地计算 |
| `DRAG_HOLD_MS` | 600 | 最后一次 Moved 后保持可交互的时长 |
| `MENU_HOLD_MS` | 4000 | 右键菜单保护期 |
| `OPACITY_MIN`（前端） | 0.05 | drawable 透明度过滤（隐藏换装部件不产生幽灵命中区；不用 DynamicFlagIsVisible——眨眼会抖） |
| `MAX_HIT_RECTS`（前端） | 32 | 按面积降序截断，限制 payload 与点查成本 |
| 上报去抖（前端） | 150ms | resize/scale 连续变化合并 |

## 5. 数据流与模块划分

```
前端 companion 窗口                          Rust
┌──────────────────────────────┐   ┌─────────────────────────────────────┐
│ Live2dStage → modelLayout     │   │ CompanionPointerState (.manage)      │
│   computeModelHitRects()      │   │  policy / region / origin / scale /  │
│   (NaN 防线 + opacity 过滤)    │   │  last_move_at / hold_until / ignore  │
│      ↓ 150ms 去抖             │   │            ↑                          │
│ companionHitRegion.ts ────────┼───→ set_companion_hit_region           │
│   (stage→窗口坐标 +72, GIF)   │   │            ↑                          │
│ GIF: gifContainRect()         │   │ sync_companion_ignore_cursor_events   │
└──────────────────────────────┘   │   ← 唯一 set_ignore_cursor_events 写点 │
                                    │            ↑                          │
                                    │ 30Hz watcher 线程（StopSignal 模式）   │
                                    │ 纯逻辑 zapmomo::companion_click_through│
                                    │ （根 crate → CI cargo test 直接覆盖）  │
                                    └─────────────────────────────────────┘
```

**纯逻辑放根 crate** 的原因：workspace `default-members` 只含根 crate，CI 的
`cargo test` 不编译 src-tauri 内嵌测试（历史上平台门控代码 0 覆盖）；
`CompanionWindowLayer` 等依赖类型本就在根 crate settings.rs。

## 6. 协议

| 项 | 定义 |
|---|---|
| command | `set_companion_hit_region(rects: Vec<HitRect>)`（窗口内逻辑像素；负值/非有限 clamp 为 0） |
| command | `set_companion_smart_click_through(enabled: bool)`（五段式：load→mutate→save→emit→rebuild_tray→sync） |
| event | `companion-smart-click-through-changed`（payload `bool`） |
| settings | `[live2d].smart_click_through: Option<bool>`（缺省 **true**）；`click_through` 语义升级为强制穿透 |
| config 读 | `Live2dConfigInfo.smart_click_through: Option<bool>` |
| 前端 api | `setCompanionHitRegion` / `setCompanionSmartClickThrough` / `onCompanionSmartClickThroughChanged` |
| capability | 无需改动（自定义命令不受 ACL 约束） |

## 7. 行为变化说明（有意为之）

- 智能模式下**不能再从窗口任意空白处拖动**，只能抓角色本体（含右键/滚轮缩放同样
  收敛到角色区域）。需要全窗交互时：托盘/设置关掉智能穿透，或开强制穿透走另一极端。
- 手动穿透文案升级为「强制穿透」，与智能穿透的优先级在 UI 描述中写明。

## 8. 分阶段实施与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| 1 | 根 crate 纯函数模块（含内嵌测试）+ settings 字段 + 单一写点重构（删 4 个旧写点）+ 设置页/托盘开关 | `cargo test` / `cargo clippy -p zapmomo-app -- -D warnings` / `tsc -b` / `biome` 全绿；smart 关闭时行为与现状一致；macOS 置底↔置顶往返不再丢手动穿透 |
| 2 | 前端 hit rects 计算（modelLayout.ts）+ 窗口坐标装配（companionHitRegion.ts）+ 上报 effect + 协议 | vitest 全绿；dev 真机确认加载/缩放/清屏各上报一次，数值含 +72 偏移 |
| 3 | Rust 轮询线程 + 事件缓存 + hold 接入 | 真机：光标在角色上可拖动/缩放/右键，移开下层可点；快速甩动不中断；顶带/letterbox/间隙穿透；置底全穿透；挂机 CPU 无增量 |
| 4 | 前端测试补齐 + 全量门禁 | `cargo fmt --check && cargo clippy -p zapmomo-app -- -D warnings && cargo test` + `pnpm tsc -b && pnpm biome check . && pnpm test:run` |

## 9. 明确不做

- 逐像素 alpha 掩码 / per-pixel hit test（`NSWindow` shape / Win32 region）：复杂度与
  收益不成正比，区域级即业界通行精度；
- 点摸交互（`autoInteract:false` 是有意关闭的，另案）；
- 动画逐帧刷新区域（30-60Hz IPC 不可接受，`ENTER_MARGIN` 覆盖小幅姿态变化）；
- Linux 置底补齐；改 bubble 窗口自驱动穿透（语义独立）；
- companion 前端直接调 `setIgnoreCursorEvents`（Rust 是唯一写点，避免双写竞争）。
