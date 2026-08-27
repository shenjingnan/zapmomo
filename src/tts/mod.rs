/// 文本转语音（TTS）。
///
/// 门面按 `ResolvedTtsConfig.backend` 分派双后端：
/// - sherpa-onnx `OfflineTts`（进程内，ZipVoice/Vits/Matcha 等中英模型）；
/// - audio.cpp sidecar（`crate::audiocpp`，PocketTTS 等 GGUF 模型经 HTTP）。
///
/// 设计上对齐 KWS/ASR：模型清单下载、配置解析、引擎「逐文件预检 + install-model 提示」
/// 等模式保持一致；进度通过回调（`FnMut(f32) -> bool`）暴露（sherpa 为合成过程
/// 协作回调，audiocpp 为请求前后探询——HTTP 在途请求无法中断）。
pub mod config;
pub mod kokoro_voices;
pub mod reaction;
pub mod voice;
pub mod voice_store;

use crate::audiocpp::client::AudiocppTts;
use config::{ResolvedTtsConfig, TtsBackendKind, TtsModelKind};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKittenModelConfig,
    OfflineTtsKokoroModelConfig, OfflineTtsMatchaModelConfig, OfflineTtsModelConfig,
    OfflineTtsPocketModelConfig, OfflineTtsSupertonicModelConfig, OfflineTtsVitsModelConfig,
    OfflineTtsZipvoiceModelConfig, Wave,
};
use std::path::{Path, PathBuf};

pub use crate::kws::model::{DownloadProgress, DownloadStage, ModelError, ProgressFn};
pub use voice::TtsVoice;

/// 合成时的「说话人/音色」参数：`Sid`（vits/matcha/kokoro 等固定说话人）、
/// `Reference`（ZipVoice 参考音频克隆）或 `Named`（audiocpp 具名音色，如 `alba`）。
#[derive(Debug, Clone, PartialEq)]
pub enum TtsVoiceParams {
    /// speaker id（本期单说话人模型恒 0；多说话人二期扩展）
    Sid(i32),
    /// 参考音频 + 逐字转写（ZipVoice 零样本克隆）
    Reference {
        wav_path: PathBuf,
        reference_text: String,
    },
    /// 具名音色（audio.cpp 后端，如 PocketTTS 的 `alba`；sherpa 后端不支持）
    Named(String),
}

/// 按模型类型构造 sherpa-onnx 的 `OfflineTtsModelConfig` 对应分支。
///
/// 纯函数（不访问文件系统），便于单测各分支字段。registry 未收录的分支
/// （kokoro/kitten/pocket/supertonic）字段照填，但缺文件时在预检阶段报错。
pub(crate) fn build_offline_model_config(cfg: &ResolvedTtsConfig) -> OfflineTtsModelConfig {
    let path = |p: Option<&PathBuf>| p.map(|p| p.to_string_lossy().to_string());
    let dict_dir = cfg
        .dict_dir
        .as_ref()
        .map(|d| d.to_string_lossy().to_string());
    let mut config = OfflineTtsModelConfig {
        num_threads: cfg.num_threads,
        debug: cfg.debug,
        provider: Some(cfg.provider.clone()),
        ..Default::default()
    };
    match cfg.model_type {
        TtsModelKind::Zipvoice => {
            config.zipvoice = OfflineTtsZipvoiceModelConfig {
                tokens: path(Some(&cfg.tokens)),
                encoder: path(Some(&cfg.encoder)),
                decoder: path(Some(&cfg.decoder)),
                vocoder: path(Some(&cfg.vocoder)),
                data_dir: path(Some(&cfg.data_dir)),
                lexicon: path(Some(&cfg.lexicon)),
                feat_scale: config::DEFAULT_FEAT_SCALE,
                t_shift: config::DEFAULT_T_SHIFT,
                target_rms: config::DEFAULT_TARGET_RMS,
                guidance_scale: config::DEFAULT_GUIDANCE_SCALE,
            };
        }
        TtsModelKind::Vits => {
            config.vits = OfflineTtsVitsModelConfig {
                model: path(cfg.model.as_ref()),
                lexicon: path(Some(&cfg.lexicon)),
                tokens: path(Some(&cfg.tokens)),
                data_dir: None,
                noise_scale: 0.667,
                noise_scale_w: 0.8,
                length_scale: 1.0,
                dict_dir,
            };
        }
        TtsModelKind::Matcha => {
            config.matcha = OfflineTtsMatchaModelConfig {
                acoustic_model: path(cfg.acoustic_model.as_ref()),
                vocoder: path(Some(&cfg.vocoder)),
                lexicon: path(Some(&cfg.lexicon)),
                tokens: path(Some(&cfg.tokens)),
                data_dir: None,
                noise_scale: 0.667,
                length_scale: 1.0,
                dict_dir,
            };
        }
        TtsModelKind::Kokoro => {
            config.kokoro = OfflineTtsKokoroModelConfig {
                model: path(cfg.model.as_ref()),
                voices: path(cfg.voices.as_ref()),
                tokens: path(Some(&cfg.tokens)),
                data_dir: path(Some(&cfg.data_dir)),
                length_scale: 1.0,
                dict_dir,
                // Kokoro 包是多 lexicon（us-en/gb-en/zh），resolve 阶段按存在探测并
                // 逗号 join 进单字段（sherpa-onnx 内部按逗号拆分）；无 lexicon.txt。
                lexicon: cfg.kokoro_lexicons.clone(),
                lang: None,
            };
        }
        TtsModelKind::Kitten => {
            config.kitten = OfflineTtsKittenModelConfig {
                model: path(cfg.model.as_ref()),
                voices: None,
                tokens: path(Some(&cfg.tokens)),
                data_dir: path(Some(&cfg.data_dir)),
                length_scale: 1.0,
            };
        }
        // registry 未收录：字段留空（sherpa 侧报缺文件），预检阶段已拦截
        TtsModelKind::Pocket => {
            config.pocket = OfflineTtsPocketModelConfig::default();
        }
        TtsModelKind::Supertonic => {
            config.supertonic = OfflineTtsSupertonicModelConfig::default();
        }
        // audiocpp-only 族：无 sherpa 配置分支（引擎构造在 AudiocppTts，
        // preflight 已按族清单拦截非法组合）
        TtsModelKind::Omnivoice
        | TtsModelKind::Voxcpm2
        | TtsModelKind::Qwen3Tts06
        | TtsModelKind::Qwen3Tts17 => {}
    }
    config
}

