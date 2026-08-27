# 移除 llama.cpp 本地模型能力 — 详细实施方案

> **⚠️ 实施时范围修正（2026-08-26）：** 原方案计划整体删除 `src/model_library/` 与全部模型库命令。实施中发现 `list_model_library`/`set_current_model`/`delete_model` 等命令**同时支撑着 KWS/ASR/TTS 页面的多模型切换对话框**（`useKwsModelSwitch` 等 hooks 均依赖模型库后端），并非仅服务 LLM。经与用户确认，最终范围为：
>
> - **保留** `src/model_library/` 模块（剥离 LLM/GGUF 条目）与模型库命令层，KWS/ASR/TTS 三个页面的模型切换对话框完整保留；
> - LLM 部分仍按原方案改为纯远程连接（用户自填 API URL + Key + 模型名），本地 LLM 模型下载/切换/预设弹窗全部移除。
>
> 下文中「移除模型库」相关任务（阶段 2、Task 3.4、Task 4.1 等）按上述修正后的范围执行。

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 移除 llama.cpp 本地推理能力和模型库管理系统，只保留 OpenAI 兼容远程 API 能力，让用户自行配置 API URL 和 Key。

**Architecture:** 删除 `src/llm/local/` 和 `src/model_library/` 两个模块，将 `LlmEngine` 简化为只使用 `OpenAiChatProvider`，`LlmSettings` 移除本地模型专属字段，前端 LLM 设置页改为 URL/Key/Model 输入表单。

**Tech Stack:** Rust 1.97, Tauri 2, React 19, TypeScript, async-openai 0.41

**预估变更量:** ~5500 行净删除，~500 行修改/新增

---

## 阶段 0：准备工作

### Task 0.1: 创建功能分支

**Step 1: 创建分支**

```bash
git checkout -b feature/remove-llama-cpp-local-models
```

**Step 2: 确认分支**

```bash
git branch --show-current
```
Expected: `feature/remove-llama-cpp-local-models`

---

## 阶段 1：后端核心 — 移除 llama.cpp 本地 provider

### Task 1.1: 删除 `src/llm/local/` 目录

**Files:**
- Delete: `src/llm/local/llama.rs`
- Delete: `src/llm/local/mod.rs`

**Step 1: 删除目录**

```bash
rm -rf src/llm/local/
```

**Step 2: 验证编译错误（预期有大量引用错误）**

```bash
cargo check 2>&1 | head -30
```
Expected: 编译错误，因为 `src/llm/mod.rs` 等文件还在引用 `local::LocalLlamaProvider`

### Task 1.2: 简化 `src/llm/mod.rs` — 移除 local provider 引用

**Files:**
- Modify: `src/llm/mod.rs:1-395`

**Step 1: 更新模块声明**

将文件头部的模块注释和 `pub mod local;` 移除：

```rust
// 修改前 (lines 1-14):
/// 本地 LLM 模块。
///
/// 分层：
/// - `LlmEngine`（门面）：生命周期 + worker 线程 + 命令/事件 channel，供 CLI/Tauri 使用。
/// - `LlmProvider`（trait）：后端抽象，本地 llama.cpp 只是其中一种实现。
/// - `local`：`LocalLlamaProvider`，唯一接触 llama.cpp 的地方。
pub mod agent;
pub mod config;
pub mod error;
pub mod http;
pub mod local;
pub mod provider;
pub mod tools;
pub mod types;

// 修改后:
/// LLM 模块（远程 API 调用）。
///
/// 分层：
/// - `LlmEngine`（门面）：生命周期 + worker 线程 + 命令/事件 channel，供 CLI/Tauri 使用。
/// - `LlmProvider`（trait）：后端抽象。
/// - `http`：`OpenAiChatProvider`，OpenAI 兼容 Chat Completions API。
pub mod agent;
pub mod config;
pub mod error;
pub mod http;
pub mod provider;
pub mod tools;
pub mod types;
```

**Step 2: 简化 `create_provider()` 函数**

将 `create_provider` (lines 242-254) 修改为只支持 HTTP provider：

