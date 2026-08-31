# 技术方案：角色包导出/分享（.zip）

> 状态：已评审通过（2026-08-30）
> 前置：`COMPANION_WAKE_WORD_WELCOME_DESIGN.md`（角色级唤醒词/欢迎语 + `character.json` 声明已实施）
> 关联模块：`src/companion_share.rs`（新）、`src/companion.rs`、`src-tauri/src/lib.rs`、伙伴面板前端

## 1. 对现状的分析

上一阶段后，角色包已具备完整的元数据声明能力（`character.json`：name/wake_word/welcome_text 作者预设随包流通），但分享仍需手工拷目录：

- 应用内没有「导出」入口；用户不清楚该拷哪些文件（误拷 `cover.png`/`voice/welcome.wav` 等应用派生文件会带入垃圾，漏拷 `voice/reference.txt` 会导致音色失效）；
- 对方拿到目录后需走「选择目录」导入，而网盘/IM 分享的现实载体是压缩包；
- `derive_id` = sha256(源路径)：解压到随机临时目录会导致同一压缩包重复导入产生重复伙伴。

## 2. 当前架构分析

导入链已有完整的「两阶段 + 白名单校验」骨架可复用：`import_source` 按「文件→GIF / 目录含 character.md→角色包 / 目录→Live2D」分派；`import_character_from_dir` 内部 = manifest 严格校验 → 锁内去重 → `.tmp-` 复制 → `validate_character_pack` → 立体声转单声道 → `commit_import`（原子 rename + RAII 清理）。**导出/导入 zip 的正确姿势是：只在两端处理 zip 编解码与安全，中间完全复用既有链路**。

托管目录内容物已明确二分：源资产（character.md/png/json、sprites/、voice/reference.*）与应用派生（cover.png、voice/welcome.wav/welcome.json）。

```mermaid
flowchart LR
    subgraph 导出 export_pack
        A[character_pack_model<br/>仅 format=character] --> B[reject_managed_dest<br/>目的地防线]
        B --> C[validate_character_pack 预检]
        C --> D[collect_export_entries<br/>白名单·排序·排除派生]
        D --> E[build_pack_manifest<br/>自定义&gt;预设&gt;缺席]
        E --> F[write_zip partial+rename]
    end
    subgraph 导入 import_zip
        G[魔数 PK 预检] --> H[确定性解压路径<br/>sha256 zip 路径]
        H --> I[extract_pack<br/>zip-slip 防护+白名单+限额]
        I --> J[resolve_pack_root<br/>宽容一层 wrapper 目录]
        J --> K[import_character_from_dir<br/>完整复用既有链]
    end
    F -.分享 .zip.-> G
```

## 3. 技术方案

### 3.1 设计决策