/// 按模型族探测根级 rule fsts（存在者逗号 join 全路径）。
///
/// - Vits：`date.fst` / `number.fst`（日期/数字规范化；缺省不影响合成）
/// - Kokoro：`date-zh.fst` / `number-zh.fst` / `phone-zh.fst`（官方建议启用的中文
///   数字/日期/电话规范化）
///
/// 传**全路径**（相对 CWD 的裸文件名在非模型目录启动时无法被 sherpa 定位）。
fn probe_rule_fsts(kind: TtsModelKind, model_dir: &Path) -> Option<String> {
    let names: &[&str] = match kind {
        TtsModelKind::Vits => &["date.fst", "number.fst"],
        TtsModelKind::Kokoro => &config::KOKORO_RULE_FSTS,
        _ => return None,
    };
    let joined = names
        .iter()
        .filter(|f| model_dir.join(f).is_file())
        .map(|f| model_dir.join(f).to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(",");
    (!joined.is_empty()).then_some(joined)
}

/// 文本转语音引擎（双后端门面）。
///
/// 内部按 `backend` 分派：sherpa（进程内 `OfflineTts`）或 audiocpp（sidecar HTTP）。
/// 对齐 `voice::AsrBackend` 的 enum 分派先例（后端仅两个且同 crate，可穷尽匹配，
/// 无需 trait 装箱）；两个后端均 `Send`，可按值 move 进合成线程（`SynthHandle`）。
/// 参考音色（零样本声音克隆的音色来源）在每次合成时按需传入，引擎可复用、可切换
/// 音色。所有方法接收 `&self`。
pub struct TtsEngine {
    inner: TtsBackendInner,
}

enum TtsBackendInner {
    Sherpa {
        tts: OfflineTts,
        cfg: ResolvedTtsConfig,
    },
    Audiocpp(AudiocppTts),
}

impl TtsEngine {
    /// 构造引擎：先按 backend 做就绪预检（文件清单见 [`config::preflight`]），
    /// sherpa 再创建 `OfflineTts`，audiocpp 定位引擎并 lease sidecar 进程。
    pub fn new(cfg: ResolvedTtsConfig) -> Result<Self, String> {
        match cfg.backend {
            TtsBackendKind::Sherpa => {
                config::preflight(&cfg)?;
                // Kokoro 主模型名随量化变体不同（model.onnx / model.int8.onnx），不在
                // required_files 清单里，单独按候选探测。
                if cfg.model_type == TtsModelKind::Kokoro
                    && config::kokoro_model_file_in(&cfg.model_dir).is_none()
                {
                    return Err(format!(
                        "缺少模型文件 {} 或 {}: {}\n请运行 `zapmomo tts install-model` 下载模型。",
                        config::DEFAULT_KOKORO_MODEL,
                        config::DEFAULT_KOKORO_INT8_MODEL,
                        cfg.model_dir.display()
                    ));
                }
                let model_config = build_offline_model_config(&cfg);
                let rule_fsts = probe_rule_fsts(cfg.model_type, &cfg.model_dir);
                let config = OfflineTtsConfig {
                    model: model_config,
                    rule_fsts,
                    ..Default::default()
                };

                let tts = OfflineTts::create(&config)
                    .ok_or_else(|| "无法创建 OfflineTts，请检查模型文件与配置。".to_string())?;

                // Kokoro v1.1 应有 103 个音色；不一致说明 voices.bin 与预期版本不符（仅告警，
                // sid 钳界由 kokoro_voices::normalize_sid 兜底）。
                if cfg.model_type == TtsModelKind::Kokoro && tts.num_speakers() != 103 {
                    tracing::warn!(
                        "Kokoro 音色数 {} 与预期 103 不符，请检查模型包版本",
                        tts.num_speakers()
                    );
                }

                Ok(Self {
                    inner: TtsBackendInner::Sherpa { tts, cfg },
                })
            }
            TtsBackendKind::Audiocpp => {
                let tts = AudiocppTts::new(cfg)?;
                Ok(Self {
                    inner: TtsBackendInner::Audiocpp(tts),
                })
            }
        }
    }

    pub fn config(&self) -> &ResolvedTtsConfig {
        match &self.inner {
            TtsBackendInner::Sherpa { cfg, .. } => cfg,
            TtsBackendInner::Audiocpp(a) => a.config(),
        }
    }

    /// 合成输出的采样率（Hz）。audiocpp 后端初值为 PocketTTS 固定 24000，
    /// 首次合成后按响应 wav 头校准。
    pub fn sample_rate(&self) -> i32 {
        match &self.inner {
            TtsBackendInner::Sherpa { tts, .. } => tts.sample_rate(),
            TtsBackendInner::Audiocpp(a) => a.sample_rate(),
        }
    }

    /// 测试构造：包装 audiocpp 客户端（直连 stub server，不 spawn 进程）。
    /// 供合成线程 SwapEngine 句间语义测试构造「无需模型文件」的真实引擎。
    #[cfg(test)]
    pub(crate) fn from_audiocpp_for_test(tts: crate::audiocpp::client::AudiocppTts) -> Self {
        Self {
            inner: TtsBackendInner::Audiocpp(tts),
        }
    }

    /// 把文本合成为 PCM 波形（f32，采样率见 [`Self::sample_rate`]）。
    ///
    /// 模型始终以 1.0 语速合成（避免 ZipVoice 高语速时 `kept_frames≤0` 触发
    /// sherpa C++ 异常导致 Rust abort），目标语速通过对输出重采样实现。
    pub fn synthesize(
        &self,
        text: &str,
        speed: f32,
        voice: &TtsVoiceParams,
    ) -> Result<Vec<f32>, String> {
        match &self.inner {
            TtsBackendInner::Sherpa { tts, cfg } => {
                let gen_config = build_generation_config(
                    cfg.model_type,
                    cfg.num_steps,
                    1.0,
                    voice,
                    tts.sample_rate(),
                )?;
                let audio = tts
                    .generate_with_config(text, &gen_config, None::<fn(&[f32], f32) -> bool>)
                    .ok_or_else(|| "语音合成失败。".to_string())?;
                apply_speed_to_samples(audio.samples(), tts.sample_rate(), speed)
            }
            TtsBackendInner::Audiocpp(a) => a.synthesize(text, speed, voice),
        }
    }

    /// 把文本合成为 PCM，并在合成过程中回调进度（0..1）。
    ///
    /// sherpa：`progress` 返回 `false` 提前终止合成（协作回调语义）。
    /// audiocpp：请求前探询（返回 `false` 则不发请求）——HTTP 在途请求无法中断，
    /// 这是两个后端的取消语义差异。
    /// 语速同 [`Self::synthesize`]：模型按 1.0 合成，输出重采样实现。
    pub fn synthesize_with_progress<F>(
        &self,
        text: &str,
        speed: f32,
        voice: &TtsVoiceParams,
        mut progress: F,
    ) -> Result<Vec<f32>, String>
    where
        F: FnMut(f32) -> bool + 'static,
    {
        match &self.inner {
            TtsBackendInner::Sherpa { tts, cfg } => {
                let gen_config = build_generation_config(
                    cfg.model_type,
                    cfg.num_steps,
                    1.0,
                    voice,
                    tts.sample_rate(),
                )?;
                let callback = move |_samples: &[f32], p: f32| progress(p);
                let audio = tts
                    .generate_with_config(text, &gen_config, Some(callback))
                    .ok_or_else(|| "语音合成失败。".to_string())?;
                apply_speed_to_samples(audio.samples(), tts.sample_rate(), speed)
            }
            TtsBackendInner::Audiocpp(a) => {
                if !progress(0.05) {
                    return Err("已取消".to_string());
                }
                let out = a.synthesize(text, speed, voice)?;
                let _ = progress(1.0);
                Ok(out)
            }
        }
    }

    /// 该引擎是否支持流式分块合成（audiocpp 流式族 true；sherpa 全族 false）。
    ///
    /// 环境变量 `ZAPMOMO_TTS_NO_STREAM` 非空时强制 false：A/B 首响测量对照与
    /// 线上流式异常时的一键回退开关（合成线程据此走整段路径）。
    pub fn supports_streaming(&self) -> bool {
        if std::env::var_os("ZAPMOMO_TTS_NO_STREAM").is_some_and(|v| !v.is_empty()) {
            return false;
        }
        match &self.inner {
            TtsBackendInner::Sherpa { .. } => false,
            TtsBackendInner::Audiocpp(a) => a.supports_streaming(),
        }
    }

    /// SSE 流式合成：逐块回调 `on_chunk(samples, sample_rate)`，回调返回 `false`
    /// 协作取消（停止读取并断开连接，见 `audiocpp::client`）。全部块回调完返回
    /// `Ok(())`（正常完成与取消不区分，取消语义由调用方掌握）。
    ///
    /// 样本已应用语速：与 [`Self::synthesize`] 同语义（样本在 `rate/speed` 域、
    /// 按 `rate` 播放）。语速经**跨 chunk 持久** `Resampler` 实现——
    /// `LinearResampler` 带内部相位状态，逐块独立重采样会丢余量样本（时长
    /// 漂移 + 块边界爆音），增量喂入 + 末尾 flush 与 `audio::record_voice` 同模式。
    /// sherpa 后端不支持流式（`OfflineTts` 整段合成），调用前应先查
    /// [`Self::supports_streaming`]。
    pub fn synthesize_streaming(
        &self,
        text: &str,
        speed: f32,
        voice: &TtsVoiceParams,
        on_chunk: &mut dyn FnMut(&[f32], i32) -> bool,
    ) -> Result<(), String> {
        match &self.inner {
            TtsBackendInner::Sherpa { .. } => Err("sherpa 后端不支持流式合成".to_string()),
            TtsBackendInner::Audiocpp(a) => {
                // 预建持久重采样器：采样率用引擎缓存值（构造初值 = 族固定值，
                // 整段路径首响应 wav 头校准后随之更新；流式事件无采样率字段）
                let rate = a.sample_rate();
                let mut resampler = if (speed - 1.0).abs() < 1e-6 {
                    None // 常用路径（语速 1.0）零拷贝直通
                } else {
                    let out_rate = (rate as f32 / speed) as i32;
                    Some(crate::audio::Resampler::new(rate, out_rate)?)
                };
                a.synthesize_streaming(text, voice, &mut |samples, rate| match &mut resampler {
                    Some(r) => {
                        let out = r.process(samples, false);
                        on_chunk(&out, rate)
                    }
                    None => on_chunk(samples, rate),
                })?;
                // 终态冲刷重采样器尾部缓冲（非空则补投一块）
                if let Some(r) = &mut resampler {
                    let tail = r.process(&[], true);
                    if !tail.is_empty() {
                        let _ = on_chunk(&tail, rate);
                    }
                }
                Ok(())
            }
        }
    }

    /// 把文本合成为 wav 文件。
    pub fn synthesize_to_wav(
        &self,
        text: &str,
        speed: f32,
        voice: &TtsVoiceParams,
        out_path: &Path,
    ) -> Result<(), String> {
        self.synthesize_to_wav_with_progress(text, speed, voice, out_path, |_p| true)
            .map(|_| ())
    }

    /// 把文本合成为 wav 文件，并在合成过程中回调进度（0..1）。
    ///
    /// 返回采样点数（已应用语速），便于调用方换算音频时长（`samples / sample_rate`）。
    pub fn synthesize_to_wav_with_progress<F>(
        &self,
        text: &str,
        speed: f32,
        voice: &TtsVoiceParams,
        out_path: &Path,
        mut progress: F,
    ) -> Result<usize, String>
    where
        F: FnMut(f32) -> bool + 'static,
    {
        match &self.inner {
            TtsBackendInner::Sherpa { tts, cfg } => {
                let gen_config = build_generation_config(
                    cfg.model_type,
                    cfg.num_steps,
                    1.0,
                    voice,
                    tts.sample_rate(),
                )?;
                let callback = move |_samples: &[f32], p: f32| progress(p);
                let audio = tts
                    .generate_with_config(text, &gen_config, Some(callback))
                    .ok_or_else(|| "语音合成失败。".to_string())?;
                let sample_rate = tts.sample_rate();
                let samples = apply_speed_to_samples(audio.samples(), sample_rate, speed)?;
                crate::audio::write_wav_f32(out_path, sample_rate as u32, &samples)?;
                Ok(samples.len())
            }
            TtsBackendInner::Audiocpp(a) => {
                if !progress(0.05) {
                    return Err("已取消".to_string());
                }
                let samples = a.synthesize(text, speed, voice)?;
                let sample_rate = a.sample_rate();
                crate::audio::write_wav_f32(out_path, sample_rate as u32, &samples)?;
                let _ = progress(1.0);
                Ok(samples.len())
            }
        }
    }
}

/// 构建生成配置的纯函数（可单测，不依赖真实 `OfflineTts`）。
///
/// `Sid` 走 speaker id；`Reference` 走参考音频克隆（仅 ZipVoice 支持，其余报错）。
fn build_generation_config(
    model_type: TtsModelKind,
    num_steps: i32,
    speed: f32,
    voice: &TtsVoiceParams,
    model_sample_rate: i32,
) -> Result<GenerationConfig, String> {
    let mut gc = GenerationConfig {
        speed,
        num_steps,
        ..Default::default()
    };
    match voice {
        TtsVoiceParams::Sid(sid) => {
            gc.sid = *sid;
        }
        TtsVoiceParams::Named(_) => {
            return Err("sherpa 后端不支持具名音色（该参数仅 audio.cpp 后端使用）".to_string());
        }
        TtsVoiceParams::Reference {
            wav_path,
            reference_text,
        } => {
            if !model_type.uses_reference_audio() {
                return Err("该模型不支持参考音频（声音克隆）语义".to_string());
            }
            let wave = Wave::read(&wav_path.to_string_lossy())
                .ok_or_else(|| format!("无法读取参考音频: {}", wav_path.display()))?;
            // 把参考音频归一化到模型目标采样率：ZipVoice 的 Mel 频谱在跨采样率
            // （如 48k→24k）时，sherpa C++ 重采样器可能抛异常，Rust 无法捕获
            // C++ 异常会直接 abort。统一到目标采样率后 Mel 重采样变为恒等变换。
            let (reference_audio, reference_sample_rate) =
                normalize_reference(wave.samples(), wave.sample_rate(), model_sample_rate)?;
            gc.reference_audio = Some(reference_audio);
            gc.reference_sample_rate = reference_sample_rate;
            gc.reference_text = Some(reference_text.clone());
        }
    }
    Ok(gc)
}

/// 把参考音频归一化到目标采样率。
///
/// ZipVoice 的 Mel 频谱在跨采样率（如 48k→24k）时，sherpa C++ 重采样器可能抛
/// 异常，而 Rust 无法捕获 C++ 异常会直接 abort。统一到模型目标采样率后，
/// Mel 频谱重采样变为恒等变换，避免崩溃。同采样率时原样返回（零开销）。
fn normalize_reference(
    samples: &[f32],
    src_rate: i32,
    target_rate: i32,
) -> Result<(Vec<f32>, i32), String> {
    if src_rate == target_rate {
        return Ok((samples.to_vec(), src_rate));
    }
    let mut resampler = crate::audio::Resampler::new(src_rate, target_rate)?;
    let out = resampler.process(samples, true);
    Ok((out, target_rate))
}

/// 对合成输出应用语速：模型以 1.0 合成后，把样本重采样到 `sample_rate / speed`，
/// 再以 `sample_rate` 写回，从而改变时长（speed>1 更快、样本更少；speed<1 更慢、样本更多）。
///
/// 这是为了避免把 speed 传给 ZipVoice 模型：模型内部 `kept_frames =
/// num_frames(speed) - 参考帧数`，高语速 + 短文本时 `kept_frames≤0` 会抛 C++
/// 异常，而 Rust 无法捕获 C++ 异常会直接 abort。改用输出重采样后任何语速都不崩。
///
/// audiocpp 后端复用同一语义（`crate::audiocpp::client` 调用），保证两个后端
/// 的语速行为一致。
pub(crate) fn apply_speed_to_samples(
    samples: &[f32],
    sample_rate: i32,
    speed: f32,
) -> Result<Vec<f32>, String> {
    if speed <= 0.0 {
        return Err(format!("语速必须为正数，当前 {speed}"));
    }
    if (speed - 1.0).abs() < 1e-6 {
        return Ok(samples.to_vec());
    }
    let out_rate = (sample_rate as f32 / speed) as i32;
    let mut resampler = crate::audio::Resampler::new(sample_rate, out_rate)?;
    Ok(resampler.process(samples, true))
}

/// TTS 模型安装目录：`~/.zapmomo/models/<name>`。
pub fn user_model_dir() -> PathBuf {
    crate::kws::model::tts_user_model_dir()
}

/// 生成唯一的 TTS 输出 wav 路径：`~/.zapmomo/tts/tts-<毫秒时间戳>.wav`
pub fn default_output_path() -> PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    crate::config::settings::get_tts_output_dir().join(format!("tts-{millis}.wav"))
}

