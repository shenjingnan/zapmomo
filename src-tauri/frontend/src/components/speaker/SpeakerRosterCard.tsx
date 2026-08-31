import { CircleAlert, ScanSearch, Trash2, UserRoundPlus } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { useToast } from "@/components/ui/toast";
import { useRuntime } from "@/providers/RuntimeContext";

interface SpeakerRosterCardProps {
  onEnrollOpen: () => void;
  onTestOpen: () => void;
}

/** 说话人管理卡：已注册列表（样本数/更新时间/删除）+ 添加与测试入口。 */
export function SpeakerRosterCard({ onEnrollOpen, onTestOpen }: SpeakerRosterCardProps) {
  const { speaker } = useRuntime();
  const toast = useToast();
  const { speakers, error, busy } = speaker.speakers;

  const handleRemove = async (speakerId: string) => {
    try {
      const deleted = await speaker.speakers.remove(speakerId);
      toast.success(deleted ? `已移除 ${speakerId}` : `${speakerId} 未注册`);
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <section className="overflow-hidden rounded-[16px] border border-panel-border bg-panel-background">
      <div className="px-3.5 py-2.5">
        <div className="flex items-center gap-2.5">
          <UserRoundPlus className="h-4 w-4 shrink-0 text-text-secondary" />
          <div>
            <h2 className="text-base font-semibold text-text-primary">说话人</h2>
            <p className="mt-0.5 text-xs text-text-muted">
              已注册说话人保存在本机（~/.zapmomo/speaker_profiles），删除后不可恢复
            </p>
          </div>
        </div>
      </div>

      {error && (
        <div className="px-3.5 pb-2">
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">{error}</AlertDescription>
          </Alert>
        </div>
      )}

      {speakers.length === 0 ? (
        <p className="px-3.5 pb-3 text-sm text-text-muted">
          尚未注册任何说话人。点击「添加说话人」录制几段语音完成注册。
        </p>
      ) : (
        <ul className="divide-y divide-divider border-t border-divider">
          {speakers.map((s) => (
            <li
              key={s.speaker_id}
              className="flex items-center justify-between gap-3 px-3.5 py-2.5"
            >
              <div className="min-w-0">
                <p className="text-sm font-medium text-text-primary">{s.speaker_id}</p>
                <p className="truncate text-xs text-text-muted">
                  {s.sample_count} 段样本 · dim {s.dim} · {s.updated_at}
                </p>
              </div>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8 shrink-0 text-text-muted hover:bg-destructive hover:text-destructive-foreground"
                onClick={() => void handleRemove(s.speaker_id)}
                disabled={busy}
                aria-label={`删除说话人 ${s.speaker_id}`}
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </li>
          ))}
        </ul>
      )}

      <div className="flex items-center justify-end gap-2 border-t border-divider px-3.5 py-2.5">
        <Button size="sm" variant="outline" onClick={onTestOpen}>
          <ScanSearch className="h-4 w-4" />
          识别测试
        </Button>
        <Button size="sm" onClick={onEnrollOpen}>
          <UserRoundPlus className="h-4 w-4" />
          添加说话人
        </Button>
      </div>
    </section>
  );
}