```rust
/// 根据配置创建 provider。
///
/// 当前只支持 OpenAI 兼容 Chat Completions API。
pub fn create_provider(
    config: ResolvedLlmConfig,
) -> Result<Box<dyn provider::LlmProvider>, LlmError> {
    match config.provider.as_str() {
        "openai" | "llamacpp-server" => Ok(Box::new(http::OpenAiChatProvider::new(&config)?)),
        other => Err(LlmError::UnsupportedProvider(other.to_string())),
    }
}
```

**Step 3: 简化 `worker_loop` 中的 load/unload 逻辑**

`OpenAiChatProvider` 的 `load()` 和 `unload()` 是空操作，`is_ready()` 始终返回 true。worker_loop 中 load 成功后直接标 ready，无需真正加载模型。保留现有逻辑不变（因为 `OpenAiChatProvider::load()` 返回 `Ok(())`，`is_ready()` 返回 `true`）。

**Step 4: 运行编译检查**

```bash
cargo check 2>&1
```
Expected: 编译通过（如果 `src/llm/config.rs` 还在引用 local 模块会有错误，下一个 task 修复）

### Task 1.3: 简化 `src/llm/config.rs` — 移除本地模型配置

**Files:**
- Modify: `src/llm/config.rs:1-301`

**Step 1: 移除不再需要的常量、函数和字段**

需要移除的内容：
- `DEFAULT_MODEL_NAME` (line 11)
- `DEFAULT_MODEL_FILE` (line 13)
- `default_model_path()` 函数 (lines 47-61)
- `discover_gguf()` 函数 (lines 67-80)
- `discover_gguf_in()` 函数 (lines 82-100)
- `default_threads()` 函数 (lines 103-108)
- `resolve_model_path()` 函数 (lines 113-130)
- `ResolvedLlmConfig.model_path` 字段 (line 23)
- `ResolvedLlmConfig.auto_load` 字段 (line 29)
- `ResolvedLlmConfig.enabled` 字段 (line 19) — 保留，但改为默认 true
- `ResolvedLlmConfig` 中 `context_size`, `batch_size`, `threads`, `gpu_layers`, `enable_thinking` 等本地专属字段

**Step 2: 重写 `ResolvedLlmConfig`**

```rust
/// 解析后的 LLM 配置（字段全部为具体类型，非 `Option`）。
#[derive(Debug, Clone)]
pub struct ResolvedLlmConfig {
    /// 是否启用 LLM
    pub enabled: bool,
    /// provider 标识（"openai" 或 "llamacpp-server"）
    pub provider: String,
    /// 角色 system prompt
    pub system_prompt: String,
    /// 采样/生成参数（仅 max_tokens, temperature, top_p 对远程 API 有效）
    pub params: GenParams,
    /// HTTP provider 的 base URL（如 https://open.bigmodel.cn/api/paas/v4）
    pub base_url: Option<String>,
    /// HTTP provider 的 API key
    pub api_key: Option<String>,
    /// HTTP provider 的模型名（如 glm-4.7-flash）
    pub model: Option<String>,
}
```

**Step 3: 重写 `resolve()` 函数**

```rust
/// 合并 settings 得到最终配置。默认 provider 改为 "openai"。
pub fn resolve(
    settings: Option<&LlmSettings>,
    _cli_model_path: Option<&Path>,
) -> Result<ResolvedLlmConfig, String> {
    let defaults = GenParams::default();

    Ok(ResolvedLlmConfig {
        enabled: settings.and_then(|s| s.enabled).unwrap_or(true),
        provider: settings
            .and_then(|s| s.provider.clone())
            .unwrap_or_else(|| "openai".to_string()),
        system_prompt: settings
            .and_then(|s| s.system_prompt.clone())
            .unwrap_or_else(default_system_prompt),
        params: GenParams {
            max_tokens: settings
                .and_then(|s| s.max_tokens)
                .unwrap_or(defaults.max_tokens),
            temperature: settings
                .and_then(|s| s.temperature)
                .unwrap_or(defaults.temperature),
            top_p: settings.and_then(|s| s.top_p).unwrap_or(defaults.top_p),
            // 以下字段远程 API 不使用，保留默认值
            ..defaults
        },
        base_url: settings.and_then(|s| s.base_url.clone()),
        api_key: settings.and_then(|s| s.api_key.clone()),
        model: settings.and_then(|s| s.model.clone()),
    })
}
```

**Step 4: 更新测试**

