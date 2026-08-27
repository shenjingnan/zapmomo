# Qwen3-TTS (audio.cpp) 接入实施方案

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 接入 Qwen3-TTS 0.6B Base 与 1.7B Base 两个音色克隆 TTS 模型到 audio.cpp sidecar 后端，复用现有句级流水线实现低首响延迟。

**Architecture:** 新增两个 `TtsModelKind`（audiocpp-only kind）+ families 表两条族描述。qwen3_tts 上游仅支持 offline 模式（无 SSE 流式），首字延迟由现有「LLM 切句 -> SynthHandle 逐句合成 -> 边合成边播放」流水线保证。音色克隆语义与 omnivoice 相同（`voice_ref` + `reference_text`），但 Base 版**必须**提供参考音频（无 auto voice 兜底），需新增 `ReferenceCloneRequired` 音色语义变体做提前拦截。

**Tech Stack:** Rust（lib crate）+ audio.cpp release-0.6.1（上游已原生支持 qwen3_tts family）+ React/TS 前端

---

## 背景与已验证事实（2026-08-26 调研）

| 事实 | 结论 | 验证方式 |
|------|------|----------|
| 上游支持 | audio.cpp release-0.6.1 含 `model_specs/qwen3_tts.json` + `src/models/qwen3_tts/` 完整实现 | 克隆仓库核实 |
| GGUF 自包含 | 两个 GGUF 均内嵌全部 11 个 sidecar 文件（config.json/vocab.json/merges.txt 等，~4.4MB blob） | 解析 GGUF 头部 `audiocpp.embedded_files.names` KV |
| 采样率 | 24000 Hz（`kSampleRate = 24000`，speaker_encoder/speech_decoder/encoder 三处一致） | 源码核实 |
| 流式 | `modes: ["offline"]` 仅离线；无 SSE | spec 核实 |
| 克隆请求体 | `voice_ref`（路径）+ `reference_text`，与 omnivoice 完全一致 | server README + runtime.cpp 核实 |
| **无音色行为** | **`Qwen3 base TTS requires voice clone reference audio`--Base 版必须参考音频，无 auto voice** | session.cpp:472 核实 |
| load_options | 无特殊项（`{}`） | server README 示例 |
| License | Apache-2.0（Qwen/Qwen3-TTS-12Hz-*-Base） | HF API 核实 |

### 模型资产清单

| 项 | 0.6B | 1.7B |
|----|------|------|
| registry id | `tts-qwen3-06b-base-q8-audiocpp` | `tts-qwen3-17b-base-q8-audiocpp` |
| manifest role | `tts-audiocpp-qwen3-06b` | `tts-audiocpp-qwen3-17b` |
| TtsModelKind | `Qwen3Tts06`（serde `qwen3_tts_06`） | `Qwen3Tts17`（serde `qwen3_tts_17`） |
| server model_id | `qwen3-tts-0.6b` | `qwen3-tts-1.7b` |
| GGUF 文件名 | `qwen3-tts-12hz-0.6b-base-q8_0.gguf` | `qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf`（**注意 `_v2` 后缀**） |
| 下载 URL | `https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/main/Qwen3-TTS-12Hz-0.6B-Base-GGUF/qwen3-tts-12hz-0.6b-base-q8_0.gguf` | `https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/main/Qwen3-TTS-12Hz-1.7B-Base-GGUF/qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf` |
| sha256 | `771420bd20ff5f35407b4fa9cf9c5461e153800d3d772ef51c9febc0a520855d` | `b55e06c7890d43c208d15aed8b4ed3f18215f295e47d5960e061b15bff338ab0` |
| size_bytes | 1991211136 (~1.86 GiB) | 2695175104 (~2.51 GiB) |

### E2E 实测回填（2026-08-27，0.6B）

- sidecar：release-0.6.1 重建编入 qwen3_tts（`AUDIOCPP_MODELS=pocket_tts,omnivoice,voxcpm2,qwen3_tts`）
- 模型：0.6B q8_0 GGUF，sha256 校验通过；参考音频 4.8s 中文女声（48kHz 立体声自动处理）
- 结果：**RTF 0.72**（6.96s 音频 / 5.00s 合成，含 server 冷启动 + 模型加载），验收标准 RTF < 1.0 通过
- 结论：0.6B 首响 = 首句合成时间，热 server + 短首句场景预计亚秒级；1.7B 未实测
- 坑：`cargo test` 的 locator 会兜底命中 `~/.zapmomo/engines/` 旧 sidecar（编译进二进制的是 downloader 元数据，`strings` 不能区分有无 qwen3_tts loader），报 `unsupported model family hint: qwen3_tts` 时先替换该目录二进制

### 首响优化分析（回应「尽可能提高首字说话速度」）

qwen3_tts 无上游流式，**首字延迟 = 首句整段合成时间**。现有架构已提供全部可得优化：

1. **句级流水线（已有，零改动）**：`SentenceSplitter` 按标点切句 -> 首句入队即开始合成 -> 播放首句时后续句并行合成。中文对话首句通常 5-15 字，0.6B Metal RTF 预计 ~0.4（同 omnivoice 基座），首响预计 0.3-0.6s。
2. **超长句兜底切分（已有，零改动）**：无标点超 80 字在空白处切分（`DEFAULT_MAX_SENTENCE_LEN`）。
3. **引擎预热（已有，零改动）**：GUI 45s idle keepalive 复用热 server（热请求 0.13s 级），避免冷启动 1-3s。
4. **1.7B 风险提示**：1.7B Metal RTF 预计 ~1.0+，句级流水线可能「播放赶不上合成」出现句间间隙。1.7B 定位为质量优先选项，验收时实测确认；若 RTF 不可用，前端 tagline 注明「建议 16GB+ 内存」。
5. **不做的事（YAGNI）**：不改 SentenceSplitter 增加逗号级切分（句号级已满足需求，用户明确「先按句号拆分即可」）；不做并行合成（SynthHandle 单消费者保序是架构不变量）。

