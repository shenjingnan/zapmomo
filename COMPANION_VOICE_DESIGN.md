# 每角色独立音色（Per-Companion Voice）技术方案

> 状态：已评审通过（2026-08-29）。本文档是实施的基准，实施过程中如有偏差需同步更新本文档。

## 1. 背景与需求

当前 TTS 音色是全局单选（设置页 `[tts].voice`），只有 character 角色包能通过托管目录 `voice/` 自带音色（运行时探测，不入库）。存在的具体问题：

1. **裁决规则不一致**：语音会话/`dsh` 播报里角色音色必赢全局音色，设置页试听里显式选择优先，无一处向用户说明。
2. **绑定能力只覆盖 character 包**：Live2D/GIF 伙伴无法拥有自己的音色，只能吃全局默认。
3. **音色库与角色零关联**：`~/.zapmomo/voices/` 是「一堆音色」而非「角色的声音」。

**目标**：每个角色尽可能有自己的音色；没有自定义音色时回退 TTS 全局默认音色。

### 1.1 已确认的产品决策

1. **混合三级解析**：
   - 第 1 级：伙伴托管目录 `voice/reference.wav + reference.txt`（角色包自带，保留；扩展到任意 format 都可探测）；
   - 第 2 级：`library.json` 绑定的音色库条目 id（新增，UI 可绑/解绑）；
   - 第 3 级：全局默认音色 `[tts].voice`（现状兜底）。
2. **qwen3_tts（强制克隆族）无音色保持报错**，仅优化文案为引导（绑定角色音色 / 设置默认音色 / 换 omnivoice/voxcpm2）。

## 2. 现状架构分析

### 2.1 音色解析链路（保持不变的部分）

统一入口 `src/tts/voice.rs`：

- `resolve_reference`：custom_wav > voice_id(`[voice].voice`/CLI) > cfg.voice(`[tts].voice`) > 内置 leijun；
- `resolve_voice_params`：backend + 模型族感知，产出 `TtsVoiceParams`（Sid / Reference / Named）。

消费点 4 处（优先级矩阵字面不变）：

| 消费点 | 行为 |
|---|---|
| `src/voice/session.rs:197`（会话初建）/ `:358`（TTS 热切换） | `cfg.character_voice` > `cfg.voice_id` > 默认 |
| `src-tauri/src/lib.rs:1334`（试听 synthesize_tts） | 显式参数 > 角色音色 > 默认 |
| `src/dsh/announce.rs:55`（播报） | 角色音色 > 配置默认 |

注入点 `src/voice/config.rs:171` `apply_companion_overrides`：仅 `cfg.tts.uses_reference_audio()` 时注入 `cfg.character_voice`；人设 persona 全量覆盖 system_prompt。

### 2.2 关键现状事实

- `active_character_voice()`（companion.rs:972）经 `active_character_model()` **过滤 is_character**；`character_voice_in(model_dir)`（:978）本身不挑 format。
- `CompanionModel.layout` 已示范 serde-default 可选字段先例；`SCHEMA_VERSION=1` 宽容加载（缺字段 default、未知字段忽略、无 deny_unknown_fields）→ 加字段无需迁移。
- `library.json` 写入口唯一（`save_library_inner`，须持 `COMPANION_LOCK`）。
- `relocate_payload` 整目录搬迁托管目录，`voice/` 子目录自然跟随；绑定存「音色库 id」而非路径 → 搬迁安全。
- `reconcile_active`（lib.rs:2895）在 `live2d.model_dir == desired` 时**早退** → 绑定变更走它不会重启会话。
- `list_tts_voices` 在非克隆模型下返回空 → 绑定 UI 不能复用。
- `voice_store::load_manifest` 吞错（`unwrap_or_default`）+ `save_voice` 覆盖写 → manifest 损坏后一次保存即**清空全部音色**（既有隐患，绑定功能会放大它）。
- `resolve_reference` 按「id 或 name」双匹配且有 builtin 回退 → 结构化绑定不可复用（名称碰撞/绑 builtin 跨模型失效/失效静默出默认音色）。

## 3. 技术方案

### 3.1 目标解析规则

```
active_companion_voice()（仅当 cfg.tts.uses_reference_audio() 时被消费）：
  1. active 伙伴托管目录 voice/reference.wav + reference.txt 成对非空 → CharacterVoice
  2. 伙伴 voice_id 绑定 → voice_store::find_voice_by_id 命中 → 折叠成 CharacterVoice
  3. 其余（未绑定 / 绑定失效）→ None
     （None 时上层自然回落 [voice].voice > [tts].voice > 内置）
```

任何一层读取失败只 `tracing::warn` + 降级，不报错（fail-open，对齐 `character_voice_in` 现有分支）。

**关键设计：绑定解析结果折叠成 `CharacterVoice { wav, text }` 注入 `cfg.character_voice`**，复用现有注入链 → `session.rs` / `resolve_voice_params` 主链路一行不改。

### 3.2 数据模型

