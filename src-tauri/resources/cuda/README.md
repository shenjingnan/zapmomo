# cuda/

Windows CUDA 运行时 DLL 的收集目录（**构建期生成，不入库**——DLL 由 CI 的
`Collect CUDA runtime DLLs` 步骤或本地 `scripts/fetch-audiocpp-dev.sh --build`
填充，`*.dll` 在 .gitignore 中）。本 README 用于占位保证目录存在。

- 来源 1：audio.cpp 构建产物的 ggml DLL（`ggml-cuda.dll` 等）
- 来源 2：CUDA Toolkit（12.4 轨）`%CUDA_PATH%\bin` 下的
  `cudart64_* / cublas64_* / cublasLt64_* / cufft64_*.dll`

经 `tauri.windows.conf.json` 的 `bundle.resources` 随 Windows 安装包落
`<安装目录>\cuda\`；运行时由 `src/audiocpp/server.rs` 的
`augmented_child_path` 把该目录前置进子进程 PATH，供引擎解析 CUDA 后端。