/// 目标目录是否已装好 TTS 主模型。
pub fn is_installed(dir: &Path) -> bool {
    crate::kws::model::has_required_files(dir, &config::REQUIRED_FILES)
}

/// 安装 TTS 主模型到 `dest_dir`（默认 `~/.zapmomo/models/<name>`）。
///
/// 幂等：已安装且 `force` 为假时直接返回。下载过程中回调进度。
pub fn install_model_to(
    dest_dir: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
) -> Result<(), ModelError> {
    crate::kws::model::install_asset_to(
        crate::kws::model::tts_asset(),
        dest_dir,
        force,
        on_progress,
        &config::REQUIRED_FILES,
    )
}

/// 安装 TTS 声码器到 `dest_dir`（独立发布的 vocos_24khz.onnx 单文件）。
///
/// 幂等：已安装且 `force` 为假时直接返回。
pub fn install_vocoder_to(
    dest_dir: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
) -> Result<(), ModelError> {
    crate::kws::model::install_raw_file_to(
        crate::kws::model::tts_vocoder_asset(),
        &dest_dir.join(config::DEFAULT_VOCODER),
        force,
        on_progress,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_new_missing_model_errors() {
        // 用一个不存在的模型目录，TtsEngine::new 应报错提示下载模型
        let mut cfg = ResolvedTtsConfig::default();
        cfg.model_dir = PathBuf::from("/nonexistent/model");
        cfg.encoder = cfg.model_dir.join("encoder.int8.onnx");
        let err = TtsEngine::new(cfg.clone()).err().unwrap();
        assert!(err.contains("install-model"), "err: {err}");
    }

    #[test]
    #[ignore = "需要先运行 cargo run -- tts install-model 下载模型"]
    fn test_synthesize_produces_audio() {
        let cfg = config::resolve(None, None).unwrap();
        if !cfg.encoder.is_file() {
            eprintln!("跳过：模型未下载，请运行 cargo run -- tts install-model");
            return;
        }
        let engine = TtsEngine::new(cfg.clone()).unwrap();
        let voice = TtsVoiceParams::Reference {
            wav_path: cfg.reference_wav.clone(),
            reference_text: cfg.reference_text.clone(),
        };
        let samples = engine
            .synthesize("你好，我是 ZapMomo。", 1.0, &voice)
            .unwrap();
        assert!(!samples.is_empty(), "合成音频不应为空");
    }

    #[test]
    #[ignore = "需要 kokoro-int8-multi-lang-v1_1 解压在 KOKORO_E2E_DIR 指定目录"]
    fn test_kokoro_synthesize_produces_audio() {
        // E2E：KOKORO_E2E_DIR=/path/to/kokoro-int8-multi-lang-v1_1 cargo test -- --ignored
        let Some(dir) = std::env::var("KOKORO_E2E_DIR").ok() else {
            eprintln!("跳过：未设置 KOKORO_E2E_DIR");
            return;
        };
        // settings 为 None → 走 detect_kind_from_dir 探测（voices.bin → Kokoro）
        let cfg = config::resolve(None, Some(Path::new(&dir))).unwrap();
        assert_eq!(cfg.model_type, TtsModelKind::Kokoro, "目录探测应为 Kokoro");
        let engine = TtsEngine::new(cfg.clone()).unwrap();
        assert_eq!(engine.sample_rate(), 24000, "Kokoro 固定 24kHz");
        // 中文女声 zf_001（sid 3）经统一解析入口
        let voice = crate::tts::voice::resolve_sid_voice(&cfg, Some("zf_001"), None).unwrap();
        let started = std::time::Instant::now();
        let samples = engine
            .synthesize("你好，我是 ZapMomo 语音伙伴。", 1.0, &voice)
            .unwrap();
        let elapsed = started.elapsed().as_secs_f32();
        assert!(!samples.is_empty(), "合成音频不应为空");
        let duration = samples.len() as f32 / engine.sample_rate() as f32;
        eprintln!(
            "kokoro e2e: {:.2}s 音频 / {:.2}s 合成 (RTF {:.2})",
            duration,
            elapsed,
            elapsed / duration
        );
    }

    #[test]
    #[ignore = "需要 omnivoice GGUF 在 OMNIVOICE_E2E_DIR 目录 + audiocpp 引擎可定位"]
    fn test_omnivoice_synthesize_produces_audio() {
        // E2E：OMNIVOICE_E2E_DIR=/path/to/omnivoice-audiocpp cargo test -- --ignored
        // 可选 OMNIVOICE_E2E_REF=/path/to/ref.wav 验证克隆（缺省走 auto voice）。
        let Some(dir) = std::env::var("OMNIVOICE_E2E_DIR").ok() else {
            eprintln!("跳过：未设置 OMNIVOICE_E2E_DIR");
            return;
        };
        let cfg = config::ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: TtsModelKind::Omnivoice,
            model_dir: PathBuf::from(&dir),
            // 阶段 1 实测：omnivoice CPU RTF 6.6 不可用，Metal 0.41 达标
            provider: std::env::var("OMNIVOICE_E2E_PROVIDER")
                .unwrap_or_else(|_| "metal".to_string()),
            ..config::ResolvedTtsConfig::default()
        };
        let engine = TtsEngine::new(cfg.clone()).unwrap();
        assert_eq!(engine.sample_rate(), 24000, "omnivoice 固定 24kHz");

        let voice = match std::env::var("OMNIVOICE_E2E_REF") {
            Ok(ref_wav) => TtsVoiceParams::Reference {
                wav_path: PathBuf::from(ref_wav),
                reference_text: std::env::var("OMNIVOICE_E2E_REF_TEXT").unwrap_or_else(|_| {
                    "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.".to_string()
                }),
            },
            Err(_) => TtsVoiceParams::Sid(0), // auto voice
        };
        let started = std::time::Instant::now();
        let samples = engine
            .synthesize(
                "你好，我是 ZapMomo 语音伙伴，正在验证 OmniVoice 中文合成。",
                1.0,
                &voice,
            )
            .unwrap();
        let elapsed = started.elapsed().as_secs_f32();
        assert!(!samples.is_empty(), "合成音频不应为空");
        let duration = samples.len() as f32 / engine.sample_rate() as f32;
        eprintln!(
            "omnivoice e2e ({}): {:.2}s 音频 / {:.2}s 合成 (RTF {:.2})",
            if matches!(voice, TtsVoiceParams::Reference { .. }) {
                "clone"
            } else {
                "auto"
            },
            duration,
            elapsed,
            elapsed / duration
        );
    }

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

    #[test]
    fn test_normalize_reference_identity_rate() {
        let samples = vec![0.1f32; 24000];
        let (out, rate) = normalize_reference(&samples, 24000, 24000).unwrap();
        assert_eq!(rate, 24000);
        assert_eq!(out.len(), 24000);
    }

    #[test]
    fn test_normalize_reference_resamples_48k_to_24k() {
        // 用户上传的 48k 参考音频 → 归一化到 24k（之前会导致 sherpa Mel 重采样崩溃）
        let samples = vec![0.1f32; 48000]; // 1 秒 @48k
        let (out, rate) = normalize_reference(&samples, 48000, 24000).unwrap();
        assert_eq!(rate, 24000);
        assert!(
            (out.len() as i64 - 24000).abs() <= 64,
            "resample len={}",
            out.len()
        );
    }

    #[test]
    fn test_normalize_reference_upsamples_16k_to_24k() {
        // 录音（16k）→ 归一化到 24k（上采样）
        let samples = vec![0.1f32; 16000]; // 1 秒 @16k
        let (out, rate) = normalize_reference(&samples, 16000, 24000).unwrap();
        assert_eq!(rate, 24000);
        assert!(
            (out.len() as i64 - 24000).abs() <= 64,
            "upsample len={}",
            out.len()
        );
    }

    #[test]
    fn test_apply_speed_identity() {
        let samples = vec![0.1f32; 24000];
        let out = apply_speed_to_samples(&samples, 24000, 1.0).unwrap();
        assert_eq!(out.len(), 24000);
    }

    #[test]
    fn test_apply_speed_faster_shortens() {
        // speed 1.3 → 样本数 ≈ 1/1.3（24k / 1.3 ≈ 18461 目标采样率）
        let samples = vec![0.1f32; 24000];
        let out = apply_speed_to_samples(&samples, 24000, 1.3).unwrap();
        assert!(
            (out.len() as i64 - 18461).abs() <= 64,
            "speed 1.3 len={}",
            out.len()
        );
    }

    #[test]
    fn test_apply_speed_slower_lengthens() {
        // speed 0.7 → 样本数 ≈ 1/0.7（24k / 0.7 ≈ 34285 目标采样率）
        let samples = vec![0.1f32; 24000];
        let out = apply_speed_to_samples(&samples, 24000, 0.7).unwrap();
        assert!(
            (out.len() as i64 - 34285).abs() <= 64,
            "speed 0.7 len={}",
            out.len()
        );
    }

    #[test]
    fn test_apply_speed_rejects_non_positive() {
        assert!(apply_speed_to_samples(&[0.0f32], 24000, 0.0).is_err());
        assert!(apply_speed_to_samples(&[0.0f32], 24000, -1.0).is_err());
    }

    #[test]
    fn test_build_offline_model_config_vits_branch() {
        let mut cfg = config::ResolvedTtsConfig::default();
        cfg.model_type = TtsModelKind::Vits;
        cfg.model = Some(PathBuf::from("/m/vits/model.onnx"));
        cfg.dict_dir = Some(PathBuf::from("/m/vits/dict"));
        let c = build_offline_model_config(&cfg);
        let v = c.vits;
        assert_eq!(v.model.as_deref(), Some("/m/vits/model.onnx"));
        let expected_tokens = cfg.tokens.to_string_lossy().to_string();
        assert_eq!(v.tokens.as_deref(), Some(expected_tokens.as_str()));
        assert_eq!(v.dict_dir.as_deref(), Some("/m/vits/dict"));
        // vits 分支不应污染 zipvoice 字段
        assert!(c.zipvoice.tokens.is_none());
    }

    #[test]
    fn test_build_offline_model_config_matcha_branch() {
        let mut cfg = config::ResolvedTtsConfig::default();
        cfg.model_type = TtsModelKind::Matcha;
        cfg.acoustic_model = Some(PathBuf::from("/m/matcha/model-steps-3.onnx"));
        cfg.vocoder = PathBuf::from("/m/matcha/vocos-22khz-univ.onnx");
        let c = build_offline_model_config(&cfg);
        assert_eq!(
            c.matcha.acoustic_model.as_deref(),
            Some("/m/matcha/model-steps-3.onnx")
        );
        assert_eq!(
            c.matcha.vocoder.as_deref(),
            Some("/m/matcha/vocos-22khz-univ.onnx")
        );
        assert!(c.vits.model.is_none());
    }

    #[test]
    fn test_build_offline_model_config_zipvoice_keeps_defaults() {
        let cfg = config::ResolvedTtsConfig::default();
        assert_eq!(cfg.model_type, TtsModelKind::Zipvoice);
        let c = build_offline_model_config(&cfg);
        assert!(c.zipvoice.encoder.is_some());
        assert!(c.zipvoice.decoder.is_some());
        assert!(c.vits.model.is_none());
        assert!(c.matcha.acoustic_model.is_none());
    }

    #[test]
    fn test_build_offline_model_config_kokoro_branch() {
        let mut cfg = config::ResolvedTtsConfig::default();
        cfg.model_type = TtsModelKind::Kokoro;
        cfg.model = Some(PathBuf::from("/m/kokoro/model.int8.onnx"));
        cfg.voices = Some(PathBuf::from("/m/kokoro/voices.bin"));
        cfg.dict_dir = Some(PathBuf::from("/m/kokoro/dict"));
        cfg.kokoro_lexicons =
            Some("/m/kokoro/lexicon-us-en.txt,/m/kokoro/lexicon-zh.txt".to_string());
        let c = build_offline_model_config(&cfg);
        assert_eq!(c.kokoro.model.as_deref(), Some("/m/kokoro/model.int8.onnx"));
        assert_eq!(c.kokoro.voices.as_deref(), Some("/m/kokoro/voices.bin"));
        assert_eq!(
            c.kokoro.lexicon.as_deref(),
            Some("/m/kokoro/lexicon-us-en.txt,/m/kokoro/lexicon-zh.txt")
        );
        assert_eq!(c.kokoro.dict_dir.as_deref(), Some("/m/kokoro/dict"));
        assert_eq!(c.kokoro.length_scale, 1.0);
        // 不污染其他分支
        assert!(c.vits.model.is_none());
        assert!(c.zipvoice.encoder.is_none());
    }

    #[test]
    fn test_probe_rule_fsts_by_kind() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path();
        // 空目录：各族都无 fst
        assert!(probe_rule_fsts(TtsModelKind::Vits, dir).is_none());
        assert!(probe_rule_fsts(TtsModelKind::Kokoro, dir).is_none());
        // 未收录族恒 None（即使文件存在）
        std::fs::write(dir.join("date.fst"), b"x").unwrap();
        assert!(probe_rule_fsts(TtsModelKind::Zipvoice, dir).is_none());
        // Vits：date.fst/number.fst 存在者全路径逗号 join
        std::fs::write(dir.join("number.fst"), b"x").unwrap();
        let vits = probe_rule_fsts(TtsModelKind::Vits, dir).unwrap();
        let date_fst = dir.join("date.fst").to_string_lossy().to_string();
        let number_fst = dir.join("number.fst").to_string_lossy().to_string();
        assert!(vits.contains(&date_fst), "{vits}");
        assert!(vits.contains(&number_fst), "{vits}");
        // Kokoro：三个 zh fst 按存在过滤（date-zh 存在、其余缺）
        std::fs::write(dir.join("date-zh.fst"), b"x").unwrap();
        std::fs::write(dir.join("phone-zh.fst"), b"x").unwrap();
        let kokoro = probe_rule_fsts(TtsModelKind::Kokoro, dir).unwrap();
        assert!(
            kokoro.contains("date-zh.fst") && kokoro.contains("phone-zh.fst"),
            "{kokoro}"
        );
        assert!(!kokoro.contains("number-zh.fst"), "{kokoro}");
        // Kokoro 不吃 Vits 的裸 fst
        assert!(!kokoro.contains("number.fst"), "{kokoro}");
    }

    #[test]
    fn test_build_generation_config_sid() {
        let gc =
            build_generation_config(TtsModelKind::Vits, 4, 1.2, &TtsVoiceParams::Sid(0), 22050)
                .unwrap();
        assert_eq!(gc.sid, 0);
        assert_eq!(gc.speed, 1.2);
        assert_eq!(gc.num_steps, 4);
        assert!(gc.reference_audio.is_none(), "sid 不应携带参考音频");
    }

    #[test]
    fn test_build_generation_config_reference_requires_zipvoice() {
        // 非 zipvoice 模型传 Reference → 报错
        let err = build_generation_config(
            TtsModelKind::Vits,
            4,
            1.0,
            &TtsVoiceParams::Reference {
                wav_path: PathBuf::from("/nonexistent.wav"),
                reference_text: "x".to_string(),
            },
            24000,
        )
        .unwrap_err();
        assert!(err.contains("参考音频"), "err: {err}");
    }

    #[test]
    fn test_build_generation_config_reference_missing_wav_errors() {
        // zipvoice + Reference + 参考音频不存在 → 读取失败
        let err = build_generation_config(
            TtsModelKind::Zipvoice,
            4,
            1.0,
            &TtsVoiceParams::Reference {
                wav_path: PathBuf::from("/nonexistent.wav"),
                reference_text: "x".to_string(),
            },
            24000,
        )
        .unwrap_err();
        assert!(err.contains("无法读取参考音频"), "err: {err}");
    }

    // ---------- 流式能力门面（tiny_http SSE stub，直连构造无需模型文件） ----------

    /// 起 SSE stub：两块各 `samples_per_chunk` 个 i16 样本（24kHz）+ done + [DONE]。
    fn spawn_streaming_stub(samples_per_chunk: usize) -> String {
        use base64::Engine as _;
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let mut events = String::new();
                for _ in 0..2 {
                    let pcm: Vec<u8> = vec![0x10, 0x27]
                        .iter()
                        .cycle()
                        .take(samples_per_chunk * 2)
                        .copied()
                        .collect();
                    let b64 = base64::engine::general_purpose::STANDARD.encode(pcm);
                    events.push_str(&format!(
                        "data: {}\n\n",
                        serde_json::json!({"type": "speech.audio.delta", "audio": b64})
                    ));
                }
                events.push_str("data: {\"type\":\"speech.audio.done\"}\n\ndata: [DONE]\n\n");
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/event-stream"[..])
                        .unwrap();
                let _ =
                    request.respond(tiny_http::Response::from_string(events).with_header(header));
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn streaming_engine(model_type: TtsModelKind, base_url: &str) -> TtsEngine {
        TtsEngine::from_audiocpp_for_test(crate::audiocpp::client::AudiocppTts::new_with_base_url(
            crate::tts::config::ResolvedTtsConfig {
                backend: crate::tts::config::TtsBackendKind::Audiocpp,
                model_type,
                ..crate::tts::config::ResolvedTtsConfig::default()
            },
            base_url,
        ))
    }

    /// 能力分派：omnivoice true / pocket false（ZAPMOMO_TTS_NO_STREAM 未设时）。
    #[test]
    fn test_supports_streaming_dispatch() {
        let url = spawn_streaming_stub(8);
        assert!(streaming_engine(TtsModelKind::Omnivoice, &url).supports_streaming());
        assert!(!streaming_engine(TtsModelKind::Pocket, &url).supports_streaming());
    }

    /// 语速跨 chunk 持久重采样：两块各 2400 样本（24k）speed 2.0 → 累计 ≈2400。
    /// 逐块独立重采样会因相位余量丢失而漂移，本测试锚定持久实例语义。
    #[test]
    fn test_synthesize_streaming_applies_speed_across_chunks() {
        let url = spawn_streaming_stub(2400);
        let engine = streaming_engine(TtsModelKind::Omnivoice, &url);
        let mut total = 0usize;
        engine
            .synthesize_streaming("x", 2.0, &TtsVoiceParams::Sid(0), &mut |samples, rate| {
                assert_eq!(rate, 24000, "样本按模型采样率播放（speed 域在样本里）");
                total += samples.len();
                true
            })
            .unwrap();
        // 4800 输入 @speed 2.0 → ≈2400 输出（线性重采样容差）
        assert!(
            (total as i64 - 2400).abs() <= 32,
            "speed 2.0 累计输出 {total}"
        );
    }
}
