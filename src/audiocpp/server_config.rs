use std::path::PathBuf;

use serde::Serialize;

use crate::asr::config::ResolvedAsrConfig;
use crate::tts::config::ResolvedTtsConfig;

/// 任务中立的 server 实例规格（TTS/ASR 两个入口收敛；技术方案 §3.4）。
///
/// 每个 audiocpp_server 进程挂单个模型（路线 A：TTS 与 ASR 各自独立实例，
/// 靠 `task` 进 `config_hash` 天然隔离）。`from_tts` / `from_asr` 负责查各自
/// 的族描述表并做非法组合校验，spec 一旦构造即合法。
#[derive(Debug, Clone)]
pub struct ServerInstanceSpec {
    /// server config `models[].task`（"tts" | "asr"），同时进 `config_hash` 防撞。
    pub task: &'static str,
    /// server config `models[].id` 与请求体 `model` 同源。
    pub model_id: String,
    /// audio.cpp `model_specs` 的 family 标识。
    pub family: String,
    /// 主模型 GGUF 绝对路径。
    pub model_path: PathBuf,
    /// server config `models[].mode`（TTS 按族流式能力翻转；ASR 首版恒 "offline"）。
    pub mode: String,
    /// server config `models[].load_options`（TTS 按族；ASR 首版空）。
    pub load_options: serde_json::Value,
    /// ggml 推理后端（server 顶层 `backend`）。
    pub provider: String,
    pub num_threads: i32,
    /// 模型目录（`config_hash` 维度）。
    pub model_dir: PathBuf,
    /// 引擎二进制覆盖路径（None = locator 自动定位）。
    pub engine_path: Option<PathBuf>,
}

impl ServerInstanceSpec {
    /// 从解析后的 TTS 配置生成实例规格（查 [`super::families`] 描述表）。
    ///
    /// `mode` 为族静态能力（`supports_streaming` → "streaming"/"offline"）——
    /// offline-mode server 会拒绝 SSE 请求（实测 HTTP 500），故流式族的 mode
    /// 翻转是必要条件。sherpa-only kind 配 audiocpp 后端的非法组合返回错误。
    pub fn from_tts(cfg: &ResolvedTtsConfig) -> Result<Self, String> {
        let desc = super::families::family_desc(cfg.model_type)
            .ok_or_else(|| format!("模型类型 {} 不支持 audiocpp 后端", cfg.model_type.as_str()))?;
        Ok(Self {
            task: "tts",
            model_id: desc.model_id.to_string(),
            family: desc.family.to_string(),
            model_path: cfg.model_dir.join(desc.gguf_file),
            mode: if desc.supports_streaming {
                "streaming"
            } else {
                "offline"
            }
            .to_string(),
            load_options: desc.load_options(),
            provider: cfg.provider.clone(),
            num_threads: cfg.num_threads,
            model_dir: cfg.model_dir.clone(),
            engine_path: cfg.engine_path.clone(),
        })
    }

    /// 从解析后的 ASR 配置生成实例规格（查 [`super::asr_families`] 描述表）。
    ///
    /// ASR 首版恒 offline 整段转写（live 流式留后续）；load_options 为空。
    pub fn from_asr(cfg: &ResolvedAsrConfig) -> Result<Self, String> {
        let desc = super::asr_families::asr_family_desc(cfg.model_type)
            .ok_or_else(|| format!("模型类型 {} 不支持 audiocpp 后端", cfg.model_type.as_str()))?;
        Ok(Self {
            task: "asr",
            model_id: desc.model_id.to_string(),
            family: desc.family.to_string(),
            model_path: cfg.model_dir.join(desc.gguf_file),
            mode: "offline".to_string(),
            load_options: serde_json::json!({}),
            provider: cfg.provider.clone(),
            num_threads: cfg.num_threads,
            model_dir: cfg.model_dir.clone(),
            engine_path: cfg.engine_path.clone(),
        })
    }
}

/// audiocpp_server 的 `--config` 载荷（serde 镜像，键名与上游 `app/server/example.json` 对齐）。
///
/// 纯数据类型：由 [`build_server_config`] 从 [`ServerInstanceSpec`] 生成，快照单测
/// 锁定 schema；上游 config 漂移时差异集中在此文件。
#[derive(Debug, Clone, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// ggml 推理后端（spec.provider，缺省按族：pocket cpu / 其余 metal）
    pub backend: String,
    pub threads: i32,
    /// eager 加载：把「模型缺失」从首次请求前移到 spawn 健康检查阶段
    pub lazy_load: bool,
    pub models: Vec<ServerModelConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerModelConfig {
    /// 与请求体 `model` 同源（见 [`ServerInstanceSpec::model_id`]）
    pub id: String,
    /// audio.cpp 模型族标识
    pub family: String,
    /// 主模型 GGUF 绝对路径
    pub path: String,
    pub task: String,
    pub mode: String,
    /// speaker embeddings 等按模型目录相对发现（实测 `embeddings/<voice>.safetensors`）
    pub load_options: serde_json::Value,
}

