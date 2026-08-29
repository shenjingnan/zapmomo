# 第三方模型声明

本目录下的模型文件由第三方提供，**不随代码分发**，由 `scripts/download-kws-model.sh`
按 `manifest.json` 清单按需下载。清单记录了来源 URL 与 sha256 校验和。

## sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20

- **用途**: 中英混合关键词唤醒词检测（KWS）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20.tar.bz2
- **发布方**: k2-fsa（sherpa-onnx 项目）
- **许可证**: Apache-2.0（依据 sherpa-onnx 项目整体许可；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `68447f4fbc67e70eee3a93961f36e81e98f47aef73ce7e7ca00885c6cd3616a6`

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

## silero_vad.onnx（离线听写 VAD）

- **用途**: 离线免提听写的语音活动检测（VAD，说/静音分段）
- **来源**: https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx
- **发布方**: k2-fsa（sherpa-onnx 项目；模型源自 Silero Team）
- **许可证**: MIT（Silero VAD；如需商用请以官方模型发布页的许可说明为准）
- **sha256**: `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`

## omnivoice-q8_0.gguf（audio.cpp TTS）

- **用途**: 多语种文本转语音 + 零样本声音克隆（TTS，OmniVoice，Qwen3-0.6B 基座，
  q8_0 量化，24kHz，generator 与 audio tokenizer 双权重内嵌单文件），由内置的
  audio.cpp 引擎（sidecar 进程，Metal 后端）驱动
- **来源**: https://huggingface.co/audio-cpp/audio.cpp-gguf/tree/main/OmniVoice-GGUF
- **发布方**: audio-cpp（audio.cpp 官方 GGUF 仓库；模型源自 k2-fsa/OmniVoice）
- **许可证**: Apache-2.0（依据 audio.cpp 仓库标注；如需商用请以模型发布页的许可说明为准）
- **sha256**: `2f4be637278043c6842de5b85d681532030e9eb6ffe0f8b0e320f68238e3da8b`

## voxcpm2-q8_0.gguf（audio.cpp TTS）

- **用途**: 多语种文本转语音 + 零样本声音克隆（TTS，VoxCPM2，OpenBMB MiniCPM-4 2B
  基座，q8_0 量化，48kHz 录音室级输出，AudioVAE V2 内置超分），由内置的
  audio.cpp 引擎（sidecar 进程，Metal 后端）驱动
- **来源**: https://huggingface.co/audio-cpp/audio.cpp-gguf/tree/main/VoxCPM2-GGUF
- **发布方**: audio-cpp（audio.cpp 官方 GGUF 仓库；模型源自 OpenBMB/VoxCPM2）
- **许可证**: Apache-2.0（依据 audio.cpp 仓库标注；如需商用请以模型发布页的许可说明为准）
- **sha256**: `c8e01ab4416011e12a28f24ede298a1aa5ce64b43f8e8aaad53b1e2fe7c96432`

## qwen3-tts-12hz-0.6b-base-q8_0.gguf / qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf（audio.cpp TTS）

- **组件**: Qwen3-TTS 12Hz Base（0.6B / 1.7B）q8_0 量化 GGUF（audio.cpp 打包，sidecar 内嵌全部配置文件）
- **来源**: https://huggingface.co/audio-cpp/audio.cpp-gguf（Qwen3-TTS-12Hz-0.6B-Base-GGUF / Qwen3-TTS-12Hz-1.7B-Base-GGUF）
- **上游模型**: https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-Base / https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-Base
- **许可证**: Apache-2.0
- **用途**: TTS 音色克隆（speaker reference）推理，经 audio.cpp sidecar 后端加载
- **sha256**: 0.6B `771420bd20ff5f35407b4fa9cf9c5461e153800d3d772ef51c9febc0a520855d` /
  1.7B `b55e06c7890d43c208d15aed8b4ed3f18215f295e47d5960e061b15bff338ab0`

## audiocpp_server（audio.cpp 引擎二进制，随安装包分发）

- **用途**: TTS 第二推理后端（ggml 系 audio.cpp 的 HTTP server sidecar，裁剪构建
  仅含 omnivoice + voxcpm2 + qwen3_tts 模型族；编译参数见 `.github/workflows/release.yml`）
- **来源**: https://github.com/0xShug0/audio.cpp（版本 pin 见 release.yml 的 AUDIOCPP_REF）
- **发布方**: ShugoAI LLC（audio.cpp 项目）
- **许可证**: Apache-2.0。随 ZapMomo（GPL-3.0-only）以独立进程形式聚合分发，
  Apache-2.0 与 GPL-3.0 兼容；本文件即其 NOTICE 性质的声明
