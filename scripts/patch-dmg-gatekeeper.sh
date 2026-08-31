#!/usr/bin/env bash
# 向 dmg 安装镜像注入「首次打开修复.command」。
#
# 背景：ZapMomo 未经 Apple 签名公证，macOS 首次打开会被 Gatekeeper 拦截，报
# 「"ZapMomo" 已损坏，无法打开」（实为隔离属性作祟，并非真损坏），需执行
#   xattr -cr /Applications/ZapMomo.app
# 把修复命令做成 dmg 里的双击脚本：双击即自动安装到「应用程序」+ 清隔离 + 启动，
# 用户不必先去阅读 README。
#
# 用法: patch-dmg-gatekeeper.sh <xxx.dmg>   （仅 macOS，依赖 hdiutil/lipo）
#
# 实现：Tauri 没有往 dmg 根目录放额外文件的配置（bundle.macOS.files 进的是
# .app 内部），只能在产物 dmg 上后处理：挂载 → 连同 .DS_Store / .background
# 原样拷出（保住安装窗口布局）→ 加入修复脚本 → Finder AppleScript 固定 fixer
# 图标位置（create-dmg/Tauri bundle_dmg 同款机制）→ 转压缩镜像。
set -euo pipefail

DMG="${1:?用法: $0 <dmg 路径>}"
VOL_NAME="ZapMomo" # Tauri 的 dmg 卷名 = productName，重建时保持一致
APP_NAME="ZapMomo" # .app 名 = productName
FIXER_NAME="首次打开修复.command"

[ -f "$DMG" ] || { echo "!! 找不到 dmg: $DMG" >&2; exit 1; }

WORK="$(mktemp -d)"
STAGE="$WORK/stage"
MNT="$WORK/mnt"
mkdir -p "$STAGE"
MOUNTED=0
cleanup() {
  # 中途失败也要卸载，否则同 runner 重试报 resource busy
  if [ "$MOUNTED" -eq 1 ] && [ -n "${DEV:-}" ]; then
    hdiutil detach "$DEV" -force -quiet >/dev/null 2>&1 || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# 挂载原镜像并全量拷贝（含 .DS_Store/.background 隐藏文件与 Applications 符号链接，
# 缺了它们安装窗口的布局/背景就没了）
hdiutil attach "$DMG" -mountpoint "$MNT" -nobrowse -readonly -quiet
MOUNTED=1
cp -a "$MNT"/. "$STAGE"/
hdiutil detach "$MNT" -quiet
MOUNTED=0

# 产物防呆：bundler 布局变化时在这里立刻失败，而不是发出装不出来的 dmg
[ -d "$STAGE/$APP_NAME.app" ] || {
  echo "!! dmg 内未找到 $APP_NAME.app，Tauri bundler 布局可能已变化" >&2
  exit 1
}

# 探测本 dmg 的架构（读 .app 主二进制，不依赖文件名）；
# universal 或探测失败时留空 = fixer 跳过架构校验（fail-open）
APP_BIN="$STAGE/$APP_NAME.app/Contents/MacOS/$APP_NAME"
ARCHS="$(lipo -archs "$APP_BIN" 2>/dev/null || true)"
DMG_ARCH=""
case "$ARCHS" in
*arm64*x86_64* | *x86_64*arm64*) DMG_ARCH="" ;;
*arm64*) DMG_ARCH="arm64" ;;
*x86_64*) DMG_ARCH="x86_64" ;;
*) DMG_ARCH="" ;;
esac

# 两段写入：头段未加引号注入探测到的架构，主体段加引号避免展开脚本里的 $
cat > "$STAGE/$FIXER_NAME" <<FIXER_HEADER
#!/bin/bash
# 双击本文件：自动把 ZapMomo 安装到「应用程序」，清除 Gatekeeper 隔离属性并启动。
# （应用未做 Apple 签名公证，macOS 会拦截并提示「已损坏，无法打开」——并非真的损坏。）
DMG_ARCH="$DMG_ARCH"
FIXER_HEADER
cat >> "$STAGE/$FIXER_NAME" <<'FIXER_BODY'
set -u
APP="/Applications/ZapMomo.app"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SRC="$SCRIPT_DIR/ZapMomo.app"

clear
echo "==> ZapMomo 首次打开修复"
echo

# --- 1. 架构校验：Intel Mac 装 arm64 包跑不起来，提前拦下 ---
HOST_ARCH="$(uname -m)"
if [ -n "$DMG_ARCH" ] && [ "$DMG_ARCH" != "$HOST_ARCH" ]; then
  if [ "$HOST_ARCH" = "x86_64" ]; then
    echo "!! 当前是 Intel Mac，但下载的是 Apple Silicon（arm64）版，装上也无法运行。"
    echo "    请回下载页改用 x64 (Intel) 版的 dmg，再重新双击本文件。"
    echo
    echo "（按任意键关闭窗口）"
    read -r -n 1 -s
    exit 1
  fi
  echo "!! 当前是 Apple Silicon，但下载的是 Intel (x64) 版（需 Rosetta 2 转译，性能较差）。"
  echo "    建议改用 arm64 版的 dmg；仍要继续请按任意键……"
  read -r -n 1 -s
  echo
fi