### 改动文件总览

| # | 文件 | 改动 |
|---|------|------|
| 1 | `src/tts/config.rs` | `TtsModelKind` 两个变体 + match 臂 |
| 2 | `src/audiocpp/families.rs` | `ReferenceCloneRequired` 语义变体 + 两条族描述 |
| 3 | `src/audiocpp/client.rs` | `apply_voice_fields` 新语义分支 |
| 4 | `src/tts/voice.rs` | `resolve_voice_params` 无音色拦截 |
| 5 | `src/tts/mod.rs` | `build_offline_model_config` 空臂 + E2E ignored 测试 |
| 6 | `src/model_library/registry.rs` | role 映射 + 测试锚点 |
| 7 | `models/model_registry.json` | 两个条目 |
| 8 | `models/manifest.json` | 两个条目 |
| 9 | `src-tauri/frontend/src/hooks/useTtsModelSwitch.ts` | TTS_PRESETS 两条 |
| 10 | `src-tauri/frontend/src/components/tts/ttsMeta.ts` | kind label 两个 |
| 11 | `scripts/fetch-audiocpp-dev.sh` + `.github/workflows/release.yml` | `AUDIOCPP_MODELS` 加 `qwen3_tts` |
| 12 | `models/THIRD_PARTY_NOTICES.md` | 两个条目 |

---

## Task 1: `TtsModelKind` 新增两个变体

**Files:**
- Modify: `src/tts/config.rs:85-172`（枚举与 impl）
- Test: `src/tts/config.rs`（内联 tests）

**Step 1: 写失败测试**

在 `src/tts/config.rs` 的 `mod tests` 中，`test_model_kind_str_and_semantics` 里追加断言（或新增测试）：

```rust
#[test]
fn test_qwen3_tts_kind_semantics() {
    for (s, kind) in [
        ("qwen3_tts_06", TtsModelKind::Qwen3Tts06),
        ("qwen3_tts_17", TtsModelKind::Qwen3Tts17),
    ] {
        assert_eq!(TtsModelKind::parse_str(s), Some(kind), "{s}");
        assert_eq!(kind.as_str(), s);
    }
    // 克隆语义（kind 级；backend 感知版本见 uses_reference_audio）
    assert!(TtsModelKind::Qwen3Tts06.uses_reference_audio());
    assert!(TtsModelKind::Qwen3Tts17.uses_reference_audio());
}
```

`test_uses_reference_audio_backend_aware` 中追加 audiocpp + qwen3 断言：

```rust
// audiocpp + qwen3_tts -> true（克隆族）
let mut cfg = ResolvedTtsConfig::default();
cfg.backend = TtsBackendKind::Audiocpp;
cfg.model_type = TtsModelKind::Qwen3Tts06;
assert!(cfg.uses_reference_audio());
```

`test_preflight_audiocpp_omnivoice_and_invalid_combo` 同款追加 qwen3 缺文件断言：

```rust
// qwen3：空目录 -> 报缺 gguf（提示语指向 qwen3 registry id）
let mut cfg = ResolvedTtsConfig {
    backend: TtsBackendKind::Audiocpp,
    model_type: TtsModelKind::Qwen3Tts06,
    model_dir: base.path().to_path_buf(),
    ..ResolvedTtsConfig::default()
};
let err = preflight(&cfg).unwrap_err();
assert!(err.contains("qwen3-tts-12hz-0.6b-base-q8_0.gguf"), "err: {err}");
assert!(err.contains("tts-qwen3-06b-base-q8-audiocpp"), "err: {err}");
```

**Step 2: 运行测试验证失败**

Run: `cargo test --manifest-path Cargo.toml -p zapmomo --lib tts::config -- --test-threads=1 2>&1 | tail -20`
Expected: 编译失败（`Qwen3Tts06` 不存在）

**Step 3: 最小实现**

`src/tts/config.rs` 枚举（`Voxcpm2` 之后）：

```rust
    /// Qwen3-TTS 0.6B Base：audio.cpp 后端专用（10 语种音色克隆，24kHz）。
    /// 同款「audiocpp-only kind」语义（见 `Omnivoice` 注释）。
    /// Base 版必须提供克隆参考音频（上游无 auto voice）。
    Qwen3Tts06,
    /// Qwen3-TTS 1.7B Base：audio.cpp 后端专用（质量优先变体）。
    Qwen3Tts17,
```

`as_str`：追加 `"qwen3_tts_06" => Some(Self::Qwen3Tts06)`、`"qwen3_tts_17" => Some(Self::Qwen3Tts17)`。
`parse_str`：对称追加。
`uses_reference_audio`：match 追加 `Self::Qwen3Tts06 | Self::Qwen3Tts17`。

`ResolvedTtsConfig::uses_reference_audio`（config.rs:315-326）Audiocpp 臂：

```rust
            TtsBackendKind::Audiocpp => {
                matches!(
                    self.model_type,
                    TtsModelKind::Omnivoice
                        | TtsModelKind::Voxcpm2
                        | TtsModelKind::Qwen3Tts06
                        | TtsModelKind::Qwen3Tts17
                )
            }
```

**Step 4: 运行测试验证通过**

