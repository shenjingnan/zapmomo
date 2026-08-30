# 插件集成页 + dsh 集成插件化（检测 / 一键安装 / 心跳在线）实施方案

## Context（背景与动机）

当前 dsh（deepseek-harness）桥的配置藏在设置页 `DshSection` 里，插件安装完全靠用户手动在终端执行
`dsh plugin --profile web add @zapmomo-ai/dsh-plugin`；装完「有没有装上、dsh 在不在跑、插件活没活」
应用侧一概不知，用户遇到「装了没反应」无从排查。

本方案把 dsh 提升为第一个「集成插件」：

1. 主菜单新增「插件集成」页，DSH 配置从设置页迁入并以集成卡片呈现；
2. 应用自动检测本地 dsh 环境、插件安装/激活状态（纯文件级检测）；
3. 「一键安装」：自动发现 dsh 可执行文件后代跑安装命令；失败时文件选择器兜底（已确认）；
4. 插件新增心跳 `plugin-hello`，前端展示「插件在线」，`dsh web` 未重启/未运行一眼可辨。

链路其余部分（事件 → LLM 文案 → 气泡/TTS → 打断联动）已落地，无需改动。

### 已与用户确认的决策

| 决策点 | 结论 |
| --- | --- |
| 第一版范围 | 完整版：页面迁移 + 环境检测 + 一键安装 + 心跳在线 |
| 「已装依赖但未激活 bundles」半成品态 | 单独展示（提示 + 修复命令） |
| dsh 可执行文件发现失败的兜底 | 文件选择器手动指认（前端 plugin-dialog `open()`），选中后仍代跑安装 |
| 设置页旧区块 | 移除 `DshSection`，不留链接；`[dsh]` TOML 存储不动 |

## 关键现状事实（实测/探索确认，file:line 以主仓库 worktree 为准）

- **dsh 侧结构**：`~/.dsh/`（数据目录）+ `~/.dsh/profiles/web/package.json`（pnpm 包结构，
  `dependencies` + `dsh.profile.bundles`）+ `cordis.patch.yml`。本机实测：插件以 link 模式装好。
- **本机实锤（决定实现方式）**：
  - `which dsh` 在登录 shell 里都找不到 —— dsh 装在 fnm `~/.local/share/fnm/node-versions/v22.22.2/installation/bin/dsh`
    （fnm 按 node 版本隔离全局 bin；v24.19.0 里**没有** dsh）→ 检测锚点必须是文件，不能是 PATH；
  - pnpm（corepack shim `pnpm ⇒ ../lib/node_modules/corepack/dist/pnpm.js`）在 v24.19.0 的 bin，
    v22.22.2 的 bin 里**没有 pnpm** → 代安装 PATH 必须补**所有**发现的 node bin 目录，只补 dsh 所在目录会 pnpm not found。
- **桥（根 crate `zapmomo::dsh`）**：`src/dsh/mod.rs` tiny_http serve（`serve` mod.rs:143、`handle_request` :197：
  405/404/Bearer 鉴权/413/`parse_event` → Ok(Some)→sink→204、**Ok(None) 未知 type→204 前向兼容**、Err→400）；
  `write_discovery` tmp+rename+0600（:39）；`EventThrottle`（:99）。
- **`src/dsh/event.rs`**：`DshEvent` kebab-case tag 枚举（task-started/finished/failed/interrupted），
  手写宽容解析 `parse_event`（:109），未知 type → `Ok(None)`。
- **tauri 侧 `src-tauri/src/lib.rs`**：`DshBridgeState`（:2094，fresh Arc 代际隔离，`is_current_generation` Arc::ptr_eq）；
  `start_dsh_bridge_impl`（:2393）线程内 on_ready/sink 闭包 + epilogue 经 `thread_app.state::<DshBridgeState>()`
  回写（:2459，**心跳回写可照抄此模式**）；`handle_dsh_event`（:2167）节流→LLM worker；
  `DshBridgeStatusPayload`（:2154，on_ready :2433 与 epilogue :2466 两处 emit）；commands
  `get_dsh_config` :2532 / `set_dsh_enabled` :2554 / `set_dsh_params` :2586 / `get_dsh_bridge_status` :2625 /
  `test_dsh_announce` :2640；invoke_handler :5977（dsh 段 :6030-6034）。
- **进度事件模板**：`migrate_storage`（lib.rs:5721-5796）：`async fn` + `tauri::async_runtime::spawn_blocking`
  + `app.emit("storage-migrate-progress", &p)` + cancel AtomicBool + State Guard Drop；前端
  `onStorageMigrateProgress`（lib/tauri.ts:362）+ SettingsPage 订阅（:71-100，biome-ignore 注释手法）。
