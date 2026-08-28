/// 离线语音识别（SenseVoice / Whisper / Qwen3-ASR）。
///
/// 与流式 `AsrEngine`（`OnlineRecognizer`）相对，使用 sherpa-onnx 的 `OfflineRecognizer`
/// 一次性整段转写 wav。用于文件转写入口（CLI `asr test` / Tauri `transcribe_audio` /
/// `transcribe_reference_audio`）与 VAD 分段听写/语音会话离线态。
use crate::asr::config::{AsrModelKind, ResolvedAsrConfig};
use sherpa_onnx::{
    OfflineModelConfig, OfflineQwen3ASRModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSenseVoiceModelConfig, OfflineWhisperModelConfig, Wave,
};
use std::path::Path;

/// 离线 ASR 引擎。
pub struct OfflineAsrEngine {
    recognizer: OfflineRecognizer,
    cfg: ResolvedAsrConfig,
}

impl OfflineAsrEngine {
    /// 构造引擎，先按模型族校验模型文件存在。
    pub fn new(cfg: ResolvedAsrConfig) -> Result<Self, String> {
        Self::check_required_files(&cfg)?;

        let mut c = OfflineRecognizerConfig::default();
        c.feat_config.sample_rate = cfg.sample_rate;
        c.model_config = build_offline_model_config(&cfg);
        let recognizer = OfflineRecognizer::create(&c)
            .ok_or_else(|| "无法创建 OfflineRecognizer，请检查模型文件与配置。".to_string())?;

        // 离线模型不叠加外部 CT 标点：SenseVoice（ITN）与 Whisper（原生）均自带标点，
        // 再套 CT Transformer 会产生 `,，`/`.。` 式双标点（实测）。
        Ok(Self { recognizer, cfg })
    }

    /// 按模型族校验必需文件（安装/导入完整性之外的运行时兜底）。
    fn check_required_files(cfg: &ResolvedAsrConfig) -> Result<(), String> {
        let files: Vec<(&str, &Path)> = match cfg.model_type {
            AsrModelKind::SenseVoice => {
                let model = cfg
                    .model
                    .as_deref()
                    .ok_or_else(|| "SenseVoice 模型未解析出主模型文件".to_string())?;
                vec![("model", model), ("tokens", &cfg.tokens)]
            }
            AsrModelKind::Whisper => vec![
                ("encoder", &cfg.encoder),
                ("decoder", &cfg.decoder),
                ("tokens", &cfg.tokens),
            ],
            AsrModelKind::Qwen3Asr => {
                let conv = cfg
                    .model
                    .as_deref()
                    .ok_or_else(|| "Qwen3-ASR 模型未解析出 conv_frontend 文件".to_string())?;
                // tokenizer 是目录（非文件），不进下面的 is_file 循环，单独校验
                if !cfg.tokens.is_dir() {
                    return Err(format!("缺少 tokenizer 目录: {}", cfg.tokens.display()));
                }
                vec![
                    ("conv_frontend", conv),
                    ("encoder", &cfg.encoder),
                    ("decoder", &cfg.decoder),
                ]
            }
            AsrModelKind::Zipformer | AsrModelKind::Paraformer => {
                return Err(format!(
                    "当前模型类型 {} 应走流式引擎，离线引擎不适用",
                    cfg.model_type.as_str()
                ));
            }
        };
        for (name, path) in files {
            if !path.is_file() {
                return Err(format!("缺少模型文件 {name}: {}", path.display()));
            }
        }
        Ok(())
    }

    /// 一次性整段转写 wav 文件，返回清洗后文本（SenseVoice 已剥情绪/语言标签）。
    pub fn transcribe_wav(&self, wav: &Path) -> Result<String, String> {
        let wave = Wave::read(&wav.to_string_lossy())
            .ok_or_else(|| format!("无法读取 wav: {}", wav.display()))?;
        self.transcribe_samples(wave.samples(), wave.sample_rate())
    }