Run: `cargo test -p zapmomo --lib tts::config -- --test-threads=1 2>&1 | tail -5`
Expected: PASS（preflight 断言仍会失败--families 表还没加，下一 Task 修复；若如此先跳过该断言所在测试，Task 2 后恢复）

**Step 5: Commit**

```bash
git add src/tts/config.rs
git commit -m "feat(tts): TtsModelKind 新增 qwen3_tts_06/17 变体"
```

---

## Task 2: families.rs 族描述表 + `ReferenceCloneRequired` 语义

**Files:**
- Modify: `src/audiocpp/families.rs`
- Test: `src/audiocpp/families.rs`（内联 tests）

**Step 1: 写失败测试**

`test_family_desc_coverage` 的排除列表不包含 Qwen3（它们应有描述）；`test_family_records_shape` 追加：

```rust
// qwen3_tts 两尺寸：24kHz / 强制克隆 / 无流式 / metal / 单文件清单
let q06 = family_desc(TtsModelKind::Qwen3Tts06).unwrap();
assert_eq!(q06.model_id, "qwen3-tts-0.6b");
assert_eq!(q06.family, "qwen3_tts");
assert_eq!(q06.required_files, &["qwen3-tts-12hz-0.6b-base-q8_0.gguf"]);
assert_eq!(q06.sample_rate, 24_000);
assert_eq!(q06.default_provider, "metal");
assert_eq!(
    q06.voice_semantics,
    VoiceSemantics::ReferenceCloneRequired
);
assert!(!q06.allows_named_voice, "Base 版仅接受 speaker reference");
assert!(!q06.supports_streaming, "上游 modes 仅 offline");
assert!(q06.registry_hint.contains("tts-qwen3-06b-base-q8-audiocpp"));
assert_eq!(q06.load_options(), serde_json::json!({}));

let q17 = family_desc(TtsModelKind::Qwen3Tts17).unwrap();
assert_eq!(q17.model_id, "qwen3-tts-1.7b");
assert_eq!(
    q17.required_files,
    &["qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf"]
);
assert_eq!(q17.sample_rate, 24_000);
assert_eq!(q17.default_provider, "metal");
```

**Step 2: 运行验证失败**

Run: `cargo test -p zapmomo --lib audiocpp::families -- --test-threads=1 2>&1 | tail -10`
Expected: FAIL（编译错误：变体与字段不存在）

**Step 3: 实现**

`VoiceSemantics` 枚举追加（families.rs:12-19 之后）：

```rust
    /// 强制参考音频克隆（qwen3_tts Base）：与 [`VoiceSemantics::ReferenceClone`]
    /// 同款 `voice_ref`+`reference_text` 映射，但 `Sid`/缺省**必须拦截**--
    /// 上游 Base 版无 auto voice（实测报错 "requires voice clone reference
    /// audio"），ZapMomo 侧提前报错给中文文案。
    ReferenceCloneRequired,
```

`AudiocppFamilyDesc` 的 `voice_semantics` 字段注释同步更新。

两条族描述（`VOXCPM2` 之后）：

```rust
/// Qwen3-TTS 0.6B Base q8_0（10 语种 3 秒音色克隆，24kHz；Metal 必需）。
///
/// 单文件 GGUF（权重 + speech tokenizer + 全部 sidecar 内嵌，实测
/// `audiocpp.embedded_files` 含 11 个文件）。**Base 版必须参考音频**（无
/// auto voice 兜底，见 `VoiceSemantics::ReferenceCloneRequired`）；CustomVoice/
/// VoiceDesign 变体不在本期接入范围。GGUF 文件名无 `_v2` 后缀。
pub const QWEN3_TTS_06B: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "qwen3-tts-0.6b",
    family: "qwen3_tts",
    gguf_file: "qwen3-tts-12hz-0.6b-base-q8_0.gguf",
    required_files: &["qwen3-tts-12hz-0.6b-base-q8_0.gguf"],
    sample_rate: 24_000,
    default_provider: "metal",
    voice_semantics: VoiceSemantics::ReferenceCloneRequired,
    allows_named_voice: false,
    supports_streaming: false,
    registry_hint: "zapmomo tts install-model --registry-id tts-qwen3-06b-base-q8-audiocpp",
};

/// Qwen3-TTS 1.7B Base q8_0（质量优先变体；GGUF 为上游 `_v2` 重打包版）。
///
/// 同 0.6B 语义；1.7B Metal RTF 预计 ~1.0+，句级流水线可能句间间隙，
/// 定位质量优先（前端 tagline 注明）。
pub const QWEN3_TTS_17B: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "qwen3-tts-1.7b",
    family: "qwen3_tts",
    gguf_file: "qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf",
    required_files: &["qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf"],
    sample_rate: 24_000,
    default_provider: "metal",
    voice_semantics: VoiceSemantics::ReferenceCloneRequired,
    allows_named_voice: false,
    supports_streaming: false,
    registry_hint: "zapmomo tts install-model --registry-id tts-qwen3-17b-base-q8-audiocpp",
};
```

`family_desc` 查表追加：

```rust
        TtsModelKind::Qwen3Tts06 => Some(&QWEN3_TTS_06B),
        TtsModelKind::Qwen3Tts17 => Some(&QWEN3_TTS_17B),
```

`test_family_desc_coverage` 的 sherpa 排除列表保持不含 Qwen3（已正确）。

**Step 4: 运行验证通过（含 Task 1 的 preflight 测试恢复）**

Run: `cargo test -p zapmomo --lib audiocpp:: -- --test-threads=1 2>&1 | tail -5 && cargo test -p zapmomo --lib tts::config -- --test-threads=1 2>&1 | tail -5`
Expected: 全 PASS