```rust
// CompanionModel 新增（serde default，老 library.json 宽容加载，SCHEMA_VERSION 保持 1）
#[serde(default, skip_serializing_if = "Option::is_none")]
pub voice_id: Option<String>,   // 绑定的音色库条目 id
```

`CompanionView`（Tauri payload）拆三字段：`voice_id`（绑定 id）、`voice_source`（`"pack"` / `"library"` / null）、`has_voice`（生效判定）——UI 据此区分「自带生效 / 绑定生效 / 绑定失效」。

### 3.3 关键决策

1. **新增 `voice_store::find_voice_by_id(id)` 严格按 id 查**，companion 侧只用它；`resolve_reference` 宽容语义保持不动（服务设置页手填场景）。
2. **`CharacterVoice` 结构体与 `character_voice` 字段名不改**，只改函数名（`active_character_voice` → `active_companion_voice`、`has_character_voice` → `has_voice`），diff 最小。
3. **生效时机显式 restart**：从 `reconcile_active` 末尾抽出 `restart_voice_session_if_running(app)`；`set_companion_voice`（改的是 active 伙伴时）与 `delete_tts_voice`（清理的绑定命中 active 伙伴时）复用。代价（打断在途播报、清 history）与「切换伙伴」现状一致。
4. **新增模型无关命令 `list_voice_library`**（= `voice_store::list_custom_voices()`）供绑定 UI。
5. **删除音色联动清理**放 Tauri 命令层（依赖单向：companion → voice_store，voice_store 不感知 companion）；清理失败仅 warn（孤儿绑定降级为默认音色，不阻断删除）。
6. **顺带修 R2**：manifest parse 失败时 `save_voice` 拒绝写并告警。
7. **命令层逻辑全部下沉根 crate**（CI 只测根 crate；命令层只做编排）。

### 3.4 优先级矩阵（最终）

```
目录 voice/ > 绑定 > [voice].voice(CLI/会话) > [tts].voice > 内置 leijun
```

与现状完全一致，仅第 1 级来源从「character 包目录」扩展为「任意 format 目录 + 绑定」。需在文档写明：显式绑定会压过 CLI `--voice`。

## 4. 实施方案

### Phase 1 — 根 crate 数据层 + 解析（纯逻辑，可全测）

| 文件 | 改动 |
|---|---|
| `src/tts/voice_store.rs` | `find_voice_by_id`（严格 id）；R2 防护；单测（id 查不到 None / name 查不命中） |
| `src/companion.rs` | `voice_id` 字段；`companion_voice_in`；`active_companion_voice`；`has_voice`；`set_voice_binding`（校验存在性）；`clear_voice_bindings`（返回被清理伙伴 id 列表）；全套单测 |

注意：改名使下游短暂编译失败 → 与 Phase 2 连续完成。

### Phase 2 — 注入点切换 + 文案

| 文件 | 改动 |
|---|---|
| `src/voice/config.rs:171` | 改调 `active_companion_voice()`；doc 三级语义；新增绑定注入 / cubism3 format 注入 / 非克隆不注入测试 |
| `src/dsh/announce.rs:55` | 同步改名 |
| `src/tts/voice.rs:142` | qwen3 报错文案更新；同步 L410 测试断言 |
| `src/voice/session.rs:1176` | 热切换取消日志文案同步 |

**不改**：`session.rs:197/356`、`cli.rs:569`。

### Phase 3 — Tauri 命令层（`src-tauri/src/lib.rs`）

- `CompanionView` 加 `voice_id` / `voice_source`；`build_view` 填充。
- 抽出 `restart_voice_session_if_running(app)`，`reconcile_active` 原位调用。
- 新增 `set_companion_voice(app, id, voice_id: Option<String>) -> CompanionLibraryView`（spawn_blocking → active 则 restart → view），注册进 `invoke_handler`。
- 新增 `list_voice_library() -> Vec<TtsVoice>`。
- `delete_tts_voice` 加 `app: AppHandle`：delete → `clear_voice_bindings`（Err 仅 warn）→ 命中 active 则 restart。

### Phase 4 — 前端

| 文件 | 改动 |
|---|---|
| `types/tauri.ts` | `CompanionModelInfo` 加 `voice_id: string \| null`、`voice_source: "pack" \| "library" \| null` |
| `lib/tauri.ts` | `listVoiceLibrary()`、`setCompanionVoice({ id, voiceId })` —— **必须 camelCase `voiceId`**（写 `voice_id` 静默丢参且 tsc 不报错） |
| `hooks/useCompanionLibrary.ts` | `setVoice`（照抄 `rename` mutation + toast 模式） |
| `pages/CompanionPage.tsx` | 右栏详情区加「音色」Select（选项 = 「使用全局默认」(value="") + 音色库条目；`voice_source === "pack"` 加 disabled 项「角色包自带音色（优先生效）」）；hint：非克隆模型提示（不禁用——绑定是模型无关元数据）、失效提示；列表 Badge 失效态 |
| 测试 | `CompanionPage.test.tsx` invoke switch 补新命令 case + 载荷键名断言；`useCompanionLibrary.test.tsx` 补 setVoice 用例 |

