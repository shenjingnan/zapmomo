#!/usr/bin/env bash
# 为本地开发准备 audio.cpp sidecar 引擎（externalBin 要求
# src-tauri/binaries/audiocpp_server-<triple> 存在，否则 `pnpm tauri dev` 硬失败）。
#
# 两种模式：
#   scripts/fetch-audiocpp-dev.sh            # 从本仓库最新 Release 下载（日常，快）
#   scripts/fetch-audiocpp-dev.sh --build    # 本地源码编译（首次发版前 / 修改引擎时）
#
# 编译参数与 release.yml 一致：裁剪 omnivoice+voxcpm2+qwen3_tts+qwen3_asr
# + DEPLOYMENT_BUILD
# （spec 内嵌）+ NATIVE_CPU=OFF（可移植）。产物约 12MB、编译约 2.5 分钟（macOS 实测）。
# 注意：上游 tag 命名 0.6.x 为 release-X.Y.Z（无 v 前缀），0.7.x 起为 vX.Y.Z。
#
# Windows（Git Bash / MSYS）：仅支持 --build 模式（Release 资产 URL 带 .exe 后缀
# 需发版流程同步调整；生产 Windows 包走 release.yml 直接消费上游官方预编译）。
# 检测到 CUDA Toolkit（%CUDA_PATH% 有 nvcc）时自动开 CUDA 后端并收集运行时
# DLL 到 src-tauri/binaries/（server.rs 的子进程 PATH 前置会覆盖该目录），
# 否则引擎仅 CPU（合成时自动回退、速度慢）。0.7.1 起上游为动态 ggml 后端
# （ggml/ggml-base/ggml-cpu-*.dll），无论开不开 CUDA 都会把构建树的 *.dll
# 一并拷到引擎旁，否则 exe 缺 ggml-base.dll 无法启动。
set -euo pipefail

cd "$(dirname "$0")/.."
REF="${AUDIOCPP_REF:-v0.7.1}"

triple() {
  local t
  case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) t=aarch64-apple-darwin ;;
    Darwin/x86_64) t=x86_64-apple-darwin ;;
    Linux/x86_64) t=x86_64-unknown-linux-gnu ;;
    MINGW*|MSYS*|CYGWIN*) t=x86_64-pc-windows-msvc ;;
    *) echo "不支持的平台: $(uname -s)/$(uname -m)" >&2; exit 1 ;;
  esac
  echo "$t"
}

TRIPLE="$(triple)"
SUFFIX=""
case "$(uname -s)" in
  Darwin|Linux) SUFFIX="" ;;    # Linux/macOS 无后缀
  *) SUFFIX=".exe" ;;           # Windows（MINGW/MSYS/CYGWIN）带 .exe
esac
DEST="src-tauri/binaries/audiocpp_server-${TRIPLE}${SUFFIX}"
mkdir -p src-tauri/binaries

