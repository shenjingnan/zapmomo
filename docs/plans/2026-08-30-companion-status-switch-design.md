# 角色伙伴「状态切换」设计（替代右键「表演」）

日期：2026-08-30
状态：已实现

## 背景与目标

角色伙伴右键菜单原有「表演」子菜单（BongoCat 兼容模拟键鼠：敲键盘/玩鼠标/键鼠同动）。
产品决策将其替换为「状态切换」：

- **Live2D 伙伴**：状态 = 动作（motion）。菜单列出模型动作，点击播放一次后自动回待机 idle。
- **角色包伙伴**：状态 = `sprites/` 目录图片（png/gif/webp），状态名 = 文件名 stem。
- **表演功能整体下线**（不共存）；GIF 单文件伙伴无状态目录，不显示子菜单。

## 现状与复用

| 能力 | 位置 | 处置 |
| --- | --- | --- |
| sprites 枚举/校验/通知 | 根 crate `companion_sprites.rs` | 复用（抽 `resolve_and_notify` 共享核心，新增 `apply_menu_switch` 菜单入口） |
| sprite 前端显示 | `CompanionRoot` `spriteOverride` 事件链 | 复用，前端零改动 |
| Live2D motion 播放 | `motionManager.startMotion(group, index, 3 /* FORCE */)` | 复用（dsh 联动同款），新增 `companion-play-motion` 事件 |
| model3.json 解析 | 根 crate `live2d/config.rs` | 新增 `parse_motion_catalog`（纯 JSON 解析，不做绝对路径解析） |
| motion 补注册 | `companion.rs::register_missing_motion_files` | 已有（散动作自动进 `Extra` 组，菜单天然可见） |
| BongoCat 静态展示 | `detect_bongocat` + 前端 `PropsLayer` | 保留（键盘背景仍显示，爪子恒默认贴图） |
| 模拟键鼠表演 | 根 crate `src/performance/` 整模块 + src-tauri 表演运行时 + 前端 `usePerformance` 等 | **删除** |

## 方案

### 菜单结构（按 active 伙伴 format 动态生成）

`build_status_submenu`（src-tauri）替换原 `build_performance_submenu`，右键菜单与托盘菜单共用：

| format | 子菜单内容 | 空时 |
| --- | --- | --- |
| `cubism3`（Live2D，含 BongoCat 格式） | `motion_status_entries`：解析 model3.json `FileReferences.Motions` | 置灰「（无可用动作）」 |
| `character`（角色包） | `sprite_status_entries`：`list_active_sprites()` + 「默认立绘」 | 置灰「（无可用形象）」 |
| `gif` / 无 active | 不显示子菜单（返回 `None`，调用方跳过挂载） | — |

菜单 id 设计：

- `motion_<组下标>_<组内下标>`：**纯数字索引**。组名是模型作者任意字符串（可含 `_`/
  空格/非 ASCII），编码进 id 会有解析歧义；构建与点击解析共用同一份
  `list_active_motions` 结果，索引天然对齐。
- `sprite_<stem>`：`strip_prefix` 取整个剩余串（stem 可含 `_`），与 `companion_set_`
  同构。
- 重名消歧：不同组下同名动作全部附组名后缀（`wave (TapBody)`），风格统一。

### 执行链路

```mermaid
flowchart LR
  subgraph 菜单点击（Rust 主线程）
    A[motion_1_0] --> B[motion_id_from_menu_id]
    C[sprite_happy] --> D[sprite_name_from_menu_id]
  end
  B --> E[重查 list_active_motions<br/>取回组名（越界静默忽略）]
  E --> F["emit_to companion<br/>companion-play-motion {group, index}"]
  F --> G[前端 onCompanionPlayMotion<br/>startMotion group index 3]
  G --> H[播完自动回 idle]
  D --> I[companion_sprites::apply_menu_switch<br/>与 LLM 工具同一校验+通知链]
  I --> J[SpriteEvent → companion-sprite-changed]
  J --> K[前端 spriteOverride 现有链路显示]
```

关键语义：