    /// 从内存样本整段转写（听写 VAD 分段喂入；`sample_rate` 一般为 16k）。
    ///
    /// 底层 `OfflineStream::accept_waveform` 纯内存，无需写临时 wav。
    pub fn transcribe_samples(&self, samples: &[f32], sample_rate: i32) -> Result<String, String> {
        let stream = self.recognizer.create_stream();
        // 若样本采样率 != 模型采样率，先重采样（whisper/sensevoice 输入 16k，一般直走 else）
        if sample_rate != self.cfg.sample_rate {
            let mut rs = crate::audio::Resampler::new(sample_rate, self.cfg.sample_rate)?;
            let out = rs.process(samples, true);
            stream.accept_waveform(self.cfg.sample_rate, &out);
        } else {
            stream.accept_waveform(sample_rate, samples);
        }
        self.recognizer.decode(&stream);
        let raw = stream.get_result().map(|r| r.text).unwrap_or_default();
        let text = match self.cfg.model_type {
            AsrModelKind::SenseVoice => clean_sensevoice_text(&raw),
            _ => raw,
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("未能识别出有效文本，请换一段更清晰的音频".to_string());
        }
        Ok(text)
    }
}

/// 按模型类型填 sherpa `OfflineModelConfig` 对应分支（纯函数，可单测）。
pub(crate) fn build_offline_model_config(cfg: &ResolvedAsrConfig) -> OfflineModelConfig {
    let mut config = OfflineModelConfig {
        num_threads: cfg.num_threads,
        debug: cfg.debug,
        provider: Some(cfg.provider.clone()),
        ..Default::default()
    };
    config.tokens = Some(cfg.tokens.to_string_lossy().to_string());
    match cfg.model_type {
        AsrModelKind::SenseVoice => {
            config.model_type = Some("sensevoice".to_string());
            config.sense_voice = OfflineSenseVoiceModelConfig {
                model: cfg.model.as_ref().map(|p| p.to_string_lossy().to_string()),
                language: cfg.language.clone(),
                use_itn: cfg.use_itn.unwrap_or(true),
            };
        }
        AsrModelKind::Whisper => {
            config.model_type = Some("whisper".to_string());
            config.whisper = OfflineWhisperModelConfig {
                encoder: Some(cfg.encoder.to_string_lossy().to_string()),
                decoder: Some(cfg.decoder.to_string_lossy().to_string()),
                language: cfg.language.clone(),
                task: Some("transcribe".to_string()),
                tail_paddings: 0,
                enable_token_timestamps: false,
                enable_segment_timestamps: false,
            };
        }
        AsrModelKind::Qwen3Asr => {
            // qwen3 无 tokens.txt（tokenizer 从目录加载）：tokens 置空串绕过 C++
            // Validate 的通用 tokens 检查；model_type 不设（Validate 以 conv_frontend
            // 非空为准，官方 Rust 示例同款——与 sensevoice/whisper 显式设
            // model_type 的做法相反）。生成参数透传 crate 默认
            // （max_total_len=512 / max_new_tokens=128 / temperature=1e-6 / top_p=0.8 / seed=42）
            config.tokens = Some(String::new());
            config.qwen3_asr = OfflineQwen3ASRModelConfig {
                conv_frontend: cfg.model.as_ref().map(|p| p.to_string_lossy().to_string()),
                encoder: Some(cfg.encoder.to_string_lossy().to_string()),
                decoder: Some(cfg.decoder.to_string_lossy().to_string()),
                tokenizer: Some(cfg.tokens.to_string_lossy().to_string()),
                hotwords: cfg
                    .hotwords
                    .as_deref()
                    .filter(|h| !h.trim().is_empty())
                    .map(qwen3_hotwords),
                ..Default::default()
            };
        }
        AsrModelKind::Zipformer | AsrModelKind::Paraformer => {
            unreachable!("离线引擎不处理流式族")
        }
    }
    config
}

/// 项目热词语义「空格分隔」→ Qwen3-ASR 期望「逗号分隔」的格式转换。
///
/// sherpa C++ 端按逗号切分去空格后拼入 chat 模板 system 段（prompt 上下文偏置，
/// 不支持热词文件路径）；配置存储格式保持空格分隔不变，仅构造层转换。
fn qwen3_hotwords(h: &str) -> String {
    h.split_whitespace().collect::<Vec<_>>().join(",")
}