移除 `#[cfg(test)]` 块中所有与本地模型路径、GGUF 发现相关的测试：
- 删除 `test_default_resolve_points_to_recommended_model`
- 删除 `test_default_path_dual_root_fallback`
- 删除 `test_resolve_relative_model_path_legacy_fallback`
- 删除 `test_resolve_discovers_downloaded_gguf`
- 删除 `test_cli_model_path_overrides`
- 删除 `test_default_threads_is_positive`

保留并修改 `test_settings_enabled_and_params`：

```rust
#[test]
fn test_settings_enabled_and_params() {
    let s = LlmSettings {
        enabled: Some(true),
        temperature: Some(0.9),
        max_tokens: Some(128),
        base_url: Some("https://api.example.com/v1".to_string()),
        api_key: Some("sk-test".to_string()),
        model: Some("test-model".to_string()),
        ..Default::default()
    };
    let cfg = resolve(Some(&s), None).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.params.temperature, 0.9);
    assert_eq!(cfg.params.max_tokens, 128);
    assert_eq!(cfg.base_url.as_deref(), Some("https://api.example.com/v1"));
    assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));
    assert_eq!(cfg.model.as_deref(), Some("test-model"));
}
```

**Step 5: 运行编译检查**

```bash
cargo check 2>&1
```
Expected: 编译通过或有少量其他模块的引用错误（后续 task 修复）

### Task 1.4: 简化 `src/llm/error.rs` — 移除本地专属错误

**Files:**
- Modify: `src/llm/error.rs:1-57`

**Step 1: 移除不再需要的错误变体**

移除以下变体：
- `ModelNotFound(PathBuf)` (line 10-11)
- `InvalidModel(PathBuf)` (line 14-15)
- `UnsupportedModel(String)` (line 18-19)
- `LoadFailed(String)` (line 22-23)
- `OutOfMemory(String)` (line 26-27)
- `ContextOverflow` (line 34-35)
- `NotLoaded` (line 46-47)

保留的变体：
- `InferenceFailed(String)` — 远程 API 调用失败
- `GenerationCancelled` — 用户取消
- `BackendUnavailable(String)` — 网络/服务不可用
- `Busy` — 生成互斥
- `UnsupportedProvider(String)` — 不支持的 provider

**Step 2: 运行编译检查**

```bash
cargo check 2>&1
```
Expected: 编译通过

### Task 1.5: 简化 `src/llm/types.rs` — 移除本地专属参数

**Files:**
- Modify: `src/llm/types.rs:1-501`

**Step 1: 简化 `GenParams`**

`GenParams` 中保留远程 API 实际使用的字段，移除本地专属字段。但为了最小化对 `LlmParamsPatch` 和前端的影响，保留结构体定义但标记废弃字段：

实际上，更好的做法是直接移除 `context_size`, `batch_size`, `threads`, `gpu_layers`, `enable_thinking` 字段。同时更新 `LlmParamsPatch` 移除对应字段。

修改后的 `GenParams`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenParams {
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub min_p: f32,
    pub repeat_penalty: f32,
    pub seed: u32,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            temperature: 0.7,
            top_p: 0.8,
            top_k: 20,
            min_p: 0.05,
            repeat_penalty: 1.05,
            seed: 0,
        }
    }
}
```

修改后的 `LlmParamsPatch`：

```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmParamsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
}
```

`LlmParamsPatch::apply_to` 方法相应简化，只保留 `max_tokens`, `temperature`, `top_p`, `top_k`, `min_p`, `repeat_penalty`, `seed` 的校验和写入。

**Step 2: 更新测试**

移除 `test_apply_patch_sets_all_fields` 中 `context_size`, `batch_size`, `threads`, `gpu_layers` 的断言。更新 `test_apply_patch_rejects_out_of_range` 移除对应边界测试。更新 `test_patch_resolve_roundtrip`。

**Step 3: 运行编译检查**

```bash
cargo check 2>&1
```
Expected: 编译通过

### Task 1.6: 简化 `src/llm/http.rs` — 移除 `model_path` 引用

**Files:**
- Modify: `src/llm/http.rs`

`OpenAiChatProvider::new()` 接收 `&ResolvedLlmConfig`，`ResolvedLlmConfig` 已移除 `model_path` 字段，因此 `http.rs` 不需要修改。但需要检查测试代码中的 `test_config()` 辅助函数是否还在构造 `model_path`。

**Step 1: 更新 `test_config()` 辅助函数**

将 `test_config` 中的 `model_path: PathBuf::new()` 移除。

**Step 2: 运行测试**

```bash
cargo test -p zapmomo -- llm::http 2>&1
```
Expected: 所有测试通过

### Task 1.7: 更新 `Cargo.toml` — 移除依赖

**Files:**
- Modify: `Cargo.toml:1-113`

**Step 1: 移除依赖**

移除以下行：
```toml
# 本地 LLM（llama.cpp Rust 绑定）
llama-cpp-2 = "0.1.154"

