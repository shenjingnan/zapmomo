import { describe, expect, it } from "vitest";
import { TTS_PRESETS } from "@/hooks/useTtsModelSwitch";
import { type PresetWithPlatforms, visiblePresets } from "@/lib/modelPlatforms";
import { isCloneRequiredTtsKind, isCloneTtsKind, ttsModelKindLabel } from "./ttsMeta";

describe("ttsModelKindLabel", () => {
  it("zipvoice 有专属标签", () => {
    expect(ttsModelKindLabel("zipvoice")).toBe("ZipVoice 克隆");
    expect(ttsModelKindLabel("unknown")).toBe("TTS");
  });

  it("omnivoice/voxcpm2 有专属标签（克隆族）", () => {
    expect(ttsModelKindLabel("omnivoice")).toBe("OmniVoice 克隆");
    expect(ttsModelKindLabel("voxcpm2")).toBe("VoxCPM2 克隆");
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
    // 已移除的 vits/matcha/kokoro/pocket 及未知值均非克隆族
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

  it("已移除的模型不再出现在预设中", () => {
    const removed = [
      "tts-vits-melo-zh-en",
      "tts-matcha-zh-baker",
      "tts-kokoro-int8-multi-lang-v1-1",
      "tts-kokoro-multi-lang-v1-1",
      "tts-pocket-english-audiocpp",
    ];
    for (const id of removed) {
      expect(
        TTS_PRESETS.find((p) => p.id === id),
        id,
      ).toBeUndefined();
    }
  });

  it("id 唯一（防预设重复注册）", () => {
    const ids = TTS_PRESETS.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("audiocpp 预设 platforms 与 registry 解锁范围一致（darwin-aarch64 + windows-x86_64）", () => {
    const audiocppIds = [
      "tts-omnivoice-q8-audiocpp",
      "tts-voxcpm2-q8-audiocpp",
      "tts-qwen3-06b-base-q8-audiocpp",
      "tts-qwen3-17b-base-q8-audiocpp",
    ];
    for (const id of audiocppIds) {
      // 标注为 PresetWithPlatforms：联合类型里 zipvoice 成员无 platforms 字段，
      // 直接访问 p?.platforms 会因属性不全而编译失败
      const p: PresetWithPlatforms | undefined = TTS_PRESETS.find((x) => x.id === id);
      expect(p, id).toBeDefined();
      expect(p?.platforms, id).toEqual(["darwin-aarch64", "windows-x86_64"]);
    }
    // sherpa 族无平台约束
    const zipvoice: PresetWithPlatforms | undefined = TTS_PRESETS.find(
      (p) => p.id === "tts-zipvoice-distill-int8",
    );
    expect(zipvoice?.platforms).toBeUndefined();
  });
});

describe("visiblePresets", () => {
  const presets = [
    { id: "free", name: "free" },
    { id: "gpu", name: "gpu", platforms: ["darwin-aarch64", "windows-x86_64"] as const },
  ];

  it("后端列表未加载（null）→ 空数组（防闪现不可用预设）", () => {
    expect(visiblePresets(presets, null)).toEqual([]);
  });

  it("后端列表不含平台受限 id → 仅全平台预设可见", () => {
    const backend = [{ id: "tts-zipvoice-distill-int8" }];
    const visible = visiblePresets(presets, backend);
    expect(visible.map((p) => p.id)).toEqual(["free"]);
  });

  it("后端列表含平台受限 id → 该预设可见", () => {
    const backend = [{ id: "free" }, { id: "gpu" }];
    expect(visiblePresets(presets, backend).map((p) => p.id)).toEqual(["free", "gpu"]);
  });
});