| 决策点 | 结论 | 理由 |
| --- | --- | --- |
| zip 结构 | **根目录扁平**（character.md 在根）；导入端宽容恰好一层 wrapper 目录 | 扁平解压目录可直接喂 `import_character_from_dir`（判据 `source.join("character.md")`），零转换；wrapper 容错救「Windows 右键压缩」的真实分享包 |
| 重复导入 | **确定性解压路径** `temp_dir/zapmomo-pack-{sha256(zip路径)[..12]}` | 同一 zip ⇒ 同一解压目录 ⇒ 同一 companion id ⇒ 重复导入走 `already_imported` |
| 导出 manifest | `version` 恒 1、`name` 恒当前名；`wake_word`/`welcome_text` = 用户自定义 > 托管原件预设 > **字段缺席**；不写推导兜底值 | 兜底值（跟随角色名/默认模板）是推导语义，写死会焊死「跟随改名」；缺席即导入端自然推导 |
| 打包范围 | 白名单：character.md/png/json、sprites/ 一层 png\|gif\|webp、voice/reference.*；**结构性排除** cover.png、voice/welcome.*、隐藏文件 | 白名单驱动而非「全收再删」，托管目录干净是解压/打包两端的结构性保证 |
| 支持范围 | 仅 `format="character"` 导出；Live2D（版权模型+无 character.md）、GIF（单文件）UI 禁用+后端硬拒绝 | 双保险 |
| zip crate | `zip = "8"`，`default-features = false, features = ["deflate-flate2"]`（minimal，纯 Rust）；显式 `CompressionMethod::Deflated` | MSRV 1.88 < 1.97；不开 aes/lzma/zstd 等重特性控制编译与供应链面 |
| 安全 | zip-slip 双防线（`enclosed_name()` + 自拼组件）；条目数 ≤4096、解压总量 ≤512MiB（防炸弹）；条目名先 `\`→`/` 归一再匹配白名单 | Windows 工具打出的 zip 用反斜杠分隔 |

### 3.2 接口

```rust
// src/companion_share.rs（新模块）
pub struct ExportedPack { pub dest: PathBuf, pub files: u32 }
pub fn export_pack(id: &str, dest: &Path) -> Result<ExportedPack, String>;
pub fn import_zip(zip_path: &Path) -> Result<(CompanionModel, bool), String>;
```

```ts
// 前端
exportCompanionPack(args: { id: string; dest: string }): Promise<{ dest: string; files: number }>;
importCompanionZip(args: { source: string }): Promise<ImportCompanionResult>;
```

### 3.3 前端交互

- 列表项第 4 个 icon 按钮（`Share2`，与打开资产/重命名/移除同款式）：非 character 格式 disabled + title 说明；busy 时 spinner；点击 → `save({ defaultPath: "{name}.zip" })` → 导出 → toast
- 「导入 GIF」右侧加 outline 图标按钮（`FileArchive`，「导入角色包（.zip）」）：open 对话框 zip filter → `import_companion_zip` → 复用既有「已导入提示/自动选中」逻辑
- 不做拖拽导入（无 drag-drop 先例）

## 4. 实施方案（三阶段）

| 阶段 | 内容 | 文件 | 验收 |
| --- | --- | --- | --- |
| 1. Rust 导出 | zip 依赖；companion.rs 常量/`CompanionManifest::preset` 放宽 `pub(crate)` + `character_pack_model` helper；新建 companion_share.rs 导出面 | Cargo.toml、src/lib.rs、src/companion.rs、src/companion_sprites.rs、src/companion_share.rs(新) | `cargo test companion_share` + clippy/fmt |
| 2. Rust 导入 + Tauri | import_zip 全链；lib.rs 抽 `finish_import`；`export_companion_pack`/`import_companion_zip` command + 注册；capabilities 加 `dialog:allow-save` | src/companion_share.rs、src-tauri/src/lib.rs、src-tauri/capabilities/default.json | `cargo test` + `cargo check/clippy -p zapmomo-app` |
| 3. 前端 | 类型/api/hook（`applyImportResult` 抽取、`importZip`/`exportPack`）+ 两按钮 + vitest | frontend 5 文件 | `tsc -b` + `vitest run` + biome（改动文件） |

## 5. 测试计划

- **导出白名单精确性**：托管目录塞入 cover.png/welcome.wav/.DS_Store/notes.txt → 导出 → zip 条目集合精确等于白名单；两次导出字节一致（排序确定性）
- **manifest 合成三态**：自定义优先 / 保留预设 / 皆无则字段缺席（JSON 文本不含 `"wake_word"`）；name 恒 = rename 后值；托管 json 损坏 → 导出报错
- **roundtrip 核心**：导入 → 导出 → import_zip → **id 一致**；再导入同 zip → `already=true` 且列表不增
- **A 层流通**：新 temp home 导入同一 zip → name/wake_word/welcome_text 与包内一致
- **安全**：`../evil.txt` 条目 → Err 且无越界文件；`__MACOSX/` 等杂物包 → 明确报错；wrapper 目录 → 成功；`\` 分隔条目 → 归一命中白名单；伪 zip → 魔数报错
- **前端**：exportPack/importZip 调用参数与 toast；非 character 行按钮 disabled；save 取消不调用
- **端到端手动**：导入角色包 → 自定义唤醒词/欢迎语 → 导出 → 删除伙伴 → 从 zip 导入 → 全部预设完整还原

## 6. 风险与边界

| 情况 | 处理 |
| --- | --- |
| zip-slip（`../`、绝对路径） | `enclosed_name()` + 自拼组件双防线；白名单只允许固定形态，`..` 结构上无法命中 |
| zip 炸弹 / 超大 sprites | 条目数与解压总量上限，解压前累计校验即中止 |
| Windows 反斜杠 | 写端组件拼 `/`；读端先归一再匹配 |
| 同名覆盖 | 导出 partial+rename 原子；解压端先清空重建；zip 白名单天然无重名 |
| dest 落在托管目录 | `reject_managed_dest` 拒绝 |
| 同 zip 内容更新后重导入 | 路径 hash ⇒ id 不变 ⇒ `already_imported`（与文件夹导入「同源不重复」语义一致，文档化；覆盖导入列 Future） |
| zip 被移动/改名后重导入 | 路径变 ⇒ id 变 ⇒ 出现重复（与文件夹导入行为一致，文档化） |

## 7. 明确不做（Future）

Live2D/GIF 导出 · 覆盖/升级导入 · 拖拽导入 · 导出进度事件 · 包校验和/版本清单 · voice_id/layout 随包流通 · URL/网盘导入 · 批量导入
