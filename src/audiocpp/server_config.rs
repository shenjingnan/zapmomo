use std::path::PathBuf;

use serde::Serialize;

use crate::tts::config::ResolvedTtsConfig;

/// audiocpp_server 的 `--config` 载荷（serde 镜像，键名与上游 `app/server/example.json` 对齐）。
///
/// 纯数据类型：由 [`build_server_config`] 从 `ResolvedTtsConfig` 生成，快照单测
/// 锁定 schema；上游 config 漂移时差异集中在此文件。
#[derive(Debug, Clone, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// ggml 推理后端：复用 `[tts].provider`（缺省 cpu——实测小模型 CPU 快于 Metal）
    pub backend: String,
    pub threads: i32,
    /// eager 加载：把「模型缺失」从首次请求前移到 spawn 健康检查阶段
    pub lazy_load: bool,
    pub models: Vec<ServerModelConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerModelConfig {
    /// 与 `/v1/audio/speech` 请求体 `model` 同源（见 [`super::families::AudiocppFamilyDesc::model_id`]）
    pub id: String,
    /// audio.cpp 模型族标识（见 [`super::families::AudiocppFamilyDesc::family`]）
    pub family: String,
    /// 主模型 GGUF 绝对路径
    pub path: String,
    pub task: String,
    pub mode: String,
    /// speaker embeddings 等按模型目录相对发现（实测 `embeddings/<voice>.safetensors`）
    pub load_options: serde_json::Value,
}

/// 由解析后的 TTS 配置生成 server config（纯函数，快照单测锚定两族）。
///
/// 模型族信息（id/family/gguf 路径/load_options/mode）查 [`super::families`] 描述表；
/// sherpa-only kind 配 audiocpp 后端的非法组合返回错误。`mode` 为族静态能力
/// （`supports_streaming` → "streaming"/"offline"）——offline-mode server 会拒绝
/// SSE 请求（实测 HTTP 500），故流式族的 mode 翻转是必要条件。mode 不进
/// `config_hash`：hash 已含 `model_type`，而 mode 是其纯函数（同版本内不会变）。
pub fn build_server_config(cfg: &ResolvedTtsConfig, port: u16) -> Result<ServerConfig, String> {
    let desc = super::families::family_desc(cfg.model_type)
        .ok_or_else(|| format!("模型类型 {} 不支持 audiocpp 后端", cfg.model_type.as_str()))?;
    Ok(ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        backend: cfg.provider.clone(),
        threads: cfg.num_threads,
        lazy_load: false,
        models: vec![ServerModelConfig {
            id: desc.model_id.to_string(),
            family: desc.family.to_string(),
            path: cfg.model_dir.join(desc.gguf_file).display().to_string(),
            task: "tts".to_string(),
            mode: if desc.supports_streaming {
                "streaming"
            } else {
                "offline"
            }
            .to_string(),
            load_options: desc.load_options(),
        }],
    })
}

/// server config 落盘路径：`<data_dir>/engines/audiocpp-server.json`。
///
/// 复用 engines 目录（与 pidfile 同层）；不用 tempfile（仅 dev-dependency）。
pub fn server_config_path() -> PathBuf {
    super::locator::engines_dir().join("audiocpp-server.json")
}

/// 生成并写入 server config，返回文件路径。
pub fn write_server_config(cfg: &ResolvedTtsConfig, port: u16) -> Result<PathBuf, String> {
    let path = server_config_path();
    let dir = path
        .parent()
        .ok_or_else(|| format!("配置路径无父目录: {}", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("创建 engines 目录失败 {}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(&build_server_config(cfg, port)?)
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

    fn audiocpp_cfg(model_dir: &std::path::Path) -> ResolvedTtsConfig {
        ResolvedTtsConfig {
            backend: TtsBackendKind::Audiocpp,
            model_dir: model_dir.to_path_buf(),
            ..ResolvedTtsConfig::default()
        }
    }

    #[test]
    fn test_build_server_config_shape() {
        let mut cfg = audiocpp_cfg(std::path::Path::new("/models/pocket-tts-english-audiocpp"));
        cfg.model_type = crate::tts::config::TtsModelKind::Pocket;
        let sc = build_server_config(&cfg, 18123).unwrap();
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

    /// omnivoice 族快照：family/id/gguf 路径/空 load_options；非法组合报错。
    #[test]
    fn test_build_server_config_omnivoice_shape() {
        let mut cfg = audiocpp_cfg(std::path::Path::new("/models/omnivoice-audiocpp"));
        cfg.model_type = crate::tts::config::TtsModelKind::Omnivoice;
        cfg.provider = "metal".to_string();
        let sc = build_server_config(&cfg, 18200).unwrap();
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
    fn test_build_server_config_rejects_sherpa_kind() {
        let cfg = audiocpp_cfg(std::path::Path::new("/m")); // model_type 缺省 Zipvoice
        let err = build_server_config(&cfg, 1).unwrap_err();
        assert!(err.contains("不支持 audiocpp 后端"), "err: {err}");
    }

    #[test]
    fn test_server_config_json_keys() {
        // 序列化键名与上游 example.json 对齐（schema 快照）
        let mut cfg = audiocpp_cfg(std::path::Path::new("/m"));
        cfg.model_type = crate::tts::config::TtsModelKind::Pocket;
        let json = serde_json::to_value(build_server_config(&cfg, 1).unwrap()).unwrap();
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
            let mut cfg = audiocpp_cfg(base.path());
            cfg.model_type = crate::tts::config::TtsModelKind::Pocket;
            let path = write_server_config(&cfg, 19999).unwrap();
            assert!(path.is_file());
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(content.contains("\"port\": 19999"));
            // 覆盖写（端口变更场景）
            write_server_config(&cfg, 20000).unwrap();
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(content.contains("\"port\": 20000"));
            // 无 .tmp 残留
            assert!(!path.with_extension("json.tmp").exists());
        });
    }
}
