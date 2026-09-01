//! audiocpp 引擎的缺省推理后端（server config `backend`）按平台解析。
//!
//! 发布引擎的后端编入随平台不同（release.yml matrix）：
//!
//! | triple | 引擎编入后端 | 缺省 provider |
//! |---|---|---|
//! | `darwin-aarch64` | Metal | `metal`（omnivoice 0.6B 实测 CPU RTF 6.6 不可用、Metal 0.41 达标，技术方案阶段 1 实测 2026-08-23） |
//! | `windows-x86_64` | CUDA | `cuda`（上游 VoxCPM2 CUDA 实测 RTF 0.23~0.55；无 N 卡 / 驱动过旧时由 `server::lease` 自动回退 CPU） |
//! | `darwin-x86_64` / linux | CPU | `cpu` |
//!
//! 用户显式配置（`[tts].provider` / `[asr].provider`）永远优先于本缺省。

/// 平台三元组 → 缺省推理后端。
pub fn default_provider_for(triple: &str) -> &'static str {
    match triple {
        "windows-x86_64" => "cuda",
        "darwin-aarch64" => "metal",
        _ => "cpu",
    }
}

/// 当前平台的缺省推理后端。
pub fn current_default_provider() -> &'static str {
    default_provider_for(crate::model_library::registry::current_platform_triple())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全三元组映射表（宿主平台无关，CI 三平台同断言）。
    #[test]
    fn test_default_provider_by_triple() {
        assert_eq!(default_provider_for("windows-x86_64"), "cuda");
        assert_eq!(default_provider_for("darwin-aarch64"), "metal");
        assert_eq!(default_provider_for("darwin-x86_64"), "cpu");
        assert_eq!(default_provider_for("linux-x86_64"), "cpu");
        // 未知三元组保守回退 CPU
        assert_eq!(default_provider_for("freebsd-x86_64"), "cpu");
    }
}