**Step 5: Commit**

```bash
git add src/audiocpp/families.rs
git commit -m "feat(audiocpp): families 表新增 qwen3_tts 两尺寸族描述与强制克隆语义"
```

---

## Task 3: client.rs `apply_voice_fields` 强制克隆拦截

**Files:**
- Modify: `src/audiocpp/client.rs:216-257`
- Test: `src/audiocpp/client.rs`（内联 tests）

**Step 1: 写失败测试**

```rust
/// qwen3_tts（强制克隆族）：Reference 正常映射；Sid/Named 提前拦截。
#[test]
fn test_synthesize_qwen3_tts_voice_semantics() {
    let (base_url, received, _handle) = spawn_stub();
    let cfg = ResolvedTtsConfig {
        backend: TtsBackendKind::Audiocpp,
        model_type: crate::tts::config::TtsModelKind::Qwen3Tts06,
        ..ResolvedTtsConfig::default()
    };
    let tts = AudiocppTts::new_with_base_url(cfg, &base_url);

    // Reference -> voice_ref + reference_text（与 omnivoice 同款映射）
    tts.synthesize(
        "你好",
        1.0,
        &TtsVoiceParams::Reference {
            wav_path: std::path::PathBuf::from("/voices/me.wav"),
            reference_text: "参考转写".into(),
        },
    )
    .unwrap();
    let body = received.lock().unwrap().last().unwrap().clone();
    assert_eq!(body["model"], "qwen3-tts-0.6b");
    assert_eq!(
        body["voice_ref"].as_str().unwrap().replace('\\', "/"),
        "/voices/me.wav"
    );
    assert_eq!(body["reference_text"], "参考转写");

    // Sid -> 提前拦截（上游 Base 版无 auto voice）
    let err = tts.synthesize("x", 1.0, &TtsVoiceParams::Sid(0)).unwrap_err();
    assert!(err.contains("需要"), "err: {err}");

    // Named -> 提前拦截（Base 版仅接受 speaker reference）
    let err = tts
        .synthesize("x", 1.0, &TtsVoiceParams::Named("v".into()))
        .unwrap_err();
    assert!(err.contains("参考音频"), "err: {err}");
}
```

**Step 2: 验证失败**

Run: `cargo test -p zapmomo --lib audiocpp::client -- --test-threads=1 2>&1 | tail -10`
Expected: FAIL（`ReferenceCloneRequired` match 臂缺失--编译错误）

**Step 3: 实现**

`apply_voice_fields`（client.rs:221 的 match）追加三个臂：

```rust
        (
            VoiceSemantics::ReferenceCloneRequired,
            TtsVoiceParams::Reference {
                wav_path,
                reference_text,
            },
        ) => {
            body["voice_ref"] = serde_json::json!(wav_path.to_string_lossy());
            body["reference_text"] = serde_json::json!(reference_text);
        }
        (VoiceSemantics::ReferenceCloneRequired, TtsVoiceParams::Sid(_)) => {
            return Err(AudiocppError::UnsupportedVoice(
                "Qwen3-TTS 需要克隆音色：请先在音色库选择或录制一个音色".to_string(),
            ));
        }
        (VoiceSemantics::ReferenceCloneRequired, TtsVoiceParams::Named(_)) => {
            return Err(AudiocppError::UnsupportedVoice(format!(
                "{} 仅支持参考音频克隆（speaker reference），不支持具名音色",
                desc.model_id
            )));
        }
```

**Step 4: 验证通过**

Run: `cargo test -p zapmomo --lib audiocpp::client -- --test-threads=1 2>&1 | tail -5`
Expected: PASS

**Step 5: Commit**

```bash
git add src/audiocpp/client.rs
git commit -m "feat(audiocpp): client 音色映射支持 qwen3_tts 强制克隆拦截"
```

---

## Task 4: voice.rs `resolve_voice_params` 无音色拦截

**Files:**
- Modify: `src/tts/voice.rs:156-193`
- Test: `src/tts/voice.rs`（内联 tests）

**Step 1: 写失败测试**

```rust
/// qwen3_tts 无音色来源时明确报错（omnivoice 走 Sid(0) auto voice，qwen3 不能）。
#[test]
fn test_resolve_voice_params_qwen3_requires_voice() {
    let cfg = ResolvedTtsConfig {
        backend: crate::tts::config::TtsBackendKind::Audiocpp,
        model_type: crate::tts::config::TtsModelKind::Qwen3Tts06,
        ..ResolvedTtsConfig::default()
    };
    let err = resolve_voice_params(&cfg, None, None, None, None).unwrap_err();
    assert!(err.contains("克隆音色"), "err: {err}");

    // 有自定义音色 -> Reference
    let base = tempfile::tempdir().unwrap();
    let wav = base.path().join("my.wav");
    std::fs::write(&wav, sample_wav_bytes()).unwrap();
    let params = resolve_voice_params(&cfg, None, None, Some(&wav), Some("转写")).unwrap();
    match params {
        crate::tts::TtsVoiceParams::Reference { wav_path, reference_text } => {
            assert_eq!(wav_path, wav);
            assert_eq!(reference_text, "转写");
        }
        other => panic!("应为 Reference，got {other:?}"),
    }
}
```

**Step 2: 验证失败**

Run: `cargo test -p zapmomo --lib tts::voice -- --test-threads=1 2>&1 | tail -10`
Expected: FAIL（qwen3 无音色返回了 Sid(0) 而非 Err）

**Step 3: 实现**

`resolve_voice_params` 的 audiocpp 兜底分支改为查族语义（voice.rs:167-170）：

