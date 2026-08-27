#!/usr/bin/env bash
# 为本地开发准备 audio.cpp sidecar 引擎（externalBin 要求
# src-tauri/binaries/audiocpp_server-<triple> 存在，否则 `pnpm tauri dev` 硬失败）。
#
# 两种模式：
#   scripts/fetch-audiocpp-dev.sh            # 从本仓库最新 Release 下载（日常，快）
#   scripts/fetch-audiocpp-dev.sh --build    # 本地源码编译（首次发版前 / 修改引擎时）
#
# 编译参数与 release.yml 一致：裁剪 pocket_tts+omnivoice+voxcpm2+qwen3_tts + DEPLOYMENT_BUILD
# （spec 内嵌）+ NATIVE_CPU=OFF（可移植）。产物约 12MB、编译约 2.5 分钟（macOS 实测）。
# 注意：上游 tag 命名是 release-X.Y.Z（无 v 前缀，v* 只有远古 windows-prebuilt）。
set -euo pipefail

cd "$(dirname "$0")/.."
REF="${AUDIOCPP_REF:-release-0.6.1}"

triple() {
  local t
  case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) t=aarch64-apple-darwin ;;
    Darwin/x86_64) t=x86_64-apple-darwin ;;
    Linux/x86_64) t=x86_64-unknown-linux-gnu ;;
    *) echo "不支持的平台: $(uname -s)/$(uname -m)" >&2; exit 1 ;;
  esac
  echo "$t"
}

TRIPLE="$(triple)"
SUFFIX=""
[ "$(uname -s)" = "Darwin" ] || SUFFIX=".exe" # Linux/macOS 无后缀；Windows 场景走 CI
DEST="src-tauri/binaries/audiocpp_server-${TRIPLE}"
mkdir -p src-tauri/binaries

if [ "${1:-}" = "--build" ]; then
  echo "==> 本地编译 audio.cpp ($REF)"
  rm -rf .audiocpp-src .audiocpp-build
  git clone --depth 1 --branch "$REF" https://github.com/0xShug0/audio.cpp .audiocpp-src
  METAL_FLAG="-DENGINE_ENABLE_METAL=ON"
  [ "$(uname -m)" = "arm64" ] || METAL_FLAG="-DENGINE_ENABLE_METAL=OFF"
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
    -DAUDIOCPP_MODEL_SET=custom -DAUDIOCPP_MODELS=pocket_tts,omnivoice,voxcpm2,qwen3_tts \
    -DAUDIOCPP_DEPLOYMENT_BUILD=ON -DENGINE_ENABLE_NATIVE_CPU=OFF \
    -DENGINE_ENABLE_CUDA=OFF -DENGINE_ENABLE_VULKAN=OFF -DENGINE_ENABLE_HIP=OFF \
    $METAL_FLAG -DCMAKE_BUILD_TYPE=Release
  cmake --build .audiocpp-build --target audiocpp_server --parallel "$(sysctl -n hw.ncpu 2>/dev/null || nproc || echo 4)"
  SRC_BIN="$(find .audiocpp-build -type f -name 'audiocpp_server*' ! -name '*.dSYM*' | head -1)"
  cp "$SRC_BIN" "$DEST"
else
  echo "==> 从 GitHub Release 下载 sidecar ($TRIPLE)"
  REPO="${GITHUB_REPOSITORY:-$(git remote get-url origin | sed -E 's#.*[:/]([^/]+/[^/.]+)(\.git)?$#\1#')}"
  URL="https://github.com/${REPO}/releases/latest/download/audiocpp_server-${TRIPLE}"
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