# --- 2. 未安装则自动安装（ditto 保留权限/扩展属性，等同 Finder 拖拽） ---
if [ ! -d "$APP" ]; then
  if [ ! -d "$SRC" ]; then
    echo "!! 未找到已安装的 ZapMomo，且本文件所在目录也没有 ZapMomo.app。"
    echo "    请打开下载的 dmg，双击其中的「首次打开修复.command」。"
    echo
    echo "（按任意键关闭窗口）"
    read -r -n 1 -s
    exit 1
  fi
  echo "==> 正在把 ZapMomo 安装到「应用程序」（约十几秒）..."
  if ! ditto "$SRC" "$APP"; then
    echo "!! 自动安装失败（当前账户可能没有「应用程序」的写入权限）。"
    echo "    请改为手动操作：把 ZapMomo 拖入「应用程序」（Finder 会提示输入管理员"
    echo "    账户密码），完成后重新双击本文件即可继续修复。"
    echo
    echo "（按任意键关闭窗口）"
    read -r -n 1 -s
    exit 1
  fi
else
  echo "==> 已安装，直接修复..."
fi

# --- 3. 清除隔离属性并启动 ---
echo "==> 正在清除 Gatekeeper 隔离属性..."
if xattr -cr "$APP"; then
  echo "==> 修复完成，正在启动 ZapMomo ..."
  sleep 1
  open "$APP"
else
  echo "!! 修复失败：请手动执行  xattr -cr \"$APP\"  或到 GitHub Issues 反馈"
fi
echo
echo "（按任意键关闭窗口）"
read -r -n 1 -s
# 用完弹出本 dmg（best-effort）。必须在最后执行：bash 增量读脚本文件，
# 过早 detach 自身所在卷会导致后续行读取失败
if [ "$(basename "$SCRIPT_DIR")" = "ZapMomo" ] && [ "${SCRIPT_DIR#/Volumes/}" != "$SCRIPT_DIR" ]; then
  hdiutil detach "$SCRIPT_DIR" -quiet >/dev/null 2>&1 || true
fi
FIXER_BODY
chmod +x "$STAGE/$FIXER_NAME"

# 先重建为可写镜像：用 Finder 把 fixer 图标钉到 app / Applications 行正下方居中位
# （Tauri 默认窗口 660x400，app 在 (175,120)、Applications 在 (425,120)，二者中点
# x=300；y=250 位于窗口下半空区视觉焦点）。注入的文件本无定位，Finder 会随手乱放，
# 不钉位用户容易看不见。Tauri bundler 自身即在 CI 上用同款 AppleScript 定位图标。
# 定位失败不阻断发布（图标退化为 Finder 自动摆放，注入本身不受影响）。
FIXER_POS_X=300
FIXER_POS_Y=250
RW_DMG="$WORK/rw.dmg"
hdiutil create -volname "$VOL_NAME" -srcfolder "$STAGE" -ov -format UDRW -quiet "$RW_DMG"
# 用默认挂载点（/Volumes/<卷名>）：Finder 只为挂在 /Volumes 下的卷注册 disk
# 对象，-mountpoint 挂进深层目录（如 CI workspace）或 -nobrowse 都会让下方
# AppleScript 报 Can't get disk。解析设备名与实际挂载点供定位/卸载使用。
ATTACH_OUT="$(hdiutil attach "$RW_DMG" -noautoopen -readwrite)"
# APFS 镜像 attach 输出多行，挂载行在最后；字段间还有对齐空格需去掉
DEV="$(printf '%s\n' "$ATTACH_OUT" | awk -F'\t' 'END{print $1}' | tr -d ' ')"
MNT_REAL="$(printf '%s\n' "$ATTACH_OUT" | awk -F'\t' 'END{print $3}' | tr -d ' ')"
MOUNTED=1
# 同名卷被占用时挂载点会带 N 后缀，此时 tell disk 会命中错卷，宁可不定位
if [ "$MNT_REAL" = "/Volumes/$VOL_NAME" ]; then
  osascript <<APPLESCRIPT || echo "::warning::未能固定 ${FIXER_NAME} 图标位置（不影响注入，图标将由 Finder 自动摆放）"
tell application "Finder"
  tell disk "$VOL_NAME"
    open
    set current view of container window to icon view
    set theViewOptions to the icon view options of container window
    set arrangement of theViewOptions to not arranged
    delay 3
    set position of item "$FIXER_NAME" of container window to {$FIXER_POS_X, $FIXER_POS_Y}
    close
  end tell
end tell
APPLESCRIPT
else
  echo "::warning::dmg 卷挂载于 $MNT_REAL 而非 /Volumes/${VOL_NAME}，跳过图标定位"
fi
# Finder 异步落盘 .DS_Store，等它写完再卸载
sleep 3
sync
hdiutil detach "$DEV" -quiet
MOUNTED=0

# 转压缩镜像（UDZO，与 create-dmg/Tauri 默认一致），原地覆盖（文件名不变，
# 下游 release job 的 `*aarch64.dmg` / `*x64.dmg` 重命名规则不受影响）
rm -f "$DMG"
hdiutil convert "$RW_DMG" -format UDZO -o "$DMG" -ov -quiet
echo "已注入 ${FIXER_NAME}（图标位置 {$FIXER_POS_X, $FIXER_POS_Y}）-> $DMG"