```rust
        if !has_any && cfg.backend == TtsBackendKind::Audiocpp {
            // 按族分派无音色兜底：omnivoice -> auto voice（Sid(0)，client 省略
            // 音色字段）；qwen3_tts Base 上游无 auto voice -> 提前报错
            let desc = crate::audiocpp::families::family_desc(cfg.model_type);
            if desc.is_some_and(|d| {
                matches!(
                    d.voice_semantics,
                    crate::audiocpp::families::VoiceSemantics::ReferenceCloneRequired
                )
            }) {
                return Err(
                    "Qwen3-TTS 需要克隆音色：请先在音色库选择或录制一个音色".to_string(),
                );
            }
            return Ok(TtsVoiceParams::Sid(0));
        }
```

函数文档注释同步：在「参考音频克隆」条目补「qwen3_tts Base 无音色时报错（上游无 auto voice）」。

**Step 4: 验证通过**

Run: `cargo test -p zapmomo --lib tts::voice -- --test-threads=1 2>&1 | tail -5`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tts/voice.rs
git commit -m "feat(tts): 音色解析对 qwen3_tts 无音色场景明确报错"
```

---

## Task 5: tts/mod.rs 空臂 + E2E ignored 测试

**Files:**
- Modify: `src/tts/mod.rs:131`（`build_offline_model_config` match）
- Modify: `src/tts/mod.rs`（tests，`test_omnivoice_synthesize_produces_audio` 之后）

**Step 1: 修改 match 臂**

```rust
        // audiocpp-only 族：无 sherpa 配置分支（引擎构造在 AudiocppTts，
        // preflight 已按族清单拦截非法组合）
        TtsModelKind::Omnivoice | TtsModelKind::Voxcpm2
        | TtsModelKind::Qwen3Tts06 | TtsModelKind::Qwen3Tts17 => {}
```

（若格式化后合并方式不同，遵循 `cargo fmt` 结果。）

**Step 2: 追加 E2E ignored 测试**

```rust
    #[test]
    #[ignore = "需要 qwen3-tts GGUF 在 QWEN3_TTS_E2E_DIR 目录 + audiocpp 引擎可定位 + 参考音频 QWEN3_TTS_E2E_REF"]
    fn test_qwen3_tts_synthesize_produces_audio() {
        // E2E：QWEN3_TTS_E2E_DIR=/path/to/qwen3-tts QWEN3_TTS_E2E_REF=/path/to/ref.wav \
        //   QWEN3_TTS_E2E_REF_TEXT="转写" cargo test -- --ignored
        let Some(dir) = std::env::var("QWEN3_TTS_E2E_DIR").ok() else {
            eprintln!("跳过：未设置 QWEN3_TTS_E2E_DIR");
            return;
        };
        let Some(ref_wav) = std::env::var("QWEN3_TTS_E2E_REF").ok() else {
            eprintln!("跳过：未设置 QWEN3_TTS_E2E_REF（Base 版必须参考音频）");
            return;
        };
        let kind = match std::env::var("QWEN3_TTS_E2E_SIZE").as_deref() {
            Ok("17") => TtsModelKind::Qwen3Tts17,
            _ => TtsModelKind::Qwen3Tts06,
        };
        let cfg = config::ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: kind,
            model_dir: PathBuf::from(&dir),
            provider: std::env::var("QWEN3_TTS_E2E_PROVIDER")
                .unwrap_or_else(|_| "metal".to_string()),
            ..config::ResolvedTtsConfig::default()
        };
        let engine = TtsEngine::new(cfg).unwrap();
        assert_eq!(engine.sample_rate(), 24_000, "qwen3_tts 固定 24kHz");
        assert!(!engine.supports_streaming(), "qwen3_tts 无流式");

        let voice = TtsVoiceParams::Reference {
            wav_path: PathBuf::from(ref_wav),
            reference_text: std::env::var("QWEN3_TTS_E2E_REF_TEXT").unwrap_or_else(|_| {
                "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.".to_string()
            }),
        };
        let started = std::time::Instant::now();
        let samples = engine
            .synthesize(
                "你好，我是 ZapMomo 语音伙伴，正在验证 Qwen3-TTS 中文合成。",
                1.0,
                &voice,
            )
            .unwrap();
        let elapsed = started.elapsed().as_secs_f32();
        assert!(!samples.is_empty(), "合成音频不应为空");
        let duration = samples.len() as f32 / engine.sample_rate() as f32;
        eprintln!(
            "qwen3_tts e2e ({:?}): {:.2}s 音频 / {:.2}s 合成 (RTF {:.2})",
            kind,
            duration,
            elapsed,
            elapsed / duration
        );
    }
```

**Step 3: 验证编译 + 测试**

Run: `cargo test -p zapmomo --lib tts:: -- --test-threads=1 2>&1 | tail -5`
Expected: PASS（E2E 测试 ignored 跳过）

**Step 4: Commit**

```bash
git add src/tts/mod.rs
git commit -m "feat(tts): qwen3_tts audiocpp-only 空臂与 E2E 测试"
```

---

## Task 6: registry.rs role 映射 + 测试锚点

**Files:**
- Modify: `src/model_library/registry.rs:217-220`（`required_files_for_role`）
- Modify: `src/model_library/registry.rs:250`（数量锚点）与 `registry_tts_kind` 断言区
- Test: `src/model_library/registry.rs`（内联 tests）

**Step 1: 写失败测试**

`required_files_for_role` 测试追加：

```rust
assert_eq!(
    required_files_for_role("tts-audiocpp-qwen3-06b"),
    &[crate::audiocpp::families::QWEN3_TTS_06B.gguf_file]
);
assert_eq!(
    required_files_for_role("tts-audiocpp-qwen3-17b"),
    &[crate::audiocpp::families::QWEN3_TTS_17B.gguf_file]
);
```

数量锚点（registry.rs:250）把 `+ 2 个 Qwen3-TTS` 追加进期望文案，总数 +2。

`registry_tts_kind` 断言区追加：

```rust
    assert_eq!(
        registry_tts_kind("tts-qwen3-06b-base-q8-audiocpp"),
        Some(TtsModelKind::Qwen3Tts06)
    );
    assert_eq!(
        registry_tts_kind("tts-qwen3-17b-base-q8-audiocpp"),
        Some(TtsModelKind::Qwen3Tts17)
    );
