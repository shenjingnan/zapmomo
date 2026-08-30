# 设计：dsh 桥启停跟随插件安装状态（去掉手动开关）

日期：2026-08-30
状态：已确认（brainstorming 两段过审）

## 背景与动机

插件集成页的 dsh 集成卡片目前有一个「启用 dsh 桥」手动开关（`settings.dsh.enabled`），
与插件安装状态相互独立：插件装着但开关关着 → 桥停；插件没装但开关开着 → 桥空转监听。
两条独立状态轴带来了额外的文案引导（「开启下方启用 dsh 桥」）和用户心智负担。

结论：桥的生命周期直接绑定插件安装状态，删除手动开关——**安装了就启用，卸载了就禁用**。
想停联动 = 卸载插件（UI 提供「卸载插件」入口）。

## 行为语义

- 桥运行 ⇔ `~/.dsh/profiles/web/package.json` 的 `dsh.profile.bundles` 含
  `@zapmomo-ai/dsh-plugin`（即检测状态机的 `plugin_activated`）。
- `settings.dsh.enabled` 字段删除；旧 settings 文件中残留的 `enabled = false` 键被
  serde 忽略、自然作废。**已确认接受**：旧版本手动关过桥但插件还装着的用户，升级后桥自动打开。
- 触发点仅两处，不做文件监听：
  1. app 启动时判定 `plugin_activated` ⇒ 自动启动桥；
  2. 卡片内安装完成 ⇒ 自动启动桥；卡片内卸载完成 ⇒ 自动停止桥。
  用户在终端自行 add/remove 的，下次启动 app 生效。

## 后端改动（Rust）

1. `src/dsh/config.rs`：`DshConfig` 删 `enabled` 字段，`resolve()` 同步；
   `src/config/settings.rs` 的 dsh 段结构体删 `enabled`。
2. `src-tauri/src/lib.rs` 启动门控（setup 钩子）：`resolve(...).enabled` 改为
   `detect_integration().plugin_activated`（已激活 ⇒ dsh 环境必然存在）。
3. 删 `set_dsh_enabled` 命令及注册。
4. `run_dsh_plugin_install` 参数化子命令（`add` / `remove`），复用定位 dsh、PATH
   补全、进度事件流、超时 kill 全套机器；新增 `uninstall_dsh_plugin` 命令，跑
   `dsh plugin --profile web remove @zapmomo-ai/dsh-plugin`，进度照走
   `dsh-install-progress` 事件。
5. 安装成功路径自动启动桥、卸载成功路径自动停止桥（阻塞实现内持 AppHandle 可达状态）。
6. `DshConfigInfo` 删 `enabled`。

## 前端改动（DshIntegrationCard）

1. 删「启用 dsh 桥」开关行；三个子开关（事件语音播报 / LLM 播报文案 / 写入对话记录）
   保留，禁用条件由状态机推导：仅 `awaiting-restart` / `online`（插件已激活）可用。
2. 新增「卸载插件」按钮：与「测试播报」同排，仅已安装状态
   （`half-activated` / `awaiting-restart` / `online`）显示；走现有安装进度流 UI。
   不加二次确认——误卸载后「一键安装」可秒回。
3. `awaiting-restart` 文案不再引导开开关，统一为「插件已就绪，启动 dsh web 即自动上线」；
   toast「dsh 桥已开启/关闭」删除。
4. `api.setDshEnabled` 删除，`api.uninstallDshPlugin` 新增；类型同步。

## 测试与验收

- `src/dsh/config.rs` enabled 用例改写；`tests/` 集成测试 `dsh.enabled` 字面量同步。
- 前端：删「启用 dsh 桥」开关用例，新增卸载用例（点击 → `uninstall_dsh_plugin` →
  进度事件 → 完成重检）；门禁 `tsc -b` + `vitest` + 改动文件 biome。
- Rust 门禁：`cargo fmt --check` + CI 口径 clippy + 全量 `cargo test`。

验收清单：

- 装着插件启动 app：桥自动运行，dsh web 在跑时卡片「在线」。
- 点「卸载插件」：进度流走完 → 桥停止 → 卡片「插件未安装」。
- 未装插件启动：桥端口不监听。
- 旧 settings 带 `enabled = false`：升级后桥自动开。
