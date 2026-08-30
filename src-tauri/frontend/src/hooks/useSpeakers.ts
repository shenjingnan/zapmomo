import { useCallback, useEffect, useState } from "react";
import { api } from "@/lib/tauri";
import type { SpeakerEnrollResult, SpeakerIdentifyResult, SpeakerInfo } from "@/types/tauri";

export interface SpeakersState {
  speakers: SpeakerInfo[];
  error: string | null;
  /** 正在录制样本（进入 anyListening，其它麦克风消费者自动禁用） */
  recording: boolean;
  /** 注册/识别请求进行中 */
  busy: boolean;
  refresh: () => Promise<void>;
  /** 录制一段样本，返回 wav 路径（错误上抛由调用方 toast） */
  recordSample: (seconds: number, device: string | null) => Promise<string>;
  /** 注册说话人（wavPaths 为录音临时文件或自选 wav；错误上抛） */
  enroll: (speakerId: string, wavPaths: string[]) => Promise<SpeakerEnrollResult>;
  /** 删除说话人档案；返回是否确实删除 */
  remove: (speakerId: string) => Promise<boolean>;
  /** 对一段 wav 做声纹识别（1:N 测试；错误上抛） */
  identifyWav: (wavPath: string) => Promise<SpeakerIdentifyResult>;
}

/** 已注册说话人列表 + 录音注册/识别操作。 */
export function useSpeakers(): SpeakersState {
  const [speakers, setSpeakers] = useState<SpeakerInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [recording, setRecording] = useState(false);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setSpeakers((await api.listSpeakers()) ?? []);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const recordSample = useCallback(async (seconds: number, device: string | null) => {
    setRecording(true);
    try {
      return await api.recordSpeakerSample({ seconds, device });
    } finally {
      setRecording(false);
    }
  }, []);

  const enroll = useCallback(
    async (speakerId: string, wavPaths: string[]) => {
      setBusy(true);
      try {
        const result = await api.speakerEnroll({ speakerId, wavPaths });
        await refresh();
        return result;
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const remove = useCallback(
    async (speakerId: string) => {
      const deleted = await api.removeSpeaker({ speakerId });
      await refresh();
      return deleted;
    },
    [refresh],
  );

  const identifyWav = useCallback(async (wavPath: string) => {
    setBusy(true);
    try {
      return await api.speakerIdentifyWav({ wavPath });
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { speakers, error, recording, busy, refresh, recordSample, enroll, remove, identifyWav };
}
