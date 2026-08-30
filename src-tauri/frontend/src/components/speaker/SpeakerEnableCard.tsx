import { Fingerprint } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { useRuntime } from "@/providers/RuntimeContext";

/** 启用声纹识别卡：写入 [speaker].enabled；运行中的语音会话由后端自动重启以生效。 */
export function SpeakerEnableCard() {
  const { speaker } = useRuntime();
  const enabled = speaker.config.config?.enabled ?? false;

  return (
    <section className="flex items-center justify-between gap-3 rounded-[16px] border border-panel-border bg-panel-background px-5 py-4">
      <div className="flex items-start gap-2.5">
        <Fingerprint className="mt-0.5 h-4 w-4 shrink-0 text-text-secondary" />
        <div>
          <p className="text-sm font-medium text-text-primary">启用声纹识别</p>
          <p className="mt-0.5 text-xs text-text-muted">
            启用后仅响应已注册说话人：声纹不匹配的语音将被忽略（不回复、不入
            历史、不存记录）；短于最短时长的语音无法确认身份，同样会被忽略。
            欢迎语对所有人播放；尚未注册声纹时按未启用处理。仅用于区分说话人， 不构成安全认证。
          </p>
        </div>
      </div>
      <Switch
        checked={enabled}
        onCheckedChange={(v) => void speaker.config.setEnabled(v)}
        aria-label="启用声纹识别"
        trackClass="bg-emerald-500"
      />
    </section>
  );
}