# token 逐字节解码（llama-cpp-2 的 token_to_piece 需要持久的 UTF-8 decoder）
encoding_rs = "0.8"

# 系统资源检测（模型库「系统资源」卡片：内存 / 磁盘 / CPU）
sysinfo = "0.33"
```

**Step 2: 运行编译检查**

```bash
cargo check 2>&1
```
Expected: 编译通过

---

## 阶段 2：后端核心 — 移除模型库

### Task 2.1: 删除 `src/model_library/` 目录

**Files:**
- Delete: `src/model_library/` 整个目录（10 个文件）

**Step 1: 删除目录**

```bash
rm -rf src/model_library/
```

**Step 2: 更新 `src/lib.rs` 中的模块声明**

**Files:**
- Modify: `src/lib.rs`

找到 `pub mod model_library;` 并移除。

**Step 3: 运行编译检查**

```bash
cargo check 2>&1
```
Expected: 编译错误，因为 `src-tauri/src/lib.rs` 还在引用 `model_library`

### Task 2.2: 简化 `src/config/settings.rs` — 移除模型库配置

**Files:**
- Modify: `src/config/settings.rs`

**Step 1: 移除 `ModelLibrarySettings` 和 `LocalModel`**

移除 `LocalModel` 结构体 (lines 258-272) 和 `ModelLibrarySettings` 结构体 (lines 278-319) 及其 Default 实现。

**Step 2: 从 `AppConfig` 移除 `model_library` 字段**

移除 `AppConfig` 中的：
```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub model_library: Option<ModelLibrarySettings>,
```

**Step 3: 简化 `LlmSettings`**

移除以下本地专属字段：
- `model_path` (line 595)
- `context_size` (line 601)
- `batch_size` (line 604)
- `threads` (line 628)
- `gpu_layers` (line 631)
- `enable_thinking` (line 634)
- `auto_load` (line 637)

保留的字段：
- `enabled`
- `provider`
- `system_prompt`
- `max_tokens`
- `temperature`
- `top_p`
- `top_k`
- `min_p`
- `repeat_penalty`
- `seed`
- `base_url`
- `api_key`
- `model`

**Step 4: 更新测试**

移除以下测试：
- `test_model_library_settings_roundtrip`
- 所有涉及 `ModelLibrarySettings` 和 `LocalModel` 的测试

**Step 5: 运行编译检查**

```bash
cargo check 2>&1
```
Expected: 编译通过（`src-tauri` 的编译错误后续阶段处理）

### Task 2.3: 运行根 crate 测试

**Step 1: 运行测试**

```bash
cargo test -p zapmomo 2>&1
```
Expected: 所有保留的测试通过（可能有部分测试因结构调整失败，逐一修复或删除）

---

## 阶段 3：Tauri 命令层

### Task 3.1: 移除 `src-tauri/src/lib.rs` 中的 model_library 导入

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: 移除导入**

移除以下 use 语句 (lines 34-45):
```rust
use zapmomo::model_library;
use zapmomo::model_library::catalog::{CatalogPage, CatalogQuery, RemoteModelDetail};
use zapmomo::model_library::download::{
    DownloadArtifactRequest, DownloadConfig, DownloadEventSink, DownloadManager, DownloadTaskView,
    UreqFileDownloader,
};
use zapmomo::model_library::huggingface::HfApiClient;
use zapmomo::model_library::{
    InstallState as LibInstallState, LibraryModel, RuntimeAction as LibRuntimeAction,
    SetCurrentResult, SystemResources, registry::ModelType as LibModelType,
    storage::StorageInfoView,
};
```

同时移除 `LlmSettings` 导入中不再需要的类型（但 `LlmSettings` 本身仍在使用）。

### Task 3.2: 重写 `LlmConfigInfo` 和 `get_llm_config`

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: 重写 `LlmConfigInfo` 结构体**

```rust
#[derive(Serialize)]
struct LlmConfigInfo {
    enabled: bool,
    provider: String,
    ready: bool,
    settings_path: String,
    system_prompt: String,
    params: GenParams,
    /// 远程 provider 的 base URL
    base_url: Option<String>,
    /// 远程 provider 的 API key（脱敏展示）
    api_key_masked: Option<String>,
    /// 远程 provider 的模型名
    model: Option<String>,
}
```

**Step 2: 重写 `get_llm_config` 命令**

```rust
#[tauri::command]
fn get_llm_config(state: State<'_, LlmState>) -> Result<LlmConfigInfo, String> {
    let cfg = llm_resolved_config()?;
    let ready = state
        .engine
        .lock()
        .ok()
        .and_then(|e| e.as_ref().map(|e| e.is_ready()))
        .unwrap_or(true); // 远程 provider 始终 ready
    Ok(LlmConfigInfo {
        enabled: cfg.enabled,
        provider: cfg.provider,
        ready,
        settings_path: zapmomo::config::settings::get_settings_path()
            .display()
            .to_string(),
        system_prompt: cfg.system_prompt,
        params: cfg.params,
        base_url: cfg.base_url,
        api_key_masked: cfg.api_key.as_ref().map(|k| mask_api_key(k)),
        model: cfg.model,
    })
}

fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}****{}", &key[..4], &key[key.len()-4..])
    }
}
```

### Task 3.3: 简化 `load_llm_impl` / `load_llm_model` / `unload_llm_model`

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: 简化 `load_llm_impl`**

远程 provider 无需加载模型文件，`load_llm_impl` 简化为创建/替换引擎实例：

```rust
fn load_llm_impl(app: AppHandle, state: &LlmState) -> Result<(), String> {
    if app.state::<VoiceSessionState>().is_running() && llm_engine_is_generating(state) {
        return Err("语音会话正在使用 LLM 生成回复，请稍候再重连。".to_string());
    }
    let cfg = llm_resolved_config()?;
    // 远程 provider 校验：必须配置 base_url 和 model
    if cfg.base_url.is_none() {
        return Err("未配置 API 地址（base_url），请在设置中填写。".to_string());
    }
    if cfg.model.is_none() {
        return Err("未配置模型名（model），请在设置中填写。".to_string());
    }
    let engine = Arc::new(zapmomo::llm::LlmEngine::new(cfg).map_err(|e| e.to_string())?);
    engine.load().map_err(|e| e.to_string())?;
    *state.engine.lock().expect("llm lock poisoned") = Some(engine.clone());
    std::thread::spawn(move || forward_llm_events(app, engine.subscribe(), false));
    Ok(())
}
```

**Step 2: 简化 `chat_llm`**

`chat_llm` 中移除 `engine.is_ready()` 检查（远程 provider 始终 ready），保留模型未加载的检查：

```rust
#[tauri::command]
fn chat_llm(state: State<'_, LlmState>, text: String) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("文本不能为空".to_string());
    }
    let cfg = llm_resolved_config()?;
    let engine = state
        .engine
        .lock()
        .expect("llm lock poisoned")
        .clone()
        .ok_or("模型未连接，请先点击「连接」".to_string())?;
    let input = vec![InputItem::Message(ChatMessage::new(ChatRole::User, text))];
    engine
        .generate(input, cfg.params)
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

### Task 3.4: 移除模型库相关命令

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: 移除的命令**

删除以下整个函数及其 `#[tauri::command]` 属性：
- `download_llm_model` (lines 1413-1495)
- `set_llm_model_path` (lines 2548-2561)
- `set_llm_thinking` (lines 2565-2571)
- `set_llm_auto_load` (lines 2575-2581)
- `list_model_library` (lines 4528-4567)
- `get_system_resources` (lines 4569-4575)
- `download_library_model` (lines 4577-4654)
- `cancel_model_download` (lines 4656-4688)
- `set_current_model` (lines 4690-4948)
- `delete_model` (lines 4950-5053)
- `remove_local_model` (~line 5250)
- `add_local_model` (~line 5275)
- `get_storage_info` (~line 5086)
- `set_data_dir` (~line 5104)
- `get_catalog_page` (如果存在)
- `get_model_detail` (如果存在)