```

**Step 2: 验证失败**

Run: `cargo test -p zapmomo --lib model_library::registry -- --test-threads=1 2>&1 | tail -10`
Expected: FAIL（role 未映射 / registry 条目不存在）

**Step 3: 实现**

`required_files_for_role` 追加（`tts-audiocpp-voxcpm2` 行后）：

```rust
        "tts-audiocpp-qwen3-06b" => &[crate::audiocpp::families::QWEN3_TTS_06B.gguf_file],
        "tts-audiocpp-qwen3-17b" => &[crate::audiocpp::families::QWEN3_TTS_17B.gguf_file],
```

数量锚点更新（下一个 Task 加 JSON 条目后才会通过）。

**Step 4: 验证（JSON 条目加入后）**

Run: `cargo test -p zapmomo --lib model_library::registry -- --test-threads=1 2>&1 | tail -5`
Expected: PASS（与 Task 7 联动）

**Step 5: Commit**

```bash
git add src/model_library/registry.rs
git commit -m "feat(registry): qwen3-tts 两尺寸 role 映射与测试锚点"
```

---

## Task 7: model_registry.json + manifest.json 条目

**Files:**
- Modify: `models/model_registry.json`（`tts-voxcpm2-q8-audiocpp` 条目之后）
- Modify: `models/manifest.json`（`tts-audiocpp-voxcpm2` 条目之后）

**Step 1: model_registry.json 追加两个条目**

```json
    {
      "id": "tts-qwen3-06b-base-q8-audiocpp",
      "name": "qwen3-tts-06b-base-audiocpp",
      "display_name": "Qwen3-TTS 0.6B 音色克隆 (audio.cpp)",
      "model_type": "tts",
      "tts_kind": "qwen3_tts_06",
      "runtime": "audiocpp",
      "format": "GGUF",
      "description": "Qwen3-TTS 0.6B Base（12Hz）零样本音色克隆，10 语种，q8_0 量化，24kHz；由 audio.cpp 引擎驱动。体积约 1.86 GiB，推理依赖 Metal 加速，建议 8GB 以上内存的 Apple Silicon 设备使用。需配合音色库克隆音色（Base 版无内置音色）。",
      "languages": ["zh", "en", "ja", "ko", "de", "fr", "ru", "pt", "es", "it"],
      "tags": ["tts", "audiocpp", "clone", "multilingual"],
      "parameter_count": "0.6B",
      "quantization": "q8_0",
      "version": "q8_0",
      "size_bytes": 1991211136,
      "homepage": "https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-Base",
      "required_assets": ["tts-audiocpp-qwen3-06b"],
      "optional_assets": [],
      "platforms": ["darwin-aarch64"],
      "download": {
        "manifest_role": "tts-audiocpp-qwen3-06b",
        "extra_roles": [],
        "kind": "raw"
      }
    },
    {
      "id": "tts-qwen3-17b-base-q8-audiocpp",
      "name": "qwen3-tts-17b-base-audiocpp",
      "display_name": "Qwen3-TTS 1.7B 音色克隆 (audio.cpp)",
      "model_type": "tts",
      "tts_kind": "qwen3_tts_17",
      "runtime": "audiocpp",
      "format": "GGUF",
      "description": "Qwen3-TTS 1.7B Base（12Hz）零样本音色克隆，10 语种，q8_0 量化（_v2 打包），24kHz；由 audio.cpp 引擎驱动。质量优先变体，体积约 2.51 GiB，推理依赖 Metal 加速，建议 16GB 以上内存的 Apple Silicon 设备使用。需配合音色库克隆音色（Base 版无内置音色）。",
      "languages": ["zh", "en", "ja", "ko", "de", "fr", "ru", "pt", "es", "it"],
      "tags": ["tts", "audiocpp", "clone", "multilingual"],
      "parameter_count": "1.7B",
      "quantization": "q8_0",
      "version": "q8_0_v2",
      "size_bytes": 2695175104,
      "homepage": "https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-Base",
      "required_assets": ["tts-audiocpp-qwen3-17b"],
      "optional_assets": [],
      "platforms": ["darwin-aarch64"],
      "download": {
        "manifest_role": "tts-audiocpp-qwen3-17b",
        "extra_roles": [],
        "kind": "raw"
      }
    }