- **Tauri 线程模型（官方文档确认）**：同步 command 跑主线程 → 长任务必须 `async fn` + `spawn_blocking`。
- **子进程先例**：`src/audiocpp/server.rs`（Windows CREATE_NO_WINDOW、stderr drain）、
  `src/llm/tools.rs`（try_wait + deadline 轮询超时 kill）、`src/audiocpp/locator.rs`（失败返回 `searched` 列表）。
  无 which/glob 依赖，不新增。
- **前端**：`Sidebar.tsx:5-11` `PRIMARY_NAV` 纯数组；`App.tsx:23-36` 直接 import 路由；
  `@tauri-apps/plugin-dialog` 的 `open()` 已在用（SettingsPage:1）→ 文件选择器无需新 Rust command；
  `App.test.tsx:196-200` 是唯一导航文本断言处，其 invokeMock 未知 command **默认 resolve undefined**
  → 新页面必须容忍 undefined（沿用 `if (!info) return null`）。
- **插件** `integrations/dsh-plugin/src/index.ts`：`post()` fire-and-forget（1s 超时、异常吞掉、
  发送前现读发现文件）；`apply(ctx)` 注册 session/event。
- **CI 盲区**：CI 只测根 crate；tauri crate 改动必须本地 `cargo clippy -p zapmomo-app -- -D warnings`；
  前端门禁 = `tsc -b` + vitest；biome 只对改动文件跑（main 预存错误）。

## 实施方案

> 实施开始时先把本方案按用户惯例存一份到项目根目录（`docs/plans/2026-08-30-dsh-integration-page-design.md`）。

### Phase 1 —— 根 crate：检测纯逻辑 + 可执行文件发现（可独立验证：cargo test）

