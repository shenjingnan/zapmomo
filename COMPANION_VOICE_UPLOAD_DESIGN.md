# 技术方案：伙伴音色上传覆盖（备份原作者音色）

> 状态：已评审通过（2026-08-30）
> 前置：`COMPANION_WAKE_WORD_WELCOME_DESIGN.md`（音色试听/欢迎语预合成已实施）、`COMPANION_PACK_SHARE_DESIGN.md`（.zip 分享已实施）
> 关联模块：`src/companion.rs`、`src/companion_share.rs`、`src-tauri/src/lib.rs`、伙伴面板前端

## 1. 对现状的分析

- 伙伴音色三级解析（`companion_voice_in`：托管目录 `voice/reference.wav+txt` > 音色库绑定 `voice_id` > 全局默认）中**目录自带优先级最高**，伙伴面板对带自带音色的角色禁用音色下拉框——用户对不满意的自带音色无法调整。
- 音色库（`tts::voice_store`）是「跨角色复用的个人音色收藏」；用户的需求是「**针对这个角色**调整音色」，且调整结果要**随角色包分享**（角色包 = 角色当前完整状态，与欢迎语文本的导出语义对称）。
- sherpa `Wave::read` 只接受单声道；导入链已有 `convert_reference_to_mono`（就地改写托管副本）解决此问题。
- 欢迎语预合成指纹覆盖 `character_voice.wav` 的 `(path, len, mtime)` → 音色文件被改写后指纹自动失效，`ensure_active_welcome_wav` 自动用新音色重生成（联动零改动）。

## 2. 当前架构分析

音色的消费方（合成/试听/欢迎语重生成）全部经由 `companion_voice_in` 单点解析；写入方只有导入链（`convert_reference_to_mono` 只在导入时改写一次）。本方案新增第二条写入路径（用户上传），写入目标仍是同一目录级资产——**解析链零改动，消费方自然切换**。

```mermaid
flowchart LR
    subgraph 上传 upload_companion_voice
        A[校验转写非空] --> B[拷贝到暂存 wav]
        B --> C[validate_wav_header<br/>轻量头校验]
        C --> D[convert_reference_to_mono<br/>暂存文件就地转 mono]
        D --> E[read_wav_mono 终检<br/>合成链同款解码器]
        E --> F[backup_original_voice<br/>首次覆盖前备份作者原版]
        F --> G[txt 先落位 → wav 最后翻面<br/>保住成对不变量]
    end
    G -.改写 reference.wav len/mtime.-> H[欢迎语指纹失效<br/>ensure 自动重生成]
    G --> I[companion_voice_in<br/>解析链零改动]
    subgraph 恢复 restore_companion_voice
        J[original.* 拷回 tmp] --> K[头校验自检] --> L[落位] --> M[最后删备份<br/>失败可重试]
    end
```

## 3. 技术方案

### 3.1 设计决策

| 决策点 | 结论 | 理由 |
| --- | --- | --- |
| 覆盖方式 | 直接写托管目录 `voice/reference.*`，解析链零改动 | 目录自带即生效；合成/试听/欢迎语联动自然切换；不与音色库/绑定语义纠缠 |
| 备份 | 首次覆盖前拷贝为 `reference.original.wav + .txt`；重复上传不覆盖备份；恢复成功后删除备份 | 备份永远保持作者原版；逐文件独立判断可自愈半完成备份 |
| 分享语义 | `reference.original.*` 不进分享包（白名单精确名匹配，结构性排除） | 接收者拿到的是分享者调好的音色；备份是本机恢复点 |
| 成对不变量 | txt 先落位、wav 最后翻面 | `validate_character_pack` 要求 wav+txt 成对，破对会被 `sanitize_active_inner` 判伙伴无效降级 active |
| 校验链 | validate_wav_header（轻量早拦）→ convert mono → read_wav_mono 终检（hound 全量解码 = 合成链同款） | 头合法但 body 截断/0 采样在落位前拦下 |
| 事务性 | 所有校验/转换在暂存文件完成；备份（纯拷贝）成功后才落位；Err 路径清 tmp | 任何失败不得破坏既有 reference.* |
| 格式范围 | 第一版仅 wav；立体声/24bit/float 自动转 16-bit mono，采样率原样留给合成侧归一 | mp3/m4a 转码列 Future |
| 适用范围 | 任意 format（GIF/Live2D 无自带音色，首传不建备份）；不写 library.json | 音色是目录级资产，voice_id 绑定体系不动 |

### 3.2 接口