/// 剥离 SenseVoice 输出中的 `<|...|>` 语言/情绪/事件标签，并折叠空白。
///
/// 无 regex 依赖（项目未引入 regex crate），逐字符跳过标签段。
/// 语言/情绪/事件标签如 `<|zh|>` `<|NEUTRAL|>` `<|Speech|>` `<|HAPPY|>` `<|nospeech|>`
/// 一次剥离；`language`/`use_itn` 配置可消一部分，但情绪标签需此后处理兜底。
pub fn clean_sensevoice_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 便捷入口：新建引擎 + 整段转写（供 CLI / Tauri / `transcribe_wav` 分发复用）。
pub fn transcribe_wav_offline(cfg: &ResolvedAsrConfig, wav: &Path) -> Result<String, String> {
    let engine = OfflineAsrEngine::new(cfg.clone())?;
    engine.transcribe_wav(wav)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::config::{AsrModelKind, ResolvedAsrConfig};
    use std::path::PathBuf;

    fn cfg_with(kind: AsrModelKind) -> ResolvedAsrConfig {
        ResolvedAsrConfig {
            model_type: kind,
            ..ResolvedAsrConfig::default()
        }
    }

    #[test]
    fn test_clean_sensevoice_text_strips_tags() {
        assert_eq!(
            clean_sensevoice_text("<|zh|><|NEUTRAL|><|Speech|><|HAPPY|>你好，世界<|nospeech|>"),
            "你好，世界"
        );
        assert_eq!(clean_sensevoice_text("  hello   world  "), "hello world");
        assert_eq!(clean_sensevoice_text(""), "");
        // 无标签文本原样（仅折叠空白）
        assert_eq!(clean_sensevoice_text("今天天气不错。"), "今天天气不错。");
    }

    #[test]
    fn test_build_offline_model_config_sensevoice_branch() {
        let mut cfg = cfg_with(AsrModelKind::SenseVoice);
        cfg.model = Some(PathBuf::from("/m/sense/model.onnx"));
        cfg.tokens = PathBuf::from("/m/sense/tokens.txt");
        cfg.language = Some("zh".to_string());
        cfg.use_itn = Some(false);
        let c = build_offline_model_config(&cfg);
        assert_eq!(c.model_type.as_deref(), Some("sensevoice"));
        assert_eq!(c.sense_voice.model.as_deref(), Some("/m/sense/model.onnx"));
        assert_eq!(c.sense_voice.language.as_deref(), Some("zh"));
        assert!(!c.sense_voice.use_itn);
        assert_eq!(c.tokens.as_deref(), Some("/m/sense/tokens.txt"));
        // 不污染其它族字段
        assert!(c.whisper.encoder.is_none());
    }

    #[test]
    fn test_build_offline_model_config_whisper_branch() {
        let mut cfg = cfg_with(AsrModelKind::Whisper);
        cfg.encoder = PathBuf::from("/m/w/tiny-encoder.onnx");
        cfg.decoder = PathBuf::from("/m/w/tiny-decoder.onnx");
        cfg.tokens = PathBuf::from("/m/w/tiny-tokens.txt");
        let c = build_offline_model_config(&cfg);
        assert_eq!(c.model_type.as_deref(), Some("whisper"));
        assert_eq!(c.whisper.encoder.as_deref(), Some("/m/w/tiny-encoder.onnx"));
        assert_eq!(c.whisper.decoder.as_deref(), Some("/m/w/tiny-decoder.onnx"));
        assert_eq!(c.whisper.task.as_deref(), Some("transcribe"));
        assert_eq!(c.tokens.as_deref(), Some("/m/w/tiny-tokens.txt"));
        assert!(c.sense_voice.model.is_none());
    }

    #[test]
    fn test_offline_engine_rejects_streaming_kinds() {
        // zipformer / paraformer 均应被离线引擎拒绝（走流式引擎）
        for kind in [AsrModelKind::Zipformer, AsrModelKind::Paraformer] {
            let err = OfflineAsrEngine::new(cfg_with(kind)).err().unwrap();
            assert!(
                err.contains("流式引擎"),
                "kind={:?} 应报「应走流式引擎」，实际: {err}",
                kind
            );
        }
    }

    #[test]
    fn test_build_offline_model_config_qwen3_branch() {
        let mut cfg = cfg_with(AsrModelKind::Qwen3Asr);
        cfg.model = Some(PathBuf::from("/m/q3/conv_frontend.onnx"));
        cfg.encoder = PathBuf::from("/m/q3/encoder.int8.onnx");
        cfg.decoder = PathBuf::from("/m/q3/decoder.int8.onnx");
        cfg.tokens = PathBuf::from("/m/q3/tokenizer");
        cfg.hotwords = Some("尼日尔河 ZapMomo".to_string());
        let c = build_offline_model_config(&cfg);
        // qwen3 不设 model_type、tokens 置空串（官方示例模式）
        assert_eq!(c.model_type, None);
        assert_eq!(c.tokens.as_deref(), Some(""));
        assert_eq!(
            c.qwen3_asr.conv_frontend.as_deref(),
            Some("/m/q3/conv_frontend.onnx")
        );
        assert_eq!(
            c.qwen3_asr.encoder.as_deref(),
            Some("/m/q3/encoder.int8.onnx")
        );
        assert_eq!(
            c.qwen3_asr.decoder.as_deref(),
            Some("/m/q3/decoder.int8.onnx")
        );
        assert_eq!(c.qwen3_asr.tokenizer.as_deref(), Some("/m/q3/tokenizer"));
        // 热词空格分隔 → 逗号分隔（C++ 端按逗号切分嵌 prompt）
        assert_eq!(c.qwen3_asr.hotwords.as_deref(), Some("尼日尔河,ZapMomo"));
        // 生成参数透传 crate 默认
        assert_eq!(c.qwen3_asr.max_total_len, 512);
        assert_eq!(c.qwen3_asr.max_new_tokens, 128);
        // 不污染其它族字段
        assert!(c.sense_voice.model.is_none());
        assert!(c.whisper.encoder.is_none());

        // hotwords None / 纯空白 → None（不拼空 prompt）
        cfg.hotwords = None;
        assert_eq!(build_offline_model_config(&cfg).qwen3_asr.hotwords, None);
        cfg.hotwords = Some("   ".to_string());
        assert_eq!(build_offline_model_config(&cfg).qwen3_asr.hotwords, None);
    }

    #[test]
    fn test_offline_engine_qwen3_missing_files_and_tokenizer_dir() {
        // 路径不存在 → 先报缺少 tokenizer 目录（目录级校验先于 is_file 循环）
        let mut cfg = cfg_with(AsrModelKind::Qwen3Asr);
        cfg.model = Some(PathBuf::from("/nonexistent/conv_frontend.onnx"));
        cfg.encoder = PathBuf::from("/nonexistent/encoder.int8.onnx");
        cfg.decoder = PathBuf::from("/nonexistent/decoder.int8.onnx");
        cfg.tokens = PathBuf::from("/nonexistent/tokenizer");
        let err = OfflineAsrEngine::new(cfg).err().unwrap();
        assert!(
            err.contains("tokenizer"),
            "应报缺少 tokenizer 目录，实际: {err}"
        );

        // tokenizer 目录存在但 onnx 缺失 → 报「缺少模型文件」
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with(AsrModelKind::Qwen3Asr);
        cfg.model = Some(PathBuf::from("/nonexistent/conv_frontend.onnx"));
        cfg.encoder = PathBuf::from("/nonexistent/encoder.int8.onnx");
        cfg.decoder = PathBuf::from("/nonexistent/decoder.int8.onnx");
        cfg.tokens = dir.path().join("tokenizer");
        std::fs::create_dir_all(&cfg.tokens).unwrap();
        let err = OfflineAsrEngine::new(cfg).err().unwrap();
        assert!(
            err.contains("缺少模型文件"),
            "应报缺少模型文件，实际: {err}"
        );
    }

    #[test]
    #[ignore = "需要先运行 cargo run -- asr install-model --model asr-sensevoice-zh-en-ja-ko-yue 下载模型"]
    fn test_transcribe_samples_transcribes_pcm() {
        // 与 transcribe_wav 同源：喂真实模型的 16k 段样本，应出非空文本（VAD 分段听写的核心路径）
        use crate::asr::config;
        let cfg = config::resolve(None, None).unwrap();
        if cfg.model_type != crate::asr::config::AsrModelKind::SenseVoice {
            eprintln!("跳过：当前模型不是 SenseVoice");
            return;
        }
        let engine = OfflineAsrEngine::new(cfg.clone()).unwrap();
        let wav = crate::asr::default_test_wav(&cfg.model_dir).expect("模型自带示例音频");
        let wave = Wave::read(&wav.to_string_lossy()).unwrap();
        let text = engine
            .transcribe_samples(wave.samples(), wave.sample_rate())
            .unwrap();
        assert!(!text.trim().is_empty(), "应转写出非空文本，实际: {text}");
    }
}