**新增 `src/dsh/integration.rs`**（`mod.rs` 挂 `pub mod integration; pub mod discover;`）：

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DshIntegrationStatus {
    pub dsh_home_detected: bool,   // ~/.dsh/ 存在
    pub profile_ready: bool,       // ~/.dsh/profiles/web/package.json 存在
    pub plugin_installed: bool,    // dependencies 含 "@zapmomo-ai/dsh-plugin"（link/git/npm 值均算）
    pub plugin_activated: bool,    // dsh.profile.bundles 数组含该包名
}
pub const PLUGIN_PACKAGE: &str = "@zapmomo-ai/dsh-plugin";
pub const MANUAL_COMMAND: &str = "dsh plugin --profile web add @zapmomo-ai/dsh-plugin";
pub fn detect(dsh_home: &Path) -> DshIntegrationStatus
```

- `detect` 只读文件系统：`~/.dsh` 目录存在性 → `profiles/web/package.json` 存在性 → serde_json 解析
  `dependencies`（对象键存在即 installed，值不限）与 `dsh.profile.bundles`（数组含包名即 activated）；
  解析失败 → installed/activated=false（半成品态可见性优先）。package.json 缺失但 profile 目录在 → profile_ready 按 package.json 判。
- 测试（tempdir 传 `dsh_home`，无需 temp-home）：全无 / 仅 home / home+profile 未装 / 半成品（installed 未
  activated / activated 未 installed 两种反向）/ 全装（npm 值与 link 值各一）。

**新增 `src/dsh/discover.rs`**：

```rust
pub struct DshDiscoveryError { pub searched: Vec<String> }   // 对齐 locator.rs 的 searched 诊断
pub fn find_dsh_executable(home: &Path) -> Result<PathBuf, DshDiscoveryError>
```

- 候选目录（存在才扫）：fnm `~/.local/share/fnm/node-versions/*/installation/bin`（**全部版本目录逐个探测**，
  按版本号数值感知降序，取含 `dsh` 的最高版本）、nvm `~/.nvm/versions/node/*/bin`、volta `~/.volta/bin`、
  homebrew `/opt/homebrew/bin` `/usr/local/bin`、`~/.local/bin`、`PATH`（`env::split_paths`，对齐 locator.rs:82）。
  Windows 追加 `%APPDATA%\npm`（`dsh.cmd`，spawn 需 `cmd /c`，标注待实测）。
- 找到后轻量验证：spawn `<dsh> --version`，try_wait 轮询 5s deadline（llm/tools.rs 手法），失败视为不可用继续找；
  全 miss → `DshDiscoveryError { searched }`。
- 测试：tempdir 伪造 fnm 目录树（多个版本、含/不含 dsh、坏 symlink），断言选中最高可用版本与 searched 列表。

### Phase 2 —— tauri crate：心跳 + 两个 command（可独立验证：clippy -p zapmomo-app）

**心跳（选最小改动路径：复用 `parse_event` 的前向兼容通道，不动 serve/handle_request 签名与既有测试）**：

1. `src/dsh/event.rs`：`DshEvent` 增 fieldless 变体 `PluginHello`，`kind()` 返回 `"plugin-hello"`，
   `parse_event` 增 match 臂 `"plugin-hello" => Ok(Some(DshEvent::PluginHello))`；doc 注明这是控制事件、
   不进播报管线。测试：parse plugin-hello body（含多余 session_id 字段）→ Some(PluginHello)。
2. `lib.rs` `DshBridgeState` 增 `last_heartbeat: Arc<Mutex<Option<i64>>>`（epoch ms；`new()` 创建、
   `start_dsh_bridge_impl` 开头清 `None`——新一代不继承旧心跳）。
3. `handle_dsh_event`（:2167）**最前**加分支：`PluginHello` → 经 `app.state::<DshBridgeState>()` 写
   `last_heartbeat = now_epoch_ms`，**且以 `is_current_generation(&running)` 防护**（sink 闭包增捕
   `running` 的 Arc 克隆，照抄 epilogue :2459-2473 手法）→ emit 一次 `dsh-bridge-status`；直接 return，
   不进节流/LLM/播报/历史。并发测试不适用（tauri 层），以代码评审覆盖。
4. `DshBridgeStatusPayload` 增 `last_heartbeat_at: Option<i64>`；两处 emit 点（on_ready/epilogue）补 `None`，
   心跳分支 emit 时带当前值；`get_dsh_bridge_status`（:2625）读 state 组装。`start_dsh_bridge_impl` 清空时同步。

**新 command `detect_dsh_integration`（同步 fn，纯文件读，亚毫秒级可跑主线程）**：
返回 `DshIntegrationStatus` + `manual_command`（`zapmomo::dsh::integration::detect(dirs home/.dsh)`）。

**新 command `install_dsh_plugin(path: Option<String>)`（`async fn` + `spawn_blocking`，照抄 migrate_storage :5721）**：

- `DshInstallState { running: Arc<AtomicBool> }` + Guard Drop（照抄 StorageMigrateState :5608-5632）防并发双击；
  running 中再调 → Err。
- spawn_blocking 内：① `path` 为 None → `find_dsh_executable`；`NotFound` → emit 终态 failed（带 searched）
  并 `Err(结构化 json)`（前端转手动模式：文件选择器 + 手动命令复制）。② spawn `<dsh> plugin --profile web
  add @zapmomo-ai/dsh-plugin`：**参数写死无注入面**；`PATH` 前插 dsh 所在目录 + **全部发现的 node bin 目录**
  （fnm v22 有 dsh、v24 有 pnpm 的本机实锤）；stdout/stderr 逐行 emit `dsh-install-progress`
  `{ state: "discovering"|"installing"|"done"|"failed", message }`（照抄 storage 模式）；120s deadline
  try_wait 轮询超时 kill（llm/tools.rs 手法）；退出码非 0 → failed（message 带尾部 stderr）。
- 成功后 emit done；前端重跑 `detect_dsh_integration` 刷新状态并提示「重启 dsh web 生效」。
- `invoke_handler`（:6030-6034 附近）注册两个 command。

### Phase 3 —— 插件心跳（integrations/dsh-plugin）

- `src/index.ts` `apply(ctx)`：开头 `post('plugin-hello', 'plugin')` + `const t = setInterval(() =>
  post('plugin-hello', 'plugin'), 15_000); t.unref?.()`（防阻塞宿主退出）；复用 `post()` 全部纪律。
- `package.json` version `0.1.0 → 0.2.0`（发布仍走 `dsh-plugin-v*` tag 流程，由维护者推送）。

### Phase 4 —— 前端：插件集成页（可独立验证：tsc -b + vitest）

1. **纯逻辑** `src-tauri/frontend/src/lib/dshIntegration.ts`（+`.test.ts`，对齐 dshMotion.ts 模式）：
   `ONLINE_WINDOW_MS = 45_000`；`composeIntegrationState(status, bridge, nowMs) -> "no-dsh" | "no-profile" |
   "not-installed" | "half-activated" | "awaiting-restart" | "online"`（heartbeat 新鲜度 =
   `last_heartbeat_at != null && now - last < 45s`；bridge 未启用时 online 不可达，卡片高亮「启用 dsh 桥」开关）。
2. **类型与 api**：`types/tauri.ts` 增 `DshIntegrationInfo`、`DshInstallProgress`；`DshBridgeStatus` 增
   `last_heartbeat_at: number | null`；`lib/tauri.ts` 增 `detectDshIntegration` / `installDshPlugin` /
   `onDshInstallProgress`（对齐 :362 手法）。
3. **页面**：`Sidebar.tsx` `PRIMARY_NAV` 增 `{ to: "/integrations", icon: Puzzle, label: "插件集成" }`
   （「模型」与「设置」之间）；`App.tsx` 增 `<Route path="integrations" element={<IntegrationsPage />} />`。
4. **组件** `src-tauri/frontend/src/components/integrations/`：
   - `IntegrationsPage.tsx`：页面骨架（对齐 SettingsPage 卡片风格）+ 挂 DshIntegrationCard；
   - `DshIntegrationCard.tsx`：卡片壳（图标/名称/描述/状态徽章）+ 原 DshSection 的 4 开关与测试播报
     **逻辑原样搬运**（含 `data-testid="dsh-bridge-error"` 与 status 事件合并）+ 状态机操作区：
     `not-installed`→「一键安装」；安装中显示进度（订阅 `dsh-install-progress`）；failed 带 searched 时
     转「选择 dsh 可执行文件」（`open({ multiple: false })` → `installDshPlugin({ path })`）+ 手动命令复制；
     `half-activated`→展示修复命令（MANUAL_COMMAND）复制；`awaiting-restart`→「重启 dsh web 生效」提示；
     `online`→在线徽章 + 端口。卡片内 `setInterval` 15s 重算新鲜度（心跳过期无事件，靠本地时钟翻转）。
5. **设置页瘦身**：`SettingsPage.tsx` 移除 `DshSection` 挂载与 import；删除 `settings/DshSection.tsx` +
   `DshSection.test.tsx`（用例迁移进 `DshIntegrationCard.test.tsx`：沿用其 mock 手法——mock
   `@tauri-apps/api/core` 的 invoke、`@tauri-apps/api/event` 的 listen、`useToast`；补状态机各态、安装流、
   文件选择器兜底、心跳新鲜度（注入 now）；`App.test.tsx` invokeMock 补新 command mock 臂 + 导航断言加「插件集成」）。

### Phase 5 —— 文档

`docs/content/docs/desktop-app/dsh-bridge.mdx`：安装章节改为「应用内插件集成页一键安装」优先、
终端命令保留为手动路径；「设置页」表述全改「插件集成页」；补在线状态/心跳说明。

## 测试与验证

```bash
# 根 crate（CI 同口径；worktree 惯例：--manifest-path + CARGO_TARGET_DIR 指向共享 target）
cargo fmt --check
cargo clippy -- -D warnings          # 不带 --all-targets（main 预存测试 lint）
cargo test