```

**Step 2: manifest.json 追加两个条目**

```json
    {
      "name": "qwen3-tts-06b-base-audiocpp",
      "role": "tts-audiocpp-qwen3-06b",
      "version": "q8_0",
      "kind": "raw",
      "archive": "qwen3-tts-12hz-0.6b-base-q8_0.gguf",
      "source": "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/main/Qwen3-TTS-12Hz-0.6B-Base-GGUF/qwen3-tts-12hz-0.6b-base-q8_0.gguf",
      "sha256": "771420bd20ff5f35407b4fa9cf9c5461e153800d3d772ef51c9febc0a520855d",
      "size_bytes": 1991211136,
      "license": "Apache-2.0"
    },
    {
      "name": "qwen3-tts-17b-base-audiocpp",
      "role": "tts-audiocpp-qwen3-17b",
      "version": "q8_0_v2",
      "kind": "raw",
      "archive": "qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf",
      "source": "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/main/Qwen3-TTS-12Hz-1.7B-Base-GGUF/qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf",
      "sha256": "b55e06c7890d43c208d15aed8b4ed3f18215f295e47d5960e061b15bff338ab0",
      "size_bytes": 2695175104,
      "license": "Apache-2.0"
    }
```

注意 JSON 语法：`voxcpm2` 条目是 manifest 最后一个（需把它的 `}` 后加逗号）。

**Step 3: 验证（含 Task 6 的 registry 测试）**

Run: `cargo test -p zapmomo --lib model_library:: -- --test-threads=1 2>&1 | tail -5`
Expected: 全 PASS（数量锚点 +2、role 映射、tts_kind 断言）

**Step 4: Commit**

```bash
git add models/model_registry.json models/manifest.json
git commit -m "feat(models): qwen3-tts 两尺寸 registry 与 manifest 下载源"
```

---

## Task 8: 前端 preset + kind label

**Files:**
- Modify: `src-tauri/frontend/src/hooks/useTtsModelSwitch.ts:8-73`（TTS_PRESETS）
- Modify: `src-tauri/frontend/src/components/tts/ttsMeta.ts:38-57`（ttsModelKindLabel）
- Test: `src-tauri/frontend/src/components/tts/ttsMeta.test.ts`

**Step 1: 写失败测试**

`ttsMeta.test.ts` 的 preset 测试追加（对齐 omnivoice/voxcpm2 断言模式）：

```typescript
  const q06 = TTS_PRESETS.find((p) => p.id === "tts-qwen3-06b-base-q8-audiocpp");
  expect(q06).toBeDefined();
  expect(q06?.kind).toBe("qwen3_tts_06");

  const q17 = TTS_PRESETS.find((p) => p.id === "tts-qwen3-17b-base-q8-audiocpp");
  expect(q17).toBeDefined();
  expect(q17?.kind).toBe("qwen3_tts_17");

  expect(ttsModelKindLabel("qwen3_tts_06")).toBe("Qwen3-TTS 克隆");
  expect(ttsModelKindLabel("qwen3_tts_17")).toBe("Qwen3-TTS 克隆");
```

**Step 2: 验证失败**

Run: `cd src-tauri/frontend && pnpm vitest run src/components/tts/ttsMeta.test.ts 2>&1 | tail -10`
Expected: FAIL（preset 不存在）

**Step 3: 实现**

`useTtsModelSwitch.ts` TTS_PRESETS 追加（voxcpm2 之后）：

```typescript
  {
    id: "tts-qwen3-06b-base-q8-audiocpp",
    name: "Qwen3-TTS 0.6B 克隆",
    kind: "qwen3_tts_06",
    languages: "多语种",
    tagline: "audio.cpp 引擎 · 声音克隆 · 10 语种 · 24kHz · 仅 Apple Silicon（Metal）· 需选择克隆音色",
    sizeBytes: 1_991_211_136,
  },
  {
    id: "tts-qwen3-17b-base-q8-audiocpp",
    name: "Qwen3-TTS 1.7B 克隆",
    kind: "qwen3_tts_17",
    languages: "多语种",
    tagline: "audio.cpp 引擎 · 声音克隆 · 10 语种 · 质量优先 · 24kHz · 仅 Apple Silicon（Metal）· 建议 16GB+ 内存 · 需选择克隆音色",
    sizeBytes: 2_695_175_104,
  },
```

`ttsMeta.ts` `ttsModelKindLabel` 追加：

```typescript
    case "qwen3_tts_06":
    case "qwen3_tts_17":
      return "Qwen3-TTS 克隆";
```

**Step 4: 验证通过**

Run: `cd src-tauri/frontend && pnpm vitest run src/components/tts/ttsMeta.test.ts 2>&1 | tail -5`
Expected: PASS

**Step 5: Commit**

```bash
git add src-tauri/frontend/src/hooks/useTtsModelSwitch.ts src-tauri/frontend/src/components/tts/ttsMeta.ts src-tauri/frontend/src/components/tts/ttsMeta.test.ts
git commit -m "feat(frontend): qwen3-tts 两尺寸模型预设与类型徽标"
```

---

## Task 9: 构建脚本与 CI 加入 qwen3_tts family

**Files:**
- Modify: `scripts/fetch-audiocpp-dev.sh:55`
- Modify: `.github/workflows/release.yml:156`

**Step 1: 修改**

`fetch-audiocpp-dev.sh`（第 55 行 + 头部注释第 9 行）：

```bash
  -DAUDIOCPP_MODEL_SET=custom -DAUDIOCPP_MODELS=pocket_tts,omnivoice,voxcpm2,qwen3_tts \
