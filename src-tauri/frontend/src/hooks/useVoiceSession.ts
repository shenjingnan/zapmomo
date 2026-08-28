import { useCallback, useEffect, useState } from "react";
import {
  api,
  onVoiceSessionError,
  onVoiceSessionPlay,
  onVoiceSessionReply,
  onVoiceSessionReplyFinished,
  onVoiceSessionState,
  onVoiceSessionStopped,
  onVoiceSessionToken,
  onVoiceSessionTranscript,
  onVoiceSessionWake,
} from "@/lib/tauri";
import type { ConversationRecord, VoiceSessionPhase } from "@/types/tauri";

/** 对话记录列表最大条数（与后端 `records.rs` 的 cap 一致）。 */
const MAX_RECORDS = 200;

/** 语音会话运行态（订阅后端 `voice-session-*` 事件）。 */
export interface VoiceSessionState {
  running: boolean;
  phase: VoiceSessionPhase;
  /** ASR 实时字幕（部分结果） */
  partial: string;
  /** 已持久化的对话记录（时间正序：旧的在上、新的在下） */
  records: ConversationRecord[];
  /** 当前轮 LLM 流式回复（`reply-finished` 时提交为 assistant 记录） */
  pendingReply: string;
  /** 当前轮用户句（transcript is_final 置位；气泡「先用户句后回复」用） */
  turnUserText: string;
  replyDone: boolean;
  /** 已入队合成的句子 */
  queuedSentences: string[];
  /** 正在播报的句子 */
  currentSentence: string | null;
  error: string | null;
  /** start/stop 在途标志 */
  pending: boolean;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  clearRecords: () => Promise<void>;
}

/**
 * 语音会话状态管理：初始化回读后端运行态 + 载入持久化对话记录，订阅 `voice-session-*`
 * 事件驱动。桌宠窗口（无 RuntimeContext）与设置窗口共用。
 *
 * 记录流：用户最终句（`transcript` is_final）与桌宠完整回复（`reply-finished` 携带 text）
 * 均提交进 `records`；后端在事件转发层同步落盘，前端仅做展示与本地累积。
 * transcript is_final 同时置位 turnUserText 供气泡展示当前轮用户句。
 */
export function useVoiceSession(): VoiceSessionState {
  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<VoiceSessionPhase>("idle");
  const [partial, setPartial] = useState("");
  const [records, setRecords] = useState<ConversationRecord[]>([]);
  const [pendingReply, setPendingReply] = useState("");
  const [turnUserText, setTurnUserText] = useState("");
  const [replyDone, setReplyDone] = useState(false);
  const [queuedSentences, setQueuedSentences] = useState<string[]>([]);
  const [currentSentence, setCurrentSentence] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  useEffect(() => {
    // 启动时回读后端状态（应用可能在 setup 已自动进入待唤醒）
    api
      .isVoiceSessionRunning()
      .then(setRunning)
      .catch(() => {});
    // 载入持久化的对话记录（跨应用重启保留）
    api
      .getConversationRecords()
      .then((records) => setRecords((records ?? []).slice(-MAX_RECORDS)))
      .catch(() => {});

    const unsubs = [
      onVoiceSessionState((p) => {
        setRunning(p.running);
        setPhase(p.state);
        // 打断 / 停止都会回到 Armed 或 Idle（打断不 emit reply-finished，流式缓冲
        // 会被丢弃），这里清掉被打断的回复，避免残留展示
        if (p.state === "armed" || p.state === "idle") {
          setPendingReply("");
          setCurrentSentence(null);
        }
      }),
      onVoiceSessionWake(() => {}),
      onVoiceSessionTranscript((p) => {
        if (p.is_final) {
          setRecords((prev) =>
            [...prev, { role: "user" as const, text: p.text, at: new Date().toISOString() }].slice(
              -MAX_RECORDS,
            ),
          );
          setPartial("");
          setTurnUserText(p.text);
        } else {
          setPartial(p.text);
        }
      }),
      onVoiceSessionToken((p) => setPendingReply((prev) => prev + p.delta)),
      onVoiceSessionReply((p) => setQueuedSentences((prev) => [...prev, p.sentence])),
      onVoiceSessionPlay((p) => setCurrentSentence(p.sentence)),
      onVoiceSessionReplyFinished((p) => {
        // 用后端权威文本提交桌宠记录（空回复 text 为 null，不落空行）
        if (p.text && p.text.trim().length > 0) {
          const text = p.text;
          setRecords((prev) =>
            [...prev, { role: "assistant" as const, text, at: new Date().toISOString() }].slice(
              -MAX_RECORDS,
            ),
          );
        }
        setPendingReply("");
        setReplyDone(true);
      }),
      onVoiceSessionError((p) => setError(p.message)),
      onVoiceSessionStopped((p) => {
        setRunning(false);
        setPhase("idle");
        if (p.error) setError(p.error);
      }),
    ];

    return () => {
      unsubs.forEach((p) => {
        void p.then((fn) => fn());
      });
    };
  }, []);

  const start = useCallback(async () => {
    setPending(true);
    setError(null);
    // 新一轮开始前清空上一轮的流式展示（记录保留，跨轮持续累积）
    setPendingReply("");
    setReplyDone(false);
    setQueuedSentences([]);
    setCurrentSentence(null);
    setPartial("");
    try {
      await api.startVoiceSession();
      setRunning(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  }, []);

  const stop = useCallback(async () => {
    setPending(true);
    try {
      await api.stopVoiceSession();
      setRunning(false);
      setPhase("idle");
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  }, []);

  const clearRecords = useCallback(async () => {
    try {
      await api.clearConversationRecords();
      setRecords([]);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  return {
    running,
    phase,
    partial,
    records,
    pendingReply,
    turnUserText,
    replyDone,
    queuedSentences,
    currentSentence,
    error,
    pending,
    start,
    stop,
    clearRecords,
  };
}