```rust
// src/companion.rs
pub fn has_original_voice(model: &CompanionModel) -> bool;   // original.wav+txt 成对存在
pub fn upload_companion_voice(id: &str, source_wav: &Path, reference_text: &str) -> Result<(), String>;
pub fn restore_companion_voice(id: &str) -> Result<(), String>;
pub(crate) const REFERENCE_ORIGINAL_WAV: &str = "reference.original.wav";
pub(crate) const REFERENCE_ORIGINAL_TXT: &str = "reference.original.txt";
```

```ts
// 前端
uploadCompanionVoice(args: { id: string; wavPath: string; referenceText: string }): Promise<CompanionLibraryView>;
restoreCompanionVoice(args: { id: string }): Promise<CompanionLibraryView>;
// CompanionModelInfo 新增 has_original_voice: boolean
```

### 3.3 前端交互

- 音色行按钮列：`音色 [Select] [🔊 试听] [⤒ 上传音色] [↺ 恢复角色自带]`
- 上传对话框（`CompanionVoiceUploadDialog`，复用 ModelDialog 外壳）：选 wav → ASR 自动转写（可编辑，必填）→ 「保存并覆盖」；正文提示覆盖/备份/分享语义；错误用 destructive Alert（toast 留给 hook 层）
- 恢复按钮仅 `has_original_voice` 时显示；点击弹确认框（恢复不可逆，照移除伙伴确认模式）
- Select 禁用项与音色库体系维持现状

## 4. 实施方案（三阶段 + 收尾）

| 阶段 | 内容 | 文件 | 验收 |
| --- | --- | --- | --- |
| 1. 根 crate 域逻辑 | 常量/voice_paths/rename_replace（convert 复用）/backup_original_voice/has_original_voice/upload/restore；validate_wav_header+convert_reference_to_mono 放宽 pub(crate)；单测 | `src/companion.rs`、`src/companion_share.rs`(补 2 测试) | `cargo test` + fmt/clippy |
| 2. Tauri 命令层 | CompanionView 字段 + 两 command + 注册 | `src-tauri/src/lib.rs` | `cargo check/clippy -p zapmomo-app` |
| 3. 前端 | 类型/fixture×4/api/hook/上传对话框/按钮列/确认框 + vitest | frontend 6 文件 | `tsc -b` + `vitest run` + biome |
| 4. 收尾 | `COMPANION_VOICE_DESIGN.md` 增补 + dev 端到端验收 | — | — |

## 5. 测试计划（关键用例）

- **根 crate**：上传后备份与作者原版字节一致；重复上传保持首次备份；无 voice/ 首传不建备份且命中 Pack 来源；立体声转 mono；损坏 wav → Err 且原文件完好无备份；空转写/未知 id → Err 零改动；恢复 roundtrip（字节一致 + 备份删除 + flag false）；无备份恢复 → Err；手动删 reference.wav 后恢复修复；**上传后 `clip_fingerprint` 变化**（欢迎语自动重生成链验证）
- **companion_share**：上传后导出 zip 含新 reference.*、不含 original.*；新 home 导入该包 `has_original_voice == false`；`classify_entry` 对 original 命名返回 None（引用常量防漂移）
- **前端**：uploadVoice/restoreVoice 的 toast 与视图更新；对话框 canSubmit 门控；恢复按钮显隐
- **端到端手动**：导入自带音色角色包 → 试听作者音色 → 上传 → 试听/合成/欢迎语均切换 → 连传两次 → 恢复回作者原版 → 导出 zip 带自定义版且无备份 → 立体声 wav 转 mono

## 6. 风险与边界

| 情况 | 处理 |
| --- | --- |
| 原 voice/ 不存在（首传） | create_dir_all 后走同链；无备份；has_original_voice=false |
| 重复上传 | 备份条件不满足 → 备份保持作者原版 |
| 备份半完成 | 逐文件独立判断自动补齐 |
| 损坏/非 wav（RIFF 非 WAVE） | 头校验拦截；原文件完好且无备份产生（不用只查 RIFF 的 is_wav_file） |
| 头合法 body 截断/0 采样 | read_wav_mono 终检拦截 |
| 恢复时 reference 被手动删 | copy 不依赖目标存在，成对复原 |
| 恢复中途失败 | 备份未删可重试 |
| 伙伴移除 | 备份随托管目录清理 |

## 7. 明确不做（Future）

非 wav 格式转码（mp3/m4a/ogg）· 在线录音作为上传来源 · 多版本备份/撤销恢复 · 音频质量校验（时长/静音/响度）· 列表「音色已自定义」Badge · 分享时音色附带开关 · CLI 暴露