**Step 2: 移除 `ModelLibraryState` 结构体**

删除 `ModelLibraryState` 结构体定义和实现。

**Step 3: 移除 `LlmState` 中不再需要的字段**

`LlmState` 中的 `switch_in_progress` 和 `switch_target_path` 字段用于模型切换事务，远程 provider 无切换概念，可以移除。

简化后的 `LlmState`:

```rust
struct LlmState {
    engine: Arc<Mutex<Option<Arc<LlmEngine>>>>,
}

impl LlmState {
    fn new() -> Self {
        Self {
            engine: Arc::new(Mutex::new(None)),
        }
    }
}
```

移除 `LlmSwitchGuard` 结构体。

### Task 3.5: 更新 Tauri 命令注册

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: 更新 `invoke_handler`**

在 `main()` 函数中找到 `tauri::generate_handler![]` 宏调用，移除已删除的命令，保留：

```rust
.invoke_handler(tauri::generate_handler![
    // LLM
    get_llm_config,
    load_llm_model,
    unload_llm_model,
    chat_llm,
    stop_llm,
    is_llm_ready,
    set_llm_params,
    set_llm_system_prompt,
    // 其余保留的命令...
])
```

### Task 3.6: 更新 Voice Session 中的 LLM 预检

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: 更新 `preflight_voice_models`**

移除 LLM 模型文件检查 (lines 2090-2095)：

```rust
// 删除以下代码:
if !cfg.llm.model_path.is_file() {
    return Err(format!(
        "LLM 模型文件不存在: {}",
        cfg.llm.model_path.display()
    ));
}
```

改为检查远程配置：

```rust
// 远程 LLM 配置校验：voice 会话需要 base_url 和 model
if cfg.llm.base_url.is_none() {
    return Err("语音会话需要配置 LLM API 地址（base_url），请在设置中填写。".to_string());
}
if cfg.llm.model.is_none() {
    return Err("语音会话需要配置 LLM 模型名（model），请在设置中填写。".to_string());
}
```

### Task 3.7: 更新 `setup()` 中的 LLM 自动加载逻辑

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: 移除或简化 `auto_load` 逻辑**

在 `setup()` 函数中，查找 LLM 自动加载相关代码（约 line 1873-1890），移除 `auto_load` 判断。远程 provider 可以在 voice session 启动时自动创建引擎，无需预加载。

### Task 3.8: 更新 data_dir 迁移相关代码

**Files:**
- Modify: `src-tauri/src/lib.rs`

移除 `StorageMigrateState` 和 `set_data_dir`、`get_storage_info` 相关代码（如果这些仅服务于模型库）。

---

## 阶段 4：前端

### Task 4.1: 移除模型库相关组件和 hooks

**Files:**
- Delete: `src-tauri/frontend/src/hooks/useModelLibrary.ts`
- Delete: `src-tauri/frontend/src/hooks/useLlmPresets.ts`
- Delete: `src-tauri/frontend/src/hooks/useLlmModelDownload.ts`
- Delete: `src-tauri/frontend/src/hooks/useModelDownload.ts`
- Delete: `src-tauri/frontend/src/hooks/useModelDownloads.ts`
- Delete: `src-tauri/frontend/src/types/modelLibrary.ts`
- Delete: `src-tauri/frontend/src/types/catalog.ts`
- Delete: `src-tauri/frontend/src/pages/models/LibraryPage.tsx`
- Delete: `src-tauri/frontend/src/components/library/` (整个目录)
- Delete: `src-tauri/frontend/src/components/llm/LlmPresetDialog.tsx`
- Delete: `src-tauri/frontend/src/components/llm/useLlmModelPicker.ts`

### Task 4.2: 简化 LLM 相关类型

**Files:**
- Modify: `src-tauri/frontend/src/types/tauri.ts`

**Step 1: 简化 `LlmConfigInfo`**

```typescript
export interface LlmConfigInfo {
  enabled: boolean;
  provider: string;
  ready: boolean;
  settings_path: string;
  system_prompt: string;
  params: LlmParams;
  base_url: string | null;
  api_key_masked: string | null;
  model: string | null;
}
```

**Step 2: 简化 `LlmParams`**

