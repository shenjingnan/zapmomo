# audiocpp/

Windows 引擎运行时 DLL 的收集目录（**构建期生成，不入库**——DLL 由 CI 的
`Fetch audio.cpp prebuilt (Windows)` 步骤从上游 v0.7.1 官方预编译包填充）。
本 README 用于占位保证目录存在（`tauri.conf.json` 构建脚本校验路径）。

- 来源 1：`audio-v0.7.1-bin-windows-x64-cuda12.4.zip` 的 ggml 动态后端
  （`ggml/ggml-base/ggml-cuda/ggml-cpu-*.dll`）与 MSVC CRT 运行时
- 来源 2：`audio-v0.7.1-cudart-windows-x64-cuda12.4.zip` 的 CUDA 12.4
  动态运行时（`cudart64_* / cublas64_* / cublasLt64_* / cufft64_*.dll`）

经 `tauri.windows.conf.json` 的 `bundle.resources` 随 Windows 安装包落
`<安装目录>\audiocpp\`；运行时由 `src-tauri/src/lib.rs` 把该目录注入
搜索目录、`src/audiocpp/server.rs` 的 `augmented_child_path` 前置进子
进程 PATH，供引擎解析动态后端与 CUDA 运行时。