/// 由实例规格生成 server config（纯函数，快照单测锚定）。
///
/// 单模型 vec（路线 A：TTS/ASR 各自独立实例）。`mode` 不进 `config_hash`：
/// hash 已含 model_id，而 mode 是其纯函数（同版本内不会变）。
pub fn build_server_config(spec: &ServerInstanceSpec, port: u16) -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        backend: spec.provider.clone(),
        threads: spec.num_threads,
        lazy_load: false,
        models: vec![ServerModelConfig {
            id: spec.model_id.clone(),
            family: spec.family.clone(),
            path: spec.model_path.display().to_string(),
            task: spec.task.to_string(),
            mode: spec.mode.clone(),
            load_options: spec.load_options.clone(),
        }],
    }
}

/// server config 落盘路径（按实例指纹分文件）：`<data_dir>/engines/audiocpp-server-<hash>.json`。
///
/// 多实例并存（TTS 切模型窗口 / TTS+ASR 双任务）下，全局单文件存在 write→spawn
/// 与子进程读 config 的竞态窗口，按指纹分文件后各实例读写互不相干；实例回收时
/// 删除对应文件，孤儿清理（`server::reap_orphan_process`）兜底扫描残留。
pub fn server_config_path(hash: u64) -> PathBuf {
    super::locator::engines_dir().join(format!("audiocpp-server-{hash}.json"))
}