if [ "${1:-}" = "--build" ]; then
  echo "==> 本地编译 audio.cpp ($REF)"
  rm -rf .audiocpp-src .audiocpp-build
  git clone --depth 1 --branch "$REF" https://github.com/0xShug0/audio.cpp .audiocpp-src
  METAL_FLAG="-DENGINE_ENABLE_METAL=ON"
  [ "$(uname -m)" = "arm64" ] || METAL_FLAG="-DENGINE_ENABLE_METAL=OFF"
  CUDA_FLAG="-DENGINE_ENABLE_CUDA=OFF"
  # Windows：检测 CUDA Toolkit（有 nvcc 才开 CUDA），并保留 CPU 回退能力
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
      METAL_FLAG="-DENGINE_ENABLE_METAL=OFF"
      if [ -x "${CUDA_PATH:-}/bin/nvcc.exe" ]; then
        CUDA_FLAG="-DENGINE_ENABLE_CUDA=ON"
        echo "==> 检测到 CUDA Toolkit: $CUDA_PATH（启用 CUDA 后端）"
      else
        echo "==> 未检测到 CUDA Toolkit（引擎仅 CPU；安装 CUDA Toolkit 后重跑可启用）"
      fi
      ;;
  esac
  # macOS：Apple Clang 不带 OpenMP，需要 brew 的 libomp（keg-only，须用
  # OpenMP_ROOT 让 CMake 的 FindOpenMP 定位；与 release.yml 的 arm64 job 一致）
  if [ "$(uname -s)" = "Darwin" ]; then
    LIBOMP_PREFIX="$(brew --prefix libomp 2>/dev/null || true)"
    if [ -n "$LIBOMP_PREFIX" ] && [ -d "$LIBOMP_PREFIX/lib" ]; then
      export OpenMP_ROOT="$LIBOMP_PREFIX"
      echo "==> 使用 libomp: $LIBOMP_PREFIX"
    else
      echo "错误：未检测到 libomp（audio.cpp 的 ENGINE_ENABLE_OPENMP 默认 ON）。" >&2
      echo "请先执行: brew install libomp" >&2
      exit 1
    fi
  fi
  # shellcheck disable=SC2086
  cmake -S .audiocpp-src -B .audiocpp-build \
    -DAUDIOCPP_MODEL_SET=custom -DAUDIOCPP_MODELS=omnivoice,voxcpm2,qwen3_tts,qwen3_asr \
    -DAUDIOCPP_DEPLOYMENT_BUILD=ON -DENGINE_ENABLE_NATIVE_CPU=OFF \
    $CUDA_FLAG -DENGINE_ENABLE_VULKAN=OFF -DENGINE_ENABLE_HIP=OFF \
    $METAL_FLAG -DCMAKE_BUILD_TYPE=Release
  JOBS="${NUMBER_OF_PROCESSORS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)}"
  cmake --build .audiocpp-build --target audiocpp_server --parallel "$JOBS"
  # MSVC multi-config 产物在 bin/Release/ 子目录，find 兜底两种布局
  SRC_BIN="$(find .audiocpp-build -type f -name 'audiocpp_server*' ! -name '*.dSYM*' | head -1)"
  cp "$SRC_BIN" "$DEST"
  # Windows：0.7.1 起上游为动态 ggml 后端（ggml/ggml-base/ggml-cpu-*.dll），
  # CPU-only 源码编译同样产出这些 DLL——无论开不开 CUDA 都拷到引擎旁（引擎
  # exe 硬导入 ggml-base.dll，缺了无法启动；引擎目录是子进程 PATH 首位，
  # server.rs::augmented_child_path 前置）。CUDA=ON 时再补 Toolkit 的运行时。
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
      while IFS= read -r -d '' dll; do cp "$dll" src-tauri/binaries/; done \
        < <(find .audiocpp-build -type f -name '*.dll' -print0)
      if [[ "$CUDA_FLAG" == *CUDA=ON* ]]; then
        for prefix in cudart64_ cublas64_ cublasLt64_ cufft64_; do
          cp "$CUDA_PATH"/bin/"${prefix}"*.dll src-tauri/binaries/ 2>/dev/null || true
        done
      fi
      ls -lh src-tauri/binaries/*.dll | head -20
      ;;
  esac
else
  echo "==> 从 GitHub Release 下载 sidecar ($TRIPLE)"
  REPO="${GITHUB_REPOSITORY:-$(git remote get-url origin | sed -E 's#.*[:/]([^/]+/[^/.]+)(\.git)?$#\1#')}"
  URL="https://github.com/${REPO}/releases/latest/download/audiocpp_server-${TRIPLE}${SUFFIX}"
  if ! curl -fsSL --fail -o "$DEST" "$URL"; then
    echo "下载失败（Release 可能尚未附带 sidecar 产物）。" >&2
    echo "首次发版前请用: scripts/fetch-audiocpp-dev.sh --build" >&2
    exit 1
  fi
fi

chmod +x "$DEST"
ls -lh "$DEST"
echo "完成：$DEST"
echo "提示：也可以不放此处——引擎还可放在 ~/.zapmomo/engines/ 或 PATH 中（locator 自动发现）。"