```typescript
export interface LlmParams {
  max_tokens: number;
  temperature: number;
  top_p: number;
  top_k: number;
  min_p: number;
  repeat_penalty: number;
  seed: number;
}
```

**Step 3: 简化 `LlmParamsPatch`**

```typescript
export interface LlmParamsPatch {
  max_tokens?: number;
  temperature?: number;
  top_p?: number;
  top_k?: number;
  min_p?: number;
  repeat_penalty?: number;
  seed?: number;
}
```

**Step 4: 移除 `LlmDownloadResult`**

### Task 4.3: 更新 `useLlm.ts` hook

**Files:**
- Modify: `src-tauri/frontend/src/hooks/useLlm.ts`

**Step 1: 移除本地模型相关状态和方法**

- 移除 `setThinking` 方法（thinking 模式是 Qwen3 本地专属）
- 移除 `setAutoLoad` 方法
- 将 `load` 重命名为 `connect`（或保留 `load` 但修改提示文案）
- 将 `unload` 重命名为 `disconnect`（或保留 `unload`）

### Task 4.4: 重写 LLM 设置页

**Files:**
- Modify: `src-tauri/frontend/src/pages/models/LlmPage.tsx`

**Step 1: 重新设计页面布局**

新的 LLM 设置页应该包含：

1. **连接状态指示器**：显示是否已连接（绿色/灰色）
2. **API 地址输入框**：`base_url`（如 `https://open.bigmodel.cn/api/paas/v4`）
3. **API Key 输入框**：密码类型，显示时脱敏
4. **模型名输入框**：`model`（如 `glm-4.7-flash`）
5. **System Prompt 编辑器**：保留现有的 `LlmSystemPrompt` 组件
6. **高级参数**：保留 temperature、max_tokens、top_p 等（简化的 `LlmAdvancedParams`）
7. **连接/断开按钮**：替代原来的加载/卸载
8. **测试对话**：保留现有的 `LlmTestDialog`

**Step 2: 添加保存逻辑**

新增 `set_llm_connection` 命令（或复用现有的 `set_llm_params` 扩展），保存 `base_url`、`api_key`、`model` 到 settings。

### Task 4.5: 更新 `llmMeta.ts`

**Files:**
- Modify: `src-tauri/frontend/src/components/llm/llmMeta.ts`

**Step 1: 简化 `llmStatus` 函数**

移除 `models_present` 判断，改为根据 `base_url` 和 `model` 是否配置来判断：

```typescript
export function llmStatus(
  cfg: LlmConfigInfo | null,
  st: { ready: boolean; loading: boolean; generating: boolean; error: string | null },
): { tone: LlmStatusTone; label: string } {
  if (st.error) return { tone: "error", label: "错误" };
  if (st.loading) return { tone: "loading", label: "连接中" };
  if (st.generating) return { tone: "loading", label: "生成中" };
  if (st.ready) return { tone: "good", label: "已连接" };
  if (cfg?.base_url && cfg?.model) return { tone: "idle", label: "未连接" };
  return { tone: "idle", label: "未配置" };
}
```

**Step 2: 移除不再需要的函数**

- 移除 `modelNameFromPath`
- 移除 `currentModelName`
- `isHttpProvider` 可以保留（现在始终返回 true）

### Task 4.6: 更新 `LlmCoreConfig.tsx`

**Files:**
- Modify: `src-tauri/frontend/src/components/llm/LlmCoreConfig.tsx`

**Step 1: 重新设计组件**

移除模型路径选择、预设下载按钮、auto-load 开关、thinking 开关。改为：
- API 地址输入（带常用预设下拉：智谱、DeepSeek、OpenRouter、自定义）
- API Key 输入（密码框）
- 模型名输入

### Task 4.7: 更新 `LlmAdvancedParams.tsx`

**Files:**
- Modify: `src-tauri/frontend/src/components/llm/LlmAdvancedParams.tsx`

移除 `context_size`, `batch_size`, `threads`, `gpu_layers` 参数编辑器。保留 `temperature`, `top_p`, `top_k`, `min_p`, `repeat_penalty`, `max_tokens`, `seed`。

### Task 4.8: 更新 `LlmRunControl.tsx`

**Files:**
- Modify: `src-tauri/frontend/src/components/llm/LlmRunControl.tsx`

将 "加载模型" / "卸载模型" 文案改为 "连接" / "断开连接"。