/// 生成并写入 server config，返回文件路径。
pub fn write_server_config(
    spec: &ServerInstanceSpec,
    port: u16,
    hash: u64,
) -> Result<PathBuf, String> {
    let path = server_config_path(hash);
    let dir = path
        .parent()
        .ok_or_else(|| format!("配置路径无父目录: {}", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("创建 engines 目录失败 {}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(&build_server_config(spec, port))
        .map_err(|e| format!("序列化 server config 失败: {e}"))?;
    // 先写临时文件再原子 rename，避免 server 读到半截 config
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)
        .map_err(|e| format!("写入 server config 失败 {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("落位 server config 失败 {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::config::TtsBackendKind;

    fn audiocpp_tts_cfg(model_dir: &std::path::Path) -> ResolvedTtsConfig {
        ResolvedTtsConfig {
            backend: TtsBackendKind::Audiocpp,
            model_dir: model_dir.to_path_buf(),
            ..ResolvedTtsConfig::default()
        }
    }

    fn pocket_spec(model_dir: &std::path::Path) -> ServerInstanceSpec {
        let mut cfg = audiocpp_tts_cfg(model_dir);
        cfg.model_type = crate::tts::config::TtsModelKind::Pocket;
        ServerInstanceSpec::from_tts(&cfg).unwrap()
    }

    #[test]
    fn test_spec_from_tts_and_config_shape() {
        let spec = pocket_spec(std::path::Path::new("/models/pocket-tts-english-audiocpp"));
        assert_eq!(spec.task, "tts");
        let sc = build_server_config(&spec, 18123);
        assert_eq!(sc.host, "127.0.0.1");
        assert_eq!(sc.port, 18123);
        assert_eq!(sc.backend, "cpu");
        assert_eq!(sc.threads, 2);
        assert!(!sc.lazy_load, "默认 eager，模型缺失前移到健康检查");
        assert_eq!(sc.models.len(), 1);
        let m = &sc.models[0];
        assert_eq!(m.id, "pocket-tts-english");
        assert_eq!(m.family, "pocket_tts");
        // 路径分隔符按平台归一后断言（Windows 的 PathBuf::join 产生 `\`，
        // server 侧同为原生程序读取，分隔符不影响运行时）
        assert_eq!(
            m.path.replace('\\', "/"),
            "/models/pocket-tts-english-audiocpp/pocket-tts-english-q8_0.gguf"
        );
        assert_eq!(m.task, "tts");
        assert_eq!(m.mode, "offline", "pocket 无流式支持，mode 保持 offline");
        assert_eq!(m.load_options["language"], "english");
    }

    /// omnivoice 族快照：family/id/gguf 路径/空 load_options/metal provider。
    #[test]
    fn test_spec_from_tts_omnivoice_shape() {
        let mut cfg = audiocpp_tts_cfg(std::path::Path::new("/models/omnivoice-audiocpp"));
        cfg.model_type = crate::tts::config::TtsModelKind::Omnivoice;
        cfg.provider = "metal".to_string();
        let spec = ServerInstanceSpec::from_tts(&cfg).unwrap();
        let sc = build_server_config(&spec, 18200);
        assert_eq!(sc.backend, "metal");
        let m = &sc.models[0];
        assert_eq!(m.id, "omnivoice");
        assert_eq!(m.family, "omnivoice");
        assert_eq!(
            m.path.replace('\\', "/"),
            "/models/omnivoice-audiocpp/omnivoice-q8_0.gguf"
        );
        // 流式族 mode 翻转为 streaming（offline server 拒绝 SSE 请求，实测 HTTP 500）
        assert_eq!(m.mode, "streaming");
        assert_eq!(m.load_options, serde_json::json!({}));
    }

    /// sherpa kind（默认 Zipvoice）配 audiocpp 后端 → 明确报错。
    #[test]
    fn test_spec_from_tts_rejects_sherpa_kind() {
        let cfg = audiocpp_tts_cfg(std::path::Path::new("/m")); // model_type 缺省 Zipvoice
        let err = ServerInstanceSpec::from_tts(&cfg).unwrap_err();
        assert!(err.contains("不支持 audiocpp 后端"), "err: {err}");
    }

    /// ASR 入口快照：task=asr / mode=offline / 空 load_options / GGUF 路径；
    /// sherpa-only kind 组合报错。
    #[test]
    fn test_spec_from_asr_qwen3_shape() {
        let cfg = ResolvedAsrConfig {
            backend: crate::asr::config::AsrBackendKind::Audiocpp,
            model_type: crate::asr::config::AsrModelKind::Qwen3Asr,
            model_dir: std::path::PathBuf::from("/models/qwen3-asr-0.6b-audiocpp"),
            provider: "metal".to_string(),
            ..ResolvedAsrConfig::default()
        };
        let spec = ServerInstanceSpec::from_asr(&cfg).unwrap();
        assert_eq!(spec.task, "asr");
        let sc = build_server_config(&spec, 18300);
        assert_eq!(sc.backend, "metal");
        let m = &sc.models[0];
        assert_eq!(m.id, "qwen3-asr-0.6b");
        assert_eq!(m.family, "qwen3_asr");
        assert_eq!(
            m.path.replace('\\', "/"),
            "/models/qwen3-asr-0.6b-audiocpp/qwen3-asr-0.6b-q8_0.gguf"
        );
        assert_eq!(m.task, "asr");
        assert_eq!(m.mode, "offline", "ASR 首版恒 offline 整段");
        assert_eq!(m.load_options, serde_json::json!({}));

        let bad = ResolvedAsrConfig {
            backend: crate::asr::config::AsrBackendKind::Audiocpp,
            model_type: crate::asr::config::AsrModelKind::Zipformer,
            ..ResolvedAsrConfig::default()
        };
        let err = ServerInstanceSpec::from_asr(&bad).unwrap_err();
        assert!(err.contains("不支持 audiocpp 后端"), "err: {err}");
    }

    #[test]
    fn test_server_config_json_keys() {
        // 序列化键名与上游 example.json 对齐（schema 快照）
        let spec = pocket_spec(std::path::Path::new("/m"));
        let json = serde_json::to_value(build_server_config(&spec, 1)).unwrap();
        for key in ["host", "port", "backend", "threads", "lazy_load", "models"] {
            assert!(json.get(key).is_some(), "missing key: {key}");
        }
        let m = &json["models"][0];
        for key in ["id", "family", "path", "task", "mode", "load_options"] {
            assert!(m.get(key).is_some(), "missing model key: {key}");
        }
    }

    #[test]
    fn test_write_server_config_atomic() {
        let base = tempfile::tempdir().unwrap();
        crate::test_util::run_with_temp_home(|home| {
            crate::test_util::set_custom_data_dir(home);
            // 写入前 engines 目录不存在也能创建
            let spec = pocket_spec(base.path());
            let path = write_server_config(&spec, 19999, 42).unwrap();
            assert!(path.is_file());
            assert!(
                path.file_name().unwrap().to_string_lossy() == "audiocpp-server-42.json",
                "按指纹分文件: {}",
                path.display()
            );
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(content.contains("\"port\": 19999"));
            // 覆盖写（端口变更场景）
            write_server_config(&spec, 20000, 42).unwrap();
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(content.contains("\"port\": 20000"));
            // 不同指纹不互相覆盖（多实例并存前提）
            let other = write_server_config(&spec, 20001, 43).unwrap();
            assert_ne!(path, other);
            assert!(path.is_file() && other.is_file());
            // 无 .tmp 残留
            assert!(!path.with_extension("json.tmp").exists());
        });
    }
}
