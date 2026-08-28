import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { VoiceReplyBubble } from "@/components/bubble/VoiceReplyBubble";
import { useVoiceSession } from "@/hooks/useVoiceSession";
import { api, onCompanionLayerChanged } from "@/lib/tauri";
import type { CompanionWindowLayer } from "@/types/tauri";

/**
 * 回复气泡窗口根组件（bubble 窗口）：独立透明窗口，galgame 对话框位。
 *
 * - 显隐由后端跟随角色窗口控制（无独立开关）；本组件只负责内容渲染与交互态。
 * - 空闲点击穿透：无可见气泡内容时 `setIgnoreCursorEvents(true)`，透明区域
 *   不挡下方窗口点击；有内容时恢复接收鼠标（气泡面可拖动）。
 * - 拖动停止（debounce 300ms）后把逻辑像素坐标写回 settings，下次启动恢复。
 * - 角色置底（layer=back）时不渲染气泡（与迁移前 companion 内行为一致）。
 * - 底部 26px 透明外边距给 CSS 阴影留扩散空间（透明窗口会裁剪窗口外阴影）。
 */
export function BubbleRoot() {
  const voice = useVoiceSession();
  const [layer, setLayer] = useState<CompanionWindowLayer>("front");
  const [bubbleVisible, setBubbleVisible] = useState(false);

  // 初始层级回读 + 设置面板/菜单切换层级时同步
  useEffect(() => {
    api
      .getLive2dConfig()
      .then((config) => {
        if (config?.window_layer) setLayer(config.window_layer);
      })
      .catch(() => {});
    const unlisten = onCompanionLayerChanged(setLayer);
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // 空闲点穿：有可见气泡内容（且角色前置）时才接收鼠标（可拖动），否则穿透到下方窗口
  const interactive = bubbleVisible && layer === "front";
  useEffect(() => {
    void api.bubbleDebugLog({
      message: `interactive=${interactive} (bubbleVisible=${bubbleVisible}, layer=${layer}) → setIgnoreCursorEvents(${!interactive})`,
    });
    getCurrentWindow()
      .setIgnoreCursorEvents(!interactive)
      .then(() => void api.bubbleDebugLog({ message: "setIgnoreCursorEvents 成功" }))
      .catch((e) => void api.bubbleDebugLog({ message: `setIgnoreCursorEvents 失败: ${e}` }));
  }, [interactive, bubbleVisible, layer]);

  // 监听窗口移动：拖动停止（debounce）后把逻辑像素坐标写回 settings。
  useEffect(() => {
    const win = getCurrentWindow();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unlisten = win.onMoved(({ payload }) => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        void (async () => {
          const factor = await win.scaleFactor();
          const x = Math.round(payload.x / factor);
          const y = Math.round(payload.y / factor);
          await api.saveBubblePosition({ x, y });
        })();
      }, 300);
    });
    return () => {
      clearTimeout(timer);
      void unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <div className="flex h-screen w-screen items-end justify-center bg-transparent px-4 pt-3 pb-[26px]">
      {layer === "front" && (
        <VoiceReplyBubble
          text={voice.pendingReply}
          phase={voice.phase}
          onVisibleChange={setBubbleVisible}
        />
      )}
    </div>
  );
}
