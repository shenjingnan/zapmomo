# 第三方模型声明

本目录下的模型文件由第三方提供，**不随代码分发**，由 `scripts/download-kws-model.sh`
按 `manifest.json` 清单按需下载。清单记录了来源 URL 与 sha256 校验和。

## sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20

- **用途**: 中英混合关键词唤醒词检测（KWS）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目）
- **许可证**: Apache-2.0（依据 sherpa-onnx 项目整体许可；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `68447f4fbc67e70eee3a93961f36e81e98f47aef73ce7e7ca00885c6cd3616a6`

## sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01

- **用途**: 纯英文关键词唤醒词检测（KWS）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目，模型由 pkufool 训练，GigaSpeech XL 1 万小时）
- **许可证**: Apache-2.0（依据 sherpa-onnx 项目整体许可；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `f170013b4716e41b62b9bfd809687c207cef798ef9bc6534d524e17af9b6561a`
- **测试夹具**: `src/kws/testdata/bpe.model` 取自该模型包内的同名单文件（239KB），
  用于钉住子词切分行为，随代码分发，许可同上

## sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20

- **用途**: 中英双语流式语音识别（ASR）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目，模型由社区贡献）
- **许可证**: Apache-2.0（依据 sherpa-onnx 项目整体许可；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `27ffbd9ee24ad186d99acc2f6354d7992b27bcab490812510665fa8f9389c5f8`

## sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12

- **用途**: 中英双语标点恢复（ASR 结果自动加标点）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/punctuation-models/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目，源自阿里 DAMO Academy 的 CT-Transformer 标点模型）
- **许可证**: Apache-2.0（依据 sherpa-onnx 项目整体许可；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `50f73f8cccffc2303999fda28b785ffcffbd7ea442c47385c30b9d045ee6afc3`

## sherpa-onnx-zipvoice-distill-int8-zh-en-emilia

- **用途**: 中英双语文本转语音（TTS，ZipVoice 零样本声音克隆）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目）
- **许可证**: Apache-2.0（依据 sherpa-onnx 项目整体许可；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `77219c8b40f4ee8d73a7f902305ff6c1128ef9b54461c41b4ca6ed890b6c2803`

## vocos_24khz.onnx（TTS 声码器）

- **用途**: ZipVoice TTS 的声码器（vocoder，把 mel 谱转成波形）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/vocoder-models/vocos_24khz.onnx
- **发布方**: k2-fsa（sherpa-onnx 项目）
- **许可证**: Apache-2.0（依据 sherpa-onnx 项目整体许可；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `bcb3b970e384161c4d634f0bb9e999ff1c471b34c9bc0b1049a5014065ed3cc0`

## kokoro-int8-multi-lang-v1_1

- **用途**: 中英双语文本转语音（TTS，Kokoro 103 音色，int8 量化）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_1.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自 hexgrad/Kokoro-82M-v1.1-zh）
- **许可证**: Apache-2.0（如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `a1e94694776049035c4f2c6529f003aaece993c76aae9a78995831c3c4dcafc6`

## kokoro-multi-lang-v1_1

- **用途**: 中英双语文本转语音（TTS，Kokoro 103 音色，fp32 未量化）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自 hexgrad/Kokoro-82M-v1.1-zh）
- **许可证**: Apache-2.0（如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `a3f4c73d043860e3fd2e5b06f36795eb81de0fc8e8de6df703245edddd87dbad`

## sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17

- **用途**: 离线多语言语音识别（ASR，zh/en/ja/ko/yue，含情绪/事件标签；int8 轻量版）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自阿里 ModelScope iic/SenseVoiceSmall）
- **许可证**: FunASR Model License v1.1（阿里）；如需商用请以官方模型发布页的许可说明为准
- **sha256**: `7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e`

## sherpa-onnx-whisper-tiny

- **用途**: 离线多语言语音识别（ASR，OpenAI Whisper tiny）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-tiny.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自 OpenAI Whisper）
- **许可证**: MIT（OpenAI Whisper；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `c46116994e539aa165266d96b325252728429c12535eb9d8b6a2b10f129e66b1`

## sherpa-onnx-streaming-paraformer-bilingual-zh-en