### Phase 5 — 收尾

`AGENTS.md` / `CHANGELOG.md` 补说明；全量门禁；人工验收。

## 5. 验收标准

### 命令级门禁

```bash
# 根 crate（CI 口径）
cargo fmt --check && cargo clippy -- -D warnings && cargo test -- --test-threads=1
# tauri crate（CI 不编译，必须本地验）
cargo check -p zapmomo-app && cargo clippy -p zapmomo-app -- -D warnings
# 前端（无 CI，必须本地全跑）
pnpm --filter zapmomo-frontend exec tsc -b
pnpm --filter zapmomo-frontend check          # biome
pnpm --filter zapmomo-frontend test:run       # vitest
```

### 人工验收清单（`pnpm tauri dev`）

1. 角色包自带音色优先于绑定（芙宁娜放 voice/ + 绑定另一音色 → 生效的是自带）；
2. 给 Live2D/GIF 伙伴绑定音色后，试听 / 语音会话 / dsh 播报三路音色一致；
3. 运行中会话改绑定 → 会话立即重启并使用新音色；
4. 删除被绑定音色 → Badge 失效 + 会话回退默认音色；
5. qwen3 无任何音色 → 报错文案正确引导；
6. 老 `library.json`（无 voice_id 字段）升级无损；`data_dir` 搬迁后绑定仍命中。

## 6. 风险与回滚

| # | 风险 | 等级 | 缓解 |
|---|---|---|---|
| R1 | 失效绑定静默降级，用户困惑 | 中 | UI 失效 Badge + hint；解析 warn 带伙伴 id |
| R2 | manifest 损坏覆盖写清空音色（既有隐患） | 高 | Phase 1 拒绝写 + 告警 |
| R3 | restart 打断在途播报 / 清 history | 中 | 仅 active 伙伴且有运行会话时触发；与切换伙伴现状一致 |
| R4 | 前端 snake_case 丢参（`voice_id` vs `voiceId`） | 中 | 测试断言载荷键名 |
| R5 | 非 character format 目录撞 Live2D 同名文件 | 低 | `voice/reference.wav` 非 Live2D 约定；探测要求 wav+txt 成对且非空 |
| R6 | 绑到 builtin id 跨模型失效 | 低 | `set_voice_binding` 只允许 voice_store 条目（结构上禁止） |
| R7 | 命令层无自动化测试 | 中 | 逻辑全下沉根 crate，命令层只编排 |
| R8 | 与窗口属性命令（set_companion_scale 等）命名混淆 | 低 | doc 注释明确是伙伴库元数据 |

**回滚**：`voice_id` 为 Option + skip_serializing_if，旧代码读新文件忽略未知字段，无需数据迁移；各 Phase 独立 commit，可单独 revert，行为面回滚集中在 companion.rs + config.rs:176 + 3 个调用点。

---

## 增补（2026-08-30）：用户上传音色覆盖（备份作者原版）

> 详细方案见 `COMPANION_VOICE_UPLOAD_DESIGN.md`。本节记录对本文档音色体系的扩展。

### 变更

角色包自带音色不再「只读」：用户可在伙伴面板**上传自定义音色覆盖**当前生效音色，
覆盖前自动备份作者原版，可一键恢复。

```
{model_dir}/voice/
├── reference.wav + reference.txt        # 当前生效音色（作者原版 或 用户上传覆盖后的版本）
└── reference.original.wav + .txt        # 首次覆盖时的作者原版备份（本机恢复点，不随包分享）
```

### 语义

- **解析链零改动**：`companion_voice_in` 三级优先级保持「目录自带 > voice_id 绑定 >
  全局默认」——上传直接写托管目录，合成/试听/欢迎语重生成自然全部切换。
- **分享语义**：导出白名单按精确文件名收 `reference.wav/txt`，`reference.original.*`
  结构性不进包 → **接收者拿到的是分享者调好的音色**（角色包 = 角色当前完整状态）。
- **联动**：音色改写使欢迎语指纹（wav len/mtime）失效 → 后台自动用新音色重生成；
  active 伙伴上传/恢复即重启语音会话。
- **事务性**：校验（头 + mono 转换 + hound 全量终检）全部在暂存文件完成，备份
  （纯拷贝）成功后才落位（txt 先、wav 最后翻面，保住成对不变量）；任何失败不破坏
  既有音色。
- **恢复**：`restore_companion_voice` 拷回备份并删除备份文件；备份在恢复成功前绝不删除。

### 新增接口

- 根 crate：`upload_companion_voice` / `restore_companion_voice` / `has_original_voice`
- Tauri command：`upload_companion_voice` / `restore_companion_voice`
- 前端：音色行「上传音色」（`CompanionVoiceUploadDialog`：选 wav → ASR 自动转写可编辑 →
  覆盖）与「恢复角色自带」（确认框，仅 `has_original_voice` 时显示）
