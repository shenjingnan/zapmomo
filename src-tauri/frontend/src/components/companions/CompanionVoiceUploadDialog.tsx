/**
 * 伙伴音色上传对话框：选参考音频 → ASR 自动转写（可编辑）→ 覆盖当前生效音色。
 *
 * 流程模板对齐 TtsVoicesDialog 的添加表单；错误在本对话框内 Alert 展示
 * （toast 留给 hook 层的成功反馈）。转写为克隆必需，空文本禁止提交。
 */

import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AudioLines, CircleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { ModelDialog } from "@/components/models/ModelDialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/tauri";
import type { CompanionModelInfo } from "@/types/tauri";

interface CompanionVoiceUploadDialogProps {
  open: boolean;
  /** 目标伙伴（null = 不渲染内容）。 */
  companion: CompanionModelInfo | null;
  onClose: () => void;
  /** 提交（hook 的 uploadVoice）；resolve true 才关闭对话框。 */
  onSubmit: (wavPath: string, referenceText: string) => Promise<boolean>;
}

export function CompanionVoiceUploadDialog({
  open,
  companion,
  onClose,
  onSubmit,
}: CompanionVoiceUploadDialogProps) {
  const [wavPath, setWavPath] = useState<string | null>(null);
  const [referenceText, setReferenceText] = useState("");
  const [transcribing, setTranscribing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 打开时重置表单。
  useEffect(() => {
    if (open) {
      setWavPath(null);
      setReferenceText("");
      setTranscribing(false);
      setSaving(false);
      setError(null);
    }
  }, [open]);

  const pickWav = async () => {
    const file = await openDialog({
      multiple: false,
      title: "选择参考音频",
      filters: [{ name: "WAV", extensions: ["wav"] }],
    });
    if (typeof file === "string") {
      setWavPath(file);
      setReferenceText("");
      setError(null);
    }
  };

  const handleTranscribe = async () => {
    if (!wavPath) return;
    setTranscribing(true);
    setError(null);
    try {
      const text = await api.transcribeReferenceAudio({ wavPath });
      setReferenceText(text);
    } catch (e) {
      setError(`自动转写失败：${String(e)}。可手动填写与音频一致的逐字转写。`);
    } finally {
      setTranscribing(false);
    }
  };

  const canSubmit = wavPath !== null && referenceText.trim() !== "" && !transcribing && !saving;

  const handleSubmit = async () => {
    if (!companion || !wavPath) return;
    setSaving(true);
    setError(null);
    const ok = await onSubmit(wavPath, referenceText.trim());
    setSaving(false);
    if (ok) onClose();
  };

  return (
    <ModelDialog open={open} onClose={onClose} title="上传伙伴音色" width="md">
      <div className="space-y-3">
        <p className="text-xs text-muted-foreground">
          将覆盖「{companion?.name ?? ""}」当前生效的音色；作者原版会自动备份，可随时恢复。
          分享角色包时会带上当前生效的音色。
        </p>

        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" onClick={() => void pickWav()}>
            {wavPath ? "重新选择" : "选择 wav 文件"}
          </Button>
          {wavPath && (
            <p
              className="min-w-0 flex-1 truncate font-mono text-xs text-text-muted"
              title={wavPath}
            >
              {wavPath}
            </p>
          )}
        </div>

        <div className="space-y-1.5">
          <div className="flex items-center justify-between gap-2">
            <label className="text-sm text-text-primary" htmlFor="companion-voice-ref">
              参考文本
            </label>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void handleTranscribe()}
              disabled={!wavPath || transcribing}
            >
              <AudioLines className="h-4 w-4" />
              {transcribing ? "转写中…" : "自动转写"}
            </Button>
          </div>
          <textarea
            id="companion-voice-ref"
            className="w-full rounded-md border border-input bg-background p-2 text-sm text-text-primary outline-none focus:ring-1 focus:ring-ring"
            rows={3}
            value={referenceText}
            onChange={(e) => setReferenceText(e.target.value)}
            placeholder="参考音频的逐字转写文本（可点「自动转写」或手动填写，须与音频一致）"
          />
        </div>

        {error && (
          <Alert variant="destructive">
            <CircleAlert className="h-4 w-4" />
            <AlertDescription className="whitespace-pre-wrap">{error}</AlertDescription>
          </Alert>
        )}
      </div>

      <div className="mt-3 flex items-center justify-end gap-2 border-t border-divider pt-3">
        <Button variant="ghost" size="sm" onClick={onClose} disabled={saving}>
          取消
        </Button>
        <Button size="sm" onClick={() => void handleSubmit()} disabled={!canSubmit}>
          {saving ? "保存中…" : "保存并覆盖"}
        </Button>
      </div>
    </ModelDialog>
  );
}