### Task 4.9: 更新 Tauri API 绑定

**Files:**
- Modify: `src-tauri/frontend/src/lib/tauri.ts`

**Step 1: 移除已删除命令的 API 函数**

移除：
- `downloadLlmModel`
- `setLlmModelPath`
- `setLlmThinking`
- `setLlmAutoLoad`
- `listModelLibrary`
- `downloadLibraryModel`
- `cancelModelDownload`
- `setCurrentModel`
- `deleteModel`
- `removeLocalModel`
- `addLocalModel`
- `getSystemResources`
- `getStorageInfo`
- `setDataDir`

**Step 2: 添加新的 API 函数**

添加：
- `setLlmConnection({ base_url, api_key, model })` — 保存远程连接配置
- `getLlmConfig` 的返回类型更新

### Task 4.10: 更新路由和导航

**Files:**
- Modify: `src-tauri/frontend/src/App.tsx`

移除模型库页面（LibraryPage）的路由和导航入口。

### Task 4.11: 清理其他引用

**Step 1: 搜索并移除所有对已删除模块的引用**

```bash
cd src-tauri/frontend
grep -r "modelLibrary\|ModelLibrary\|model_library\|catalog\|LibraryPage\|LlmPreset\|LlmModelDownload\|useModelDownload\|useModelLibrary\|useLlmPresets" src/ --include="*.ts" --include="*.tsx"
```

逐一清理找到的引用。

---

## 阶段 5：测试与验证

### Task 5.1: 更新 `src/llm/mod.rs` 中的测试

**Files:**
- Modify: `src/llm/mod.rs` (tests)

`test_subscribe_each_receiver_gets_copy` 和 `test_generate_mutual_exclusion` 测试中使用了 `cfg.model_path`，需要更新。

### Task 5.2: 更新 `src/config/settings.rs` 中的测试

**Files:**
- Modify: `src/config/settings.rs`

移除 `test_model_library_settings_roundtrip` 测试。

### Task 5.3: 运行完整 Rust 测试套件

```bash
cargo test -p zapmomo 2>&1
```
Expected: 所有保留的测试通过

### Task 5.4: 运行 Tauri crate 编译检查

```bash
cargo check -p zapmomo-app 2>&1
```
Expected: 编译通过

### Task 5.5: 运行前端类型检查

```bash
cd src-tauri/frontend
npx tsc -b 2>&1
```
Expected: 类型检查通过

### Task 5.6: 运行前端测试

```bash
cd src-tauri/frontend
npx vitest run 2>&1
```
Expected: 所有保留的测试通过

### Task 5.7: 代码质量检查

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo clippy -p zapmomo-app -- -D warnings
```
Expected: 全部通过

### Task 5.8: 完整构建验证

```bash
cargo build 2>&1
```
Expected: 构建成功

---

## 阶段 6：清理

### Task 6.1: 移除 `models/` 目录中的 LLM 模型注册

**Files:**
- Modify: `models/manifest.json` — 移除 LLM 相关条目
- Modify: `models/verified_registry.json` — 移除 LLM 相关条目

或保留这些文件不做修改（它们不影响编译，只是不再被引用）。

### Task 6.2: 最终检查

```bash
# 确认无残留引用
grep -r "llama.cpp\|llama-cpp\|llama_cpp" src/ src-tauri/src/ --include="*.rs" 2>/dev/null
grep -r "model_library\|ModelLibrary" src/ src-tauri/src/ --include="*.rs" 2>/dev/null
grep -r "gguf\|GGUF" src/ src-tauri/src/ --include="*.rs" 2>/dev/null
grep -r "encoding_rs\|sysinfo" Cargo.toml src-tauri/Cargo.toml 2>/dev/null
```
Expected: 无输出（无残留引用）

---

## 验证方案

1. **单元测试**: `cargo test -p zapmomo` — 所有保留的测试通过
2. **编译检查**: `cargo check -p zapmomo-app` — Tauri 应用编译通过
3. **前端类型检查**: `npx tsc -b` — 前端类型检查通过
4. **前端测试**: `npx vitest run` — 前端测试通过
5. **代码规范**: `cargo fmt --check && cargo clippy -- -D warnings` — 格式和 lint 通过
6. **手动验证**: 启动应用后，在 LLM 设置页填写 API 地址和 Key，测试对话功能