- **动作是「播一次」**：pixi-live2d-display 播完 motion 自动回 idle 循环，无「当前
  状态」勾选态。菜单点击时重查目录而非信任菜单快照——弹出与点击之间伙伴可能已切换，
  索引越界即静默忽略，天然一致，无需加锁。
- **形象是会话态**：与 LLM `set_character_sprite` 工具完全同一写点（`resolve_and_notify`），
  行为一致（`default` 恢复、大小写不敏感、归属校验）。
- **Rust 枚举下标 = 前端播放下标**：`parse_motion_catalog` 按清单数组逐位枚举，缺
  `File` 的项用占位名保位置（不剔除），否则下标错位导致播放错误动作。空组/非数组组
  跳过（与前端 `buildCatalog` 口径一致；构建/解析两侧共用同一结果，自洽）。

### 表演功能下线范围

删除：

- 根 crate `src/performance/`（`mod.rs`/`rng.rs`/`simulator.rs`/`source.rs`，纯表演
  代码，含全部测试；BongoCat props 探测在 `live2d/config.rs::detect_bongocat`，与之无关）
- src-tauri：`PERFORMANCE` 状态机、`start/stop_performance`、`is_performing` command、
  `perform_*` 菜单分支、`primary_monitor_rect`、`spawn_performance_worker`、
  `start_performance_impl`、`stop_performance_inner/sync` 及全部调用点、
  invoke_handler 三项注册
- 前端：`usePerformance.ts`/`damper.ts`/`paramMapping.ts`/`keyNormalize.ts` 及测试、
  `startPerformance/stopPerformance/isPerforming` API、
  `performance-started/stopped/device-changed` 监听、
  `PerformanceScene/StartedPayload/StoppedPayload/DeviceEventPayload` 类型、
  `Live2dStage` 的 `onParamFrame`/`Live2dStageHandle`/`Live2dParamWriter` 命令式表面

保留与改名：

- `PerformancePropsView`/`PerformanceKeyView` → `BongoCatPropsView`/`BongoCatKeyView`
  （props 静态展示仍需要）
- `PropsLayer` 保留（`pressedKeys` 恒空 = 只显示键盘背景；`PressedKey` 类型本地化）
- dsh 播报动作联动（`pickMotionGroup`）不受影响

## 错误处理与边界

| 场景 | 行为 |
| --- | --- |
| 模型清单畸形 / 读取失败 | `list_active_motions` 降级为空 → 菜单置灰占位，不影响其余菜单 |
| 模型无 `Motions` 键 | `Ok(vec![])`（可选资源语义） |
| 菜单弹出与点击间隙切换伙伴 | 点击时重查目录，索引越界静默忽略 |
| 启动早期（模型未加载）点击动作 | 前端 `modelRef` null 防御跳过（与 dsh 监听同款） |
| 动作文件缺失/损坏 | `startMotion` 返回 false，前端不感知失败（一次性语义无状态残留） |
| sprites 同 stem 冲突 | 既有 png > gif > webp 优先级，不变 |

## 测试

- 根 crate：`parse_motion_catalog`（组/名派生/下标对齐/空组/畸形 JSON/中文）、
  `list_active_motions`（Live2D/角色包/GIF/无 active/畸形清单）、`apply_menu_switch`
  （成功通知/未知名/default/空白名）
- src-tauri：`motion_id_from_menu_id`/`sprite_name_from_menu_id`（含跨命名空间负例）、
  `motion_status_entries`/`sprite_status_entries`（数字 id/重名消歧/默认立绘/空占位）
- 前端：`CompanionRoot.test.tsx` 新增「菜单动作播放」describe（就绪播放/未就绪跳过/
  GIF 跳过），fake 模型桩补 `motionManager.startMotion` spy

## 已知限制

- GIF 单文件伙伴不支持状态切换（无 sprites 目录；`spriteOverride` 前端链路已兼容，
  后续放开 `is_character` 判定即可支持，本期不做）
- Cubism 2（`model.json`）不支持（项目本就不支持该格式）
- 动作为一次性播放：如需「停留该动作」的状态语义，需要 idle 接管逻辑（本期明确不做）