```

`release.yml`（第 156 行，`COMMON_FLAGS`）同款追加 `,qwen3_tts`。

**Step 2: 验证（本地重建 sidecar，实测 qwen3_tts 可加载）**

```bash
brew install libomp  # 已装则跳过
scripts/fetch-audiocpp-dev.sh --build
```

Expected: 编译约 2.5-4 分钟，产物 `src-tauri/binaries/audiocpp_server-<triple>` 体积比原来略增（qwen3_tts 模块编译进二进制）。

**Step 3: 手动冒烟（可选但推荐，需要已下载 GGUF）**

```bash
# 假设模型装在 ~/.zapmomo/models/qwen3-tts-06b-base-audiocpp/
# 用 locator 语义直接起 server 验证（参考 audiocpp::server_config 生成 config 的模式）
```

若暂无 GGUF，可在 Task 11 E2E 时统一验证。

**Step 4: Commit**

```bash
git add scripts/fetch-audiocpp-dev.sh .github/workflows/release.yml
git commit -m "chore(build): audio.cpp sidecar 编入 qwen3_tts 模型族"
```

---

## Task 10: THIRD_PARTY_NOTICES.md 条目

**Files:**
- Modify: `models/THIRD_PARTY_NOTICES.md`（`voxcpm2-q8_0.gguf` 条目之后，对齐 omnivoice/voxcpm2 的条目格式）

**Step 1: 追加两个条目**

对齐现有条目格式（标题 = 文件名，含来源/许可证/用途段落）：

```markdown
### qwen3-tts-12hz-0.6b-base-q8_0.gguf / qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf

- **组件**: Qwen3-TTS 12Hz Base（0.6B / 1.7B）q8_0 量化 GGUF（audio.cpp 打包，sidecar 内嵌全部配置文件）
- **来源**: https://huggingface.co/audio-cpp/audio.cpp-gguf（Qwen3-TTS-12Hz-0.6B-Base-GGUF / Qwen3-TTS-12Hz-1.7B-Base-GGUF）
- **上游模型**: https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-Base / https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-Base
- **许可证**: Apache-2.0
- **用途**: TTS 音色克隆（speaker reference）推理，经 audio.cpp sidecar 后端加载
```

**Step 2: Commit**

```bash
git add models/THIRD_PARTY_NOTICES.md
git commit -m "docs(models): qwen3-tts 第三方组件声明"
```

---

## Task 11: E2E 实测验收（ignored 测试 + 首响打点）

**前置**：Task 9 已重建 sidecar；已下载 GGUF（`zapmomo tts install-model --registry-id tts-qwen3-06b-base-q8-audiocpp` 或手动放置）；准备一段参考音频与其转写。

**Step 1: 下载模型并实测**

```bash
cargo run -- tts install-model --registry-id tts-qwen3-06b-base-q8-audiocpp
# 记录真实 RTF 与首响数据（E2E 测试输出）
QWEN3_TTS_E2E_DIR=<模型目录> \
QWEN3_TTS_E2E_REF=<参考音频路径> \
QWEN3_TTS_E2E_REF_TEXT="<参考转写>" \
cargo test -p zapmomo --lib test_qwen3_tts_synthesize_produces_audio -- --ignored --nocapture
```

Expected: `RTF < 1.0`（0.6B Metal；1.7B 记录实际值，若 >1.0 在前端 tagline 已注明）。

**Step 2: 语音会话首响打点（voice run）**

用 voice 会话跑一轮对话，观察日志 `首响打点：唤醒->首句 X 生成->首句 Y 首句->首块 Z`。验收标准（0.6B）：`首句->首块` < 1s（首句 10 字内）。

**Step 3: 结果回填**

把实测 RTF/首响数据补进本方案文档的「模型资产清单」小节（或 memory），供后续调优参考。

---

## Task 12: 全量验证

**Step 1: Rust 全量**

```bash
cargo fmt --check && cargo clippy -p zapmomo -- -D warnings && cargo test -p zapmomo -- --test-threads=1
```

Expected: 全 PASS。

**Step 2: 前端全量**

```bash
cd src-tauri/frontend && pnpm vitest run && pnpm tsc -b
```

Expected: 全 PASS。

**Step 3: tauri crate 检查**

```bash
cargo check -p zapmomo-app && cargo clippy -p zapmomo-app -- -D warnings
```

Expected: 全 PASS（如遇 webkit 依赖缺失按 CONTRIBUTING.md 处理）。

**Step 4: Commit（如有 fmt/clippy 修正）**

```bash
git add -A && git commit -m "style: qwen3-tts 接入 fmt/clippy 修正"
```

---

## 验收清单

- [ ] `cargo test -p zapmomo -- --test-threads=1` 全绿
- [ ] 前端 `pnpm vitest run` 全绿
- [ ] GUI 模型库可见两个 Qwen3-TTS 条目并可下载（macOS arm64）
- [ ] 下载完成后「设为当前模型」可切换，preflight 通过
- [ ] 无音色时合成报明确中文错误（「需要克隆音色」）
- [ ] 选定音色后 GUI 测试合成出声（24kHz）
- [ ] 语音会话（voice run）句级流水线正常，首响打点 < 1s（0.6B）
- [ ] 非 Apple Silicon 平台模型库不显示两模型（platforms 过滤）
- [ ] `scripts/fetch-audiocpp-dev.sh --build` 产出含 qwen3_tts 的 sidecar

## 风险与回退

| 风险 | 缓解 |
|------|------|
| 1.7B Metal RTF > 1（句间间隙） | 前端 tagline 已注明质量定位；实测后可下架或改 platforms |
| 上游 `_v2` GGUF 与后续 release 行为漂移 | 锁定 release-0.6.1 tag（现状）；sha256 校验下载完整性 |
| qwen3_tts 请求体字段与 omnivoice 有隐藏差异 | E2E 测试实测兜底；client 请求体断言测试锚定 |
| sidecar 体积增长影响安装包 | 编译产物实测（预计 +1-2MB，可接受）；必要时 CI 上报体积 |
| CPU 平台用户误选 | platforms: ["darwin-aarch64"] 过滤（与 omnivoice 同款） |