# tauri crate（CI 盲区，必须本地自查）
cargo clippy -p zapmomo-app -- -D warnings

# 前端（worktree 首次需 pnpm install --prefer-offline）
cd src-tauri/frontend && pnpm tsc -b && pnpm vitest run
pnpm biome check <改动的 ts/tsx 文件>   # 只跑改动文件
```

**E2E 手动验收**（本机有真实 dsh 环境）：`pnpm tauri dev` →
1. 「插件集成」出现在导航，DSH 卡片显示 `online` 或 `awaiting-restart`（本机已装插件）；
2. `dsh web` 跑一个任务 → 桌宠气泡+播报（回归不破坏）；
3. 退出 `dsh web` → 45s 内卡片翻为 `awaiting-restart`；
4. 「测试播报」与 4 个开关在卡片内功能如旧；
5. 安装流（可选，不卸载现有插件的前提下跳过实测或用临时 profile 验证 spawn 链路）。

## 风险与待实测

| 风险 | 缓解 |
| --- | --- |
| `dsh plugin add` 内部走 pnpm，GUI 环境 PATH 残缺（本机实锤 pnpm 在另一 node 版本目录） | PATH 补全部发现的 node bin 目录；失败 message 带 searched + stderr 尾部；手动命令复制兜底 |
| 安装需访问 npm registry（网络失败） | 进度流透传 stderr，failed 态展示原因 |
| Windows `dsh.cmd` spawn 需 `cmd /c`、路径表未实测 | 首版 macOS 优先，Windows 候选目录已列、标注待实测 |
| dsh 未跑过 `web`（无 web profile） | 状态机单列 `no-profile` 态，引导先启动一次 `dsh web` |
| CI 不测 src-tauri / 前端 | 门禁清单本地全跑（见上） |
| 分离旧线程迟到心跳污染新桥 | 心跳写入与 emit 以 `is_current_generation` 防护（照抄 epilogue） |

## 不做（YAGNI）

卸载/禁用插件（`dsh plugin remove`）、多 profile 支持（仅 web）、通用集成注册表机制（页面留扩展位）、
安装取消按钮（120s 自动 kill 足够）、桥关闭时的心跳补发。
