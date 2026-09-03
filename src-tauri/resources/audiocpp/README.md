# audiocpp/

Windows 引擎运行时 DLL 的收集目录（**构建期生成，不入库**——DLL 由 CI 的
`Fetch audio.cpp prebuilt (Windows)` 步骤从上游 v0.7.1 官方预编译包填充；
本地 dev 手动把 `*.dll` 拷入本目录即可）。
本 README 用于占位保证目录存在（`tauri.conf.json` 构建脚本校验路径）。

- 来源 1：`audio-v0.7.1-bin-windows-x64-cuda12.4.zip` 的 ggml 动态后端
  （`ggml/ggml-base/ggml-cuda/ggml-cpu-*.dll`）与 MSVC CRT 运行时
- 来源 2：`audio-v0.7.1-cudart-windows-x64-cuda12.4.zip` 的 CUDA 12.4
  动态运行时（`cudart64_* / cublas64_* / cublasLt64_* / cufft64_*.dll`）

经 `tauri.windows.conf.json` 的 `bundle.resources`（空 target）随 Windows
安装包落到**安装根目录**、与 `audiocpp_server.exe` 同目录——ggml 的后端
枚举（`ggml_backend_load_best`）只扫「引擎 exe 目录 + 进程 CWD」、不查
PATH，DLL 必须与引擎同目录。运行时由 `src-tauri/src/lib.rs` 把该目录注入
搜索目录与子进程 PATH（解析加载期硬导入），`src/audiocpp/server.rs` 把
子进程 CWD 指向含 ggml 库的目录（兜底非同目录布局）。
