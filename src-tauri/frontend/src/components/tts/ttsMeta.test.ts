import { describe, expect, it } from "vitest";
import { groupKokoroVoices, isCloneRequiredTtsKind, isCloneTtsKind, ttsModelKindLabel } from "./ttsMeta";
import { TTS_PRESETS } from "@/hooks/useTtsModelSwitch";
import type { TtsVoice } from "@/types/tauri";

function kv(id: string, group: TtsVoice["group"], sid: number | null): TtsVoice {
  return { id, name: id, wav_path: "", reference_text: "", custom: false, sid, group };
}

describe("ttsModelKindLabel", () => {
  it("kokoro 有专属标签", () => {
    expect(ttsModelKindLabel("kokoro")).toBe("Kokoro");
    expect(ttsModelKindLabel("zipvoice")).toBe("ZipVoice 克隆");
    expect(ttsModelKindLabel("unknown")).toBe("TTS");
  });

  it("omnivoice 有专属标签（克隆族）", () => {
    expect(ttsModelKindLabel("omnivoice")).toBe("OmniVoice 克隆");
    expect(ttsModelKindLabel("voxcpm2")).toBe("VoxCPM2 克隆");
    expect(ttsModelKindLabel("pocket")).toBe("PocketTTS");
  });

  it("qwen3_tts 两尺寸均为 Qwen3-TTS 克隆标签", () => {
    expect(ttsModelKindLabel("qwen3_tts_06")).toBe("Qwen3-TTS 克隆");
    expect(ttsModelKindLabel("qwen3_tts_17")).toBe("Qwen3-TTS 克隆");
  });
});

describe("isCloneTtsKind / isCloneRequiredTtsKind", () => {
  it("克隆族：zipvoice/omnivoice/voxcpm2/qwen3_tts 两尺寸", () => {
    for (const kind of ["zipvoice", "omnivoice", "voxcpm2", "qwen3_tts_06", "qwen3_tts_17"]) {
      expect(isCloneTtsKind(kind), kind).toBe(true);
    }
    for (const kind of ["kokoro", "vits", "matcha", "pocket", ""]) {
      expect(isCloneTtsKind(kind), kind).toBe(false);
    }
  });

  it("强制克隆族仅 qwen3_tts 两尺寸（上游 Base 无 auto voice 兜底）", () => {
    expect(isCloneRequiredTtsKind("qwen3_tts_06")).toBe(true);
    expect(isCloneRequiredTtsKind("qwen3_tts_17")).toBe(true);
    expect(isCloneRequiredTtsKind("zipvoice")).toBe(false);
    expect(isCloneRequiredTtsKind("omnivoice")).toBe(false);
    expect(isCloneRequiredTtsKind("voxcpm2")).toBe(false);
  });
});

describe("TTS_PRESETS", () => {
  it("含 omnivoice 条目且 id 与后端 registry 一致", () => {
    const omni = TTS_PRESETS.find((p) => p.id === "tts-omnivoice-q8-audiocpp");
    expect(omni).toBeDefined();
    expect(omni?.kind).toBe("omnivoice");
  });

  it("含 voxcpm2 条目且 id 与后端 registry 一致", () => {
    const vox = TTS_PRESETS.find((p) => p.id === "tts-voxcpm2-q8-audiocpp");
    expect(vox).toBeDefined();
    expect(vox?.kind).toBe("voxcpm2");
  });

  it("含 qwen3-tts 两尺寸条目且 id 与后端 registry 一致", () => {
    const q06 = TTS_PRESETS.find((p) => p.id === "tts-qwen3-06b-base-q8-audiocpp");
    expect(q06).toBeDefined();
    expect(q06?.kind).toBe("qwen3_tts_06");

    const q17 = TTS_PRESETS.find((p) => p.id === "tts-qwen3-17b-base-q8-audiocpp");
    expect(q17).toBeDefined();
    expect(q17?.kind).toBe("qwen3_tts_17");
  });

  it("id 唯一（防预设重复注册）", () => {
    const ids = TTS_PRESETS.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("groupKokoroVoices", () => {
  it("按语言分组且中文优先（女声 → 男声 → 英文）", () => {
    const voices = [
      kv("af_maple", "english_female", 0),
      kv("zm_009", "chinese_male", 58),
      kv("zf_001", "chinese_female", 3),
      kv("zf_002", "chinese_female", 4),
    ];
    const groups = groupKokoroVoices(voices);
    expect(groups.map((g) => g.group)).toEqual([
      "chinese_female",
      "chinese_male",
      "english_female",
    ]);
    expect(groups[0].label).toBe("中文女声");
    expect(groups[0].items.map((v) => v.id)).toEqual(["zf_001", "zf_002"]);
    expect(groups[1].items).toHaveLength(1);
    expect(groups[2].items).toHaveLength(1);
  });

  it("空分组被过滤；无 group 的音色（zipvoice 混入）不归入任何组", () => {
    const voices = [kv("zf_001", "chinese_female", 3), kv("leijun-1", null, null)];
    const groups = groupKokoroVoices(voices);
    expect(groups).toHaveLength(1);
    expect(groups[0].items).toHaveLength(1);
  });

  it("空列表返回空数组", () => {
    expect(groupKokoroVoices([])).toEqual([]);
  });
});