- **用途**: 流式语音识别（ASR，中英双语；包内含 fp32+int8 双份，运行时默认加载 int8）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-paraformer-bilingual-zh-en.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自阿里 FunASR/ModelScope paraformer-large）
- **许可证**: FunASR Model License v1.1（阿里）；如需商用请以官方模型发布页的许可说明为准
- **sha256**: `5462a1fce42693deae572af1e8c4687124b12aa85fe61ff4d3168bb5280e205f`

## sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en

- **用途**: 流式语音识别（ASR，普通话/粤语/英语三语；包内含 fp32+int8 双份，运行时默认加载 int8）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自阿里 FunASR/ModelScope paraformer-large）
- **许可证**: FunASR Model License v1.1（阿里）；如需商用请以官方模型发布页的许可说明为准
- **sha256**: `d479167d8752628d9032d29de1060493865389d1e295a1c2e8e011e7062f1932`

## sherpa-onnx-whisper-base

- **用途**: 离线多语言语音识别（ASR，OpenAI Whisper base）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-base.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自 OpenAI Whisper）
- **许可证**: MIT（OpenAI Whisper；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `911b2083efd7c0dca2ac3b358b75222660dc09fb716d64fbfc417ba6c99ff3de`

## sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25

- **用途**: 离线多语言语音识别（ASR，29 语言 + 中文方言自动识别，LLM 自回归解码，支持热词；int8 轻量版）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自阿里 QwenLM/Qwen3-ASR）
- **许可证**: Apache-2.0（Qwen3-ASR；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `393f8a14e2f5fb96746aaab342997a40641001fbd5bf9592a080a8329178ee96`

## silero_vad.onnx（离线听写 VAD）

- **用途**: 离线免提听写的语音活动检测（VAD，说/静音分段）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自 Silero Team）
- **许可证**: MIT（Silero VAD；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`

## pocket-tts-english-q8_0.gguf + embeddings/alba.safetensors（audio.cpp TTS）

- **用途**: 英文文本转语音（TTS，PocketTTS 100M，q8_0 量化，固定音色 alba），
  由内置的 audio.cpp 引擎（sidecar 进程）驱动
- **来源**: https://huggingface.co/audio-cpp/audio.cpp-gguf/tree/main/PocketTTS-GGUF/english
- **发布方**: audio-cpp（audio.cpp 官方 GGUF 仓库；模型源自 Kyutai PocketTTS）
- **许可证**: Apache-2.0（依据 audio.cpp 仓库标注；如需商用请以模型发布页的许可说明为准）
- **sha256**: gguf `0315406421d515d9ffbde49ed998832ff2962562ef8abde440c85fa0a27d8b2a` /
  embeddings `69c32db63ca56843d994f81f343f62e0bf2d73f7e4c9bc73e44bb1110b1d8845`

## omnivoice-q8_0.gguf（audio.cpp TTS）

- **用途**: 多语种文本转语音 + 零样本声音克隆（TTS，OmniVoice，Qwen3-0.6B 基座，
  q8_0 量化，24kHz，generator 与 audio tokenizer 双权重内嵌单文件），由内置的
  audio.cpp 引擎（sidecar 进程，Metal 后端）驱动
- **来源**: https://huggingface.co/audio-cpp/audio.cpp-gguf/tree/main/OmniVoice-GGUF
- **发布方**: audio-cpp（audio.cpp 官方 GGUF 仓库；模型源自 k2-fsa/OmniVoice）
- **许可证**: Apache-2.0（依据 audio.cpp 仓库标注；如需商用请以模型发布页的许可说明为准）
- **sha256**: `2f4be637278043c6842de5b85d681532030e9eb6ffe0f8b0e320f68238e3da8b`

## audiocpp_server（audio.cpp 引擎二进制，随安装包分发）

- **用途**: TTS 第二推理后端（ggml 系 audio.cpp 的 HTTP server sidecar，裁剪构建
  仅含 pocket_tts + omnivoice 模型族；编译参数见 `.github/workflows/release.yml`）
- **来源**: https://github.com/0xShug0/audio.cpp（版本 pin 见 release.yml 的 AUDIOCPP_REF）
- **发布方**: ShugoAI LLC（audio.cpp 项目）
- **许可证**: Apache-2.0。随 ZapMomo（GPL-3.0-only）以独立进程形式聚合分发，
  Apache-2.0 与 GPL-3.0 兼容；本文件即其 NOTICE 性质的声明
