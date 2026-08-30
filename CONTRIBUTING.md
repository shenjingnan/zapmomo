# 参与开发（Contributing）

感谢关注 ZapMomo！本文档面向贡献者与开发者，覆盖本地环境搭建、CLI 命令参考、测试与发布流程。

> 终端用户文档见 [README](README.md) 与[文档站](docs/)。

## 环境准备

| 工具 | 版本 | 用途 |
| ---- | ---- | ---- |
| Rust | 1.97（`rust-toolchain.toml` 固定） | 编译 / 测试 / Lint |
| pnpm | 任意现代版本 | Tauri 前端依赖管理与 CLI |
| 平台依赖 | — | Linux 构建 Tauri 需 webkit2gtk 等系统库；Windows / macOS 开箱即用 |

常用命令速查（完整说明见下文各节）：

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test   # 完整检查
```

## 快速开始（CLI）

```bash
# 运行
cargo run
cargo run -- config
cargo run -- greet --name World
cargo run -- completion bash        # 生成 shell 补全

# 测试
cargo test
cargo test -- --test-threads=1      # 单线程测试（避免 env 竞争）

# 代码质量检查
cargo fmt --check
cargo clippy -- -D warnings

# 覆盖率
cargo tarpaulin
```

## 模型来源与校验

模型**不随代码分发**，由 CLI 内置的 `install-model` 命令（或 `scripts/download-kws-model.sh`）按 `models/manifest.json` 清单下载：

- **清单** `models/manifest.json`（随仓库）记录每个模型的 `name / version / source / sha256 / license`
- **校验**：下载后对整包计算 sha256 与清单比对，**不匹配即删除报错**；解压先到临时目录再原子移动，避免留下损坏的半截模型
- **幂等**：模型已存在且完整则跳过
- **合规**：第三方来源与许可见 `models/THIRD_PARTY_NOTICES.md`
- KWS / ASR / TTS 沿用同一套清单机制（见下文各节）

## CLI 命令参考

### 关键词唤醒词（KWS）

接入 sherpa-onnx 关键词检测模型（zipformer 中英混合），实现「说出唤醒词 → 程序反应」。

```bash
# 1. 下载模型（约 31MB，默认安装到 ~/.zapmomo/models/<模型名>，不入库）
cargo run -- kws install-model

# 2. 离线验证（无需麦克风）：对模型自带 wav 检测出「文森特卡索」「法国」
cargo run -- kws test

# 3. 实时监听：说出唤醒词，控制台打印反应（首次运行需授权麦克风）
cargo run -- kws run

# 4. 查看可用麦克风设备
cargo run -- kws devices
```

| 命令 | 说明 |
|------|------|
| `kws run` | 实时监听麦克风，检测唤醒词。`--duration 秒` 限时、`--device 名称` 指定设备、`--keywords` 附加关键词（直接输中文，多个用 `/` 分隔） |
| `kws test` | 离线检测 wav（默认模型自带 `test_wavs/zh_3.wav`）。`--wav` 指定文件 |
| `kws devices` | 列出可用输入设备 |
| `kws install-model` | 下载并安装唤醒词模型（默认 `~/.zapmomo/models/<模型名>`）。`--model-dir` 指定目录、`--force` 强制重装 |

配置（`~/.zapmomo/settings.toml` 的 `[kws]` 段，全部可选）：

```toml
[kws]
model_dir = "/path/to/model"              # 模型目录（支持 ${env.VAR}）
provider = "cpu"                           # 推理后端，默认 cpu
num_threads = 4                            # 推理线程数，默认 2
chunk_size = 3200                          # 每次喂给模型的采样数（@16k），默认 3200
sample_rate = 16000                        # 模型输入采样率，默认 16000
keywords_score = 1.0                       # 关键词 boosting 分数
keywords_threshold = 0.25                  # 触发阈值：越大越不容易误触发（0.15~0.5）
encoder = "encoder-...-chunk-16-left-64.int8.onnx"   # 模型目录内带 int8 变体可选
decoder = "decoder-...-chunk-16-left-64.onnx"
joiner  = "joiner-...-chunk-16-left-64.onnx"
tokens  = "tokens.txt"
keywords_file = "/path/to/keywords.txt"    # 自定义关键词文件
debug = false
```

#### 自定义唤醒词

**直接输入中文即可**：`--keywords 你好小智` 会由内置的拼音转换（`src/kws/token.rs`）自动把汉字拆成模型
可编码的 ppinyin token（`你好小智` → `n ǐ h ǎo x iǎo zh ì`），无需任何外部工具；多个关键词用 `/` 或换行分隔。

keywords 文件（默认 `<model_dir>/test_wavs/keywords.txt`，可用 `[kws] keywords_file` 覆盖）每行一个关键词，
同样支持直接写中文，也支持精确的「token + `@显示词`」格式：

```
你好小智 @你好小智                      # 中文：直接写，自动转 ppinyin
w én s ēn t è k ǎ s uǒ @文森特卡索     # 中文：精确 token（声母+带调韵母）
L AY1 T AH1 P @LIGHT_UP                  # 英文：ARPAbet 音素
```

v1 默认使用模型自带的中英混合关键词集（见 `test_wavs/keywords.txt`）。

#### 模型相关测试

```bash
# 常规测试（不依赖模型）
cargo test -- --test-threads=1

# 模型相关测试（需先下载模型）
./scripts/run-kws-model-tests.sh
```

### 声纹识别（Speaker Recognition）

基于 sherpa-onnx 官方 CAM++ 声纹模型（3D-Speaker，中文 16k，192 维），从**声音特征**（而非 ASR 文本）
判断「是谁在说话」：支持多人注册（每段音频独立提取 embedding，检索取最大相似度）、1:1 验证、
1:N 识别与 unknown 判定。声纹档案持久化在 `~/.zapmomo/speaker_profiles/<id>.json`，
语音会话内可通过 `[speaker].enabled` 在 ASR 前对用户语音打说话人标签（默认关闭）。

> 声纹识别仅用于区分说话人，**不是安全认证**：录音回放、噪声、麦克风差异与声音变化都可能导致误判。

```bash
# 1. 下载声纹模型（约 27MB，默认安装到 ~/.zapmomo/models/<模型名>）
cargo run -- speaker install-model

# 2. 注册：传多个 wav 或一个目录（目录取全部 *.wav；同名覆盖重建）
cargo run -- speaker enroll owner ./samples/owner/

# 3. 识别（1:N）：输出 speaker/score/threshold/latency 与全量分数表
cargo run -- speaker identify test.wav

# 4. 验证（1:1）
cargo run -- speaker verify owner test.wav

# 5. 管理已注册说话人
cargo run -- speaker list
cargo run -- speaker remove owner
```

配置（`~/.zapmomo/settings.toml` 的 `[speaker]` 段，全部可选）：

```toml
[speaker]
enabled = false                   # 启用声纹识别（仅响应已注册说话人：不匹配整句忽略）
threshold = 0.6                   # 相似度阈值（余弦），越大越严格
min_audio_duration_secs = 1.0     # 短于该时长直接跳过识别（防「嗯/啊」短促声误识别）
model_dir = "/path/to/model"      # 模型目录（支持 ${env.VAR}）
model = "xxx.onnx"                # 模型文件名（缺省扫描目录探测 campplus 模型）
provider = "cpu"                  # 推理后端
num_threads = 1                   # 推理线程数
auto_download = true              # 模型缺失时自动下载
max_buffer_duration_secs = 20.0   # 会话内声纹缓冲上限（超长截断保留最近音频）
debug = false
```

桌面应用「模型与能力 → 声纹识别」页提供等价的 GUI：启用开关、参数、模型一键下载、
应用内录音注册（或选择 wav）、已注册说话人管理与识别测试。GUI 与 CLI 共用同一份
声纹档案与共享引擎实例（应用内注册对运行中的语音会话即时生效）。

### 语音识别（ASR）

接入 sherpa-onnx 语音识别模型，把麦克风语音实时转成文本（支持中英混说）。模型族：

- **流式 Zipformer**（默认，中英双语）— 实时识别，支持热词
- **离线 Qwen3-ASR** — 29 语言自动识别；整段文件转写与免提听写（VAD 分段），不参与实时会话

```bash
# 1. 下载模型（约 790MB：ASR int8 + 标点，默认安装到 ~/.zapmomo/models/<模型名>，不入库）
cargo run -- asr install-model

# 1b. 其它模型（如离线 Qwen3-ASR）在桌面应用「模型」页下载安装

# 2. 离线转写（无需麦克风）：对模型自带 wav 输出转写文本（最终结果自动加标点）
cargo run -- asr test

# 3. 实时转写：说话即出字幕（首次运行需授权麦克风，Ctrl-C 退出）
cargo run -- asr run

# 4. 查看可用麦克风设备
cargo run -- asr devices
```

- **标点恢复（自动开启）**：`install-model` 会同时下载标点模型，识别出的**最终结果自动加标点**（如「昨天是 Monday，是星期三。」）。标点模型缺失时 ASR 仍可用，仅无标点（降级不报错）。
- **热词增强**：对专有名词/易错词提权。命令行用 `--hotwords "尼日尔河 文森特卡索"`（空格分隔、中文直接写），或写入 `settings.toml` 的 `[asr] hotwords`。热词与空白符惩罚（`blank_penalty`）仅流式 zipformer（transducer）生效。

| 命令 | 说明 |
|------|------|
| `asr run` | 实时监听麦克风并转写。`--duration 秒` 限时、`--device 名称` 指定设备、`--hotwords "词1 词2"` 热词 |
| `asr test` | 离线转写 wav（默认模型自带 `test_wavs/0.wav`）。`--wav` 指定文件、`--hotwords` 热词 |
| `asr devices` | 列出可用输入设备 |
| `asr install-model` | 下载并安装 ASR + 标点模型（默认 `~/.zapmomo/models/<模型名>`）。`--model-dir` 指定目录、`--force` 强制重装 |

配置（`~/.zapmomo/settings.toml` 的 `[asr]` 段，全部可选）：

```toml
[asr]
model_dir = "/path/to/model"              # 模型目录（支持 ${env.VAR}）
provider = "cpu"                           # 推理后端，默认 cpu
num_threads = 4                            # 推理线程数，默认 2
decoding_method = "greedy_search"          # greedy_search | modified_beam_search
enable_endpoint = true                     # 端点检测（静音自动断句）
rule1_min_trailing_silence = 2.4          # 断句静音阈值（秒）
rule2_min_trailing_silence = 1.2
rule3_min_utterance_length = 20.0
hotwords = "你好小智 文森特卡索"          # 热词（空格分隔、中文直接写），可选
enable_punctuation = true                  # 最终结果自动加标点，默认 true
punctuation_model = "model.onnx"           # 标点模型文件名（相对标点模型目录）
encoder = "encoder-epoch-99-avg-1.int8.onnx"
decoder = "decoder-epoch-99-avg-1.onnx"    # 官方 int8 配方：fp32 decoder
joiner  = "joiner-epoch-99-avg-1.int8.onnx"
tokens  = "tokens.txt"
debug = false
```

### 文本转语音（TTS）

接入 sherpa-onnx 的 ZipVoice 零样本声音克隆模型（中英双语），把文本合成为 wav（离线批量合成，无流式 feed）。

```bash
# 1. 下载模型（约 156MB：TTS 主包 + 声码器，默认安装到 ~/.zapmomo/models/<模型名>，不入库）
cargo run -- tts install-model

# 2. 列出内置音色（雷军、新闻女声等）
cargo run -- tts voices

# 3. 合成语音（默认音色雷军；--voice 切换内置音色、--speed 调语速）
cargo run -- tts run --text "你好，我是 ZapMomo"
cargo run -- tts run --text "你好" --voice news-female --speed 1.2
```

- **零样本声音克隆**：`--voice 内置音色` 一键使用，或用 `--reference-wav 参考音频` + `--reference-text 转写文本` 克隆任意音色
- **输出**：默认 `~/.zapmomo/tts/<时间戳>.wav`，`--output` 指定路径

| 命令 | 说明 |
|------|------|
| `tts run` | 合成文本为 wav。`--text` 必填；`--voice` 内置音色、`--speed` 语速、`--reference-wav/--reference-text` 自定义参考音色、`--output` 输出路径 |
| `tts voices` | 列出内置音色（解析模型包 `test_wavs/prompt.txt`） |
| `tts install-model` | 下载安装 TTS 主包 + 声码器（默认 `~/.zapmomo/models/<模型名>`）。`--model-dir` 指定目录、`--force` 强制重装 |

配置（`~/.zapmomo/settings.toml` 的 `[tts]` 段，全部可选）：

```toml
[tts]
model_dir = "/path/to/model"              # 模型目录（支持 ${env.VAR}）
encoder = "encoder.int8.onnx"
decoder = "decoder.int8.onnx"
vocoder = "vocos_24khz.onnx"              # 声码器（install-model 时一并下载）
tokens = "tokens.txt"
lexicon = "lexicon.txt"
data_dir = "espeak-ng-data"
reference_wav = "test_wavs/leijun-1.wav"  # 默认音色参考音频
reference_text = "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系."  # 参考音频转写
num_steps = 4                             # 扩散解码步数（质量/速度权衡）
speed = 1.0                               # 语速
provider = "cpu"                          # 推理后端，默认 cpu
num_threads = 2                           # 推理线程数
debug = false
```

### 本地大语言模型（LLM）

基于 llama.cpp（Rust 绑定 `llama-cpp-2`）的本地大语言模型，支持流式对话与 Agent 工具调用；也可通过 OpenAI 兼容的 `/v1/responses` 接口接入远程 API 或 `llama-server`。

LLM 模型为 **GGUF 文件**：内置清单提供多个可一键下载的预设（应用内「AI 大脑（LLM）配置」页），也支持自备 GGUF 放入 `~/.zapmomo/models/<任意目录>/` 自动发现，或用 `[llm] model_path` 指定路径。

```bash
# 1. 获取模型：桌面应用「AI 大脑（LLM）配置」页一键下载（Qwen3-0.6B / 4B 预设），
#    或自行下载推荐模型 Qwen3-4B-Instruct-2507（Q4_K_M 量化约 2.5GB）放到 ~/.zapmomo/models/
# 2. 验证模型可加载
cargo run -- llm load

# 3. 单轮对话（流式输出）
cargo run -- llm chat --text "你好，你是谁？"
```

- **推荐模型**：Qwen3-4B-Instruct-2507（`Qwen3-4B-Instruct-2507-Q4_K_M.gguf`）；任意 GGUF 均可，自动发现
- **后端**：默认纯 CPU；Metal 加速已预留（`gpu_layers` 可配，llama-cpp-2 0.1.154 的 Metal logits 崩溃待升级依赖后启用）
- **Agent**：循环调用 provider、执行工具调用，直到产出纯文本回复（最多 10 轮，防止死循环）
- **远程接入**：配置 `base_url / api_key / model` 走 OpenAI 兼容 `/v1/responses`（官方 API 或 `llama-server`）

| 命令 | 说明 |
|------|------|
| `llm load` | 加载模型并打印信息（架构 / 上下文）。`--model-path` 指定 GGUF |
| `llm chat` | 单轮对话（加载 + 流式生成）。`--text` 必填、`--model-path` 指定 GGUF |

配置（`~/.zapmomo/settings.toml` 的 `[llm]` 段，全部可选）：

```toml
[llm]
enabled = false                    # 是否启用（桌面应用默认懒加载）
provider = "local"                 # local（llama.cpp）| http（OpenAI 兼容）
model_path = "/path/to/model.gguf" # GGUF 绝对路径（支持 ${env.VAR}）
system_prompt = "你是 ZapMomo，一个友好的桌面 AI 伙伴。请用简洁自然的中文回答，语气亲切，不要啰嗦。"
context_size = 8192                # 上下文窗口（token）
batch_size = 512                   # 单次 decode 的 batch 大小
max_tokens = 512                   # 最多生成 token 数
temperature = 0.7
top_p = 0.8
top_k = 20
min_p = 0.05
repeat_penalty = 1.05
seed = 0                           # 随机种子；0 = 随机
threads = 0                        # CPU 线程数；0 = 自动（物理核数 - 2）
gpu_layers = 0                     # 卸载到 GPU 的层数；-1 = 全部（Metal），0 = 纯 CPU
enable_thinking = false            # Qwen3 思考模式（输出 <think> 块）
auto_load = false                  # 应用启动时自动加载模型
# --- http provider 专用 ---
# base_url = "http://127.0.0.1:8080/v1"  # OpenAI 兼容 base URL
# api_key = ""                            # API key（本地 server 可留空）
# model = "qwen3-4b"                      # 模型名
```

### 语音会话（Voice）

把 KWS / ASR / LLM / TTS 四个能力模块串成一条完整对话链路：**唤醒词 → 识别 → 思考 → 句级流式播报**。
sherpa-onnx 的 TTS 只有整句一次性合成，因此「流式输出」由句级流水线近似：LLM 流式 token → 断句 → 独立合成线程逐句合成 → 边合成边播放。

- **唤醒词打断** — 播报/思考期间保持唤醒词监听，再次唤醒立即打断回听
- **免唤醒续聊** — 回复播完后自动进入聆听，无需重复唤醒

```bash
# 开始语音会话：说唤醒词唤醒、对话播报，Ctrl-C 退出
cargo run -- voice run
```

| 命令 | 说明 |
|------|------|
| `voice run` | 跑完整语音会话（唤醒 → 识别 → 对话 → 句级流式播报）。`--keywords` 唤醒词、`--voice` 音色、`--speed` 语速、`--max-turns` 轮数上限 |

配置（`~/.zapmomo/settings.toml` 的 `[voice]` 段，全部可选）：

```toml
[voice]
enabled = true                # 应用启动时自动进入待唤醒，默认 true
keywords = "你好小智"          # 会话唤醒词（中文直接写，多个用 / 分隔），默认 KWS 模型内置
voice = "leijun-1"             # 回复用 TTS 音色 id
speed = 1.0                    # 播报语速
max_turns = 0                  # 最多对话轮数；0 = 无限（Ctrl-C 退出）
history_max = 12               # 传给 LLM 的历史消息条数上限
barge_in = true                # 播报/思考中唤醒词打断，默认 true
voice_barge_in = true          # 播报中语音打断（说话即打断，仅流式 ASR 生效），默认 true
barge_in_similarity_threshold = 0.5  # 语音打断回声比对阈值（外放误触发时调高）
follow_up = true               # 回复播完自动聆听（免唤醒续聊），默认 true
welcome_text = "你好，我在。"  # 唤醒后的欢迎语
```

## 桌面应用开发（Tauri 2）

复用同一套 KWS / ASR / TTS / LLM / Voice / 音频 / 配置逻辑的桌面 GUI，代码在 `src-tauri/`，
前端为 React + Vite + TypeScript（Tailwind CSS + shadcn/ui，构建产物打包进应用）。

```bash
# 安装 Tauri CLI（首次）
pnpm install

# 开发模式（热重载，需已下载模型：cargo run -- kws install-model）
pnpm tauri dev

# 构建当前平台的安装包（macOS 产出 .app/.dmg）
pnpm tauri build

# 仅检查 / Lint tauri crate（Linux 需 webkit 依赖）
cargo check -p zapmomo-app
cargo clippy -p zapmomo-app -- -D warnings
```

### audio.cpp sidecar 引擎（externalBin）

TTS 第二后端（PocketTTS English）由 audio.cpp 引擎驱动，引擎二进制作为
externalBin 随安装包分发。`pnpm tauri dev` / `pnpm tauri build` 要求
`src-tauri/binaries/audiocpp_server-<target-triple>` 存在（该目录不入库）：

```bash
# 从本仓库 Release 下载（日常；首次发版前无产物，用下面的 --build）
scripts/fetch-audiocpp-dev.sh

# 本地源码编译（裁剪构建仅含 pocket_tts 模型族，约 1.5 分钟）
scripts/fetch-audiocpp-dev.sh --build
```

也可以不放该目录——引擎放在 `~/.zapmomo/engines/` 或 PATH 中同样会被自动发现；
不使用 audio.cpp 后端（默认 sherpa）时引擎缺失只影响 `[tts].backend = "audiocpp"`
的合成，其余功能不受影响。引擎版本 pin 在 `.github/workflows/release.yml` 的
`AUDIOCPP_REF`。

### Live2D 虚拟角色（开发说明）

常驻角色窗口由 `src-tauri/` 实现，模型定位逻辑在根 crate 的 `src/live2d/`：

- **尺寸自适应** — 窗口尺寸随模型真实包围盒宽高比自适应
- **格式** — 支持 Cubism 2 / 3 / 4 / 5（`.model3.json` / `model.json`）
- **模型来源** — 用户自备 Live2D 模型目录（非清单下载），默认 `~/.zapmomo/models/live2d`；Cubism Core 运行时随仓库版本管理

配置（`~/.zapmomo/settings.toml` 的 `[live2d]` 段，全部可选）：

```toml
[live2d]
model_dir = "/path/to/live2d-model"      # 模型根目录（含 .model3.json / model.json）
window_position = { x = 100, y = 100 }   # 角色窗口位置记忆
window_scale = 1.0                       # 窗口缩放（0.25 ~ 2.0）
```

### 一键重启（开发模式白屏）

设置面板「通用」、角色右键菜单与托盘菜单均提供「重启」：退出后自动重新拉起，用于应用需要重启才能生效的配置。

- **打包版（生产）** — 正常：前端资源内置（`asset://`），重启后直接加载。
- **开发模式（`pnpm tauri dev`）** — 重启后新进程会**白屏**。原因：Tauri 内置重启只重新拉起应用二进制、不重跑 `beforeDevCommand`，而 `tauri dev` 在应用退出时会连同 Vite dev server 一起拆掉（[tauri#6163](https://github.com/tauri-apps/tauri/issues/6163)），新进程连不上 `localhost:1420`。需要重启效果时请手动重跑 `pnpm tauri dev`。

## 项目结构

```
├── Cargo.toml           # 项目配置和依赖（workspace 根）
├── rust-toolchain.toml  # Rust 工具链版本（1.97.1）
├── src/
│   ├── main.rs          # 入口文件
│   ├── lib.rs           # 库入口 + 测试工具（test_util 临时 HOME 隔离）
│   ├── cli.rs           # CLI 命令定义（kws / asr / tts / llm / voice）
│   ├── config/
│   │   ├── mod.rs       # 配置模块入口
│   │   └── settings.rs  # TOML 配置管理（含 [kws]/[asr]/[tts]/[llm]/[voice]/[live2d] 段）
│   ├── kws/             # 关键词唤醒词检测（sherpa-onnx）
│   │   ├── mod.rs       # KwsEngine + 离线/实时检测
│   │   ├── config.rs    # KWS 配置解析与默认值
│   │   ├── model.rs     # 模型下载 / sha256 校验 / 解压安装
│   │   ├── token.rs     # 汉字 → ppinyin token 转换
│   │   ├── english.rs   # 英文关键词 → ARPAbet 音素
│   │   └── reaction.rs  # Reaction 可插拔反应（控制台 / GUI / 测试）
│   ├── asr/             # 语音识别（sherpa-onnx 流式转写）
│   │   ├── mod.rs       # AsrEngine + 离线/实时转写
│   │   ├── config.rs    # ASR 配置解析与默认值
│   │   └── reaction.rs  # Reaction 可插拔反应
│   ├── tts/             # 文本转语音（sherpa-onnx ZipVoice）
│   │   ├── mod.rs       # TtsEngine + 离线合成
│   │   ├── config.rs    # TTS 配置解析与默认值
│   │   ├── voice.rs     # 内置音色解析
│   │   └── reaction.rs  # Reaction 可插拔反应
│   ├── llm/             # 本地大语言模型（llama.cpp）
│   │   ├── mod.rs       # LlmEngine 门面 + 事件 channel
│   │   ├── config.rs    # LLM 配置解析与默认值
│   │   ├── local/       # LocalLlamaProvider（llama.cpp 后端）
│   │   ├── http.rs      # OpenAI 兼容 Responses API provider
│   │   ├── agent.rs     # Agent 循环 + 工具调用
│   │   ├── provider.rs  # LlmProvider trait
│   │   └── tools.rs     # 工具运行时
│   ├── voice/           # 语音会话编排（KWS→ASR→LLM→TTS 全链路）
│   │   ├── mod.rs       # 会话模块入口 + CLI 运行
│   │   ├── session.rs   # 会话状态机与事件循环（唤醒/聆听/思考/播报）
│   │   ├── listen.rs    # 唤醒/聆听监听（KWS + ASR）
│   │   ├── splitter.rs  # LLM 流式 token 断句
│   │   ├── synthesizer.rs # TTS 句级合成线程
│   │   ├── player.rs    # 音频播报（rodio Sink）
│   │   ├── records.rs   # 对话记录持久化
│   │   ├── events.rs    # 会话事件
│   │   ├── state.rs     # 会话状态
│   │   └── config.rs    # [voice] 配置解析
│   ├── companion.rs     # 伙伴库（Live2D 模型集合 + 当前使用项）
│   ├── model_library/   # 模型列表核心服务（registry 预设 / 安装 / 下载 / 选择）
│   ├── live2d/          # Live2D 角色模型定位
│   │   ├── mod.rs
│   │   └── config.rs    # [live2d] 配置解析
│   ├── audio.rs         # cpal 麦克风采集 + 重采样
│   ├── logging.rs       # tracing 双层日志
│   └── datetime.rs      # 日期时间工具
├── models/              # 模型资产（本体不入库，按清单下载）
│   ├── manifest.json    # 模型清单（source / sha256 / license）
│   └── THIRD_PARTY_NOTICES.md
├── src-tauri/           # Tauri 2 桌面应用（workspace 成员）
│   ├── src/lib.rs       # commands + 监听线程 + TauriReaction
│   ├── frontend/        # React + Vite + TypeScript 多页面控制面板（Tailwind + shadcn/ui）
│   ├── tauri.conf.json  # Tauri 配置（打包目标/图标/权限文案）
│   ├── capabilities/    # 权限声明
│   └── icons/           # 应用图标
├── tests/               # 集成测试
├── package.json         # Tauri CLI（@tauri-apps/cli）
├── scripts/             # 模型下载 / 模型测试 / 图标生成等脚本
├── .github/             # CI / 发布流水线 / Issue 模板
└── .githooks/           # Git hooks
```

## 依赖说明

| 分类 | Crate | 用途 |
|------|-------|------|
| 核心 | clap / clap_complete | CLI 参数解析 / Shell 补全生成 |
| 核心 | tokio | 异步运行时 |
| 核心 | serde / serde_json / toml | 序列化 |
| 核心 | chrono | 日期时间处理 |
| 核心 | tracing / tracing-subscriber | 日志 |
| 核心 | thiserror / anyhow | 错误处理 |
| KWS | sherpa-onnx | 唤醒词检测 / 语音识别 / 文本转语音（预编译库） |
| KWS | cpal | 麦克风音频采集 |
| KWS | pinyin | 汉字 → 带声调拼音（自定义关键词自动转换） |
| LLM | llama-cpp-2 | 本地大语言模型推理（llama.cpp Rust 绑定） |
| LLM | encoding_rs | token 逐字节解码（llama-cpp-2 UTF-8 decoder） |
| LLM | reqwest | OpenAI 兼容 HTTP provider（`/v1/responses`） |
| Voice | rodio | 音频播报（Sink 句级流式播放） |
| Voice | ctrlc | Ctrl-C 优雅退出（语音会话） |
| 模型下载 | ureq | HTTP 客户端（模型下载） |
| 模型下载 | sha2 / hex | 下载模型的 sha256 校验 |
| 模型下载 | tar / bzip2 | 解压 tar.bz2 模型包 |

## 发布流程

每次发布新版本会自动构建 **Windows / macOS（Intel+Apple Silicon）/ Linux** 安装包并合并到一个 GitHub Release：

1. 合入 `main` 后，`publish.yml` 中的 release-plz 自动 bump 版本、更新 changelog，打出 `vX.Y.Z` tag 并发布到 crates.io，同时维护「版本发布 PR」。
2. tag push 触发 `release.yml`：在三个平台的原生 runner 上运行 `tauri-action` 构建安装包（`.dmg` / `.app.tar.gz` / `.msi` / `.exe` / `.deb` / `.rpm` / `.AppImage`）。
3. 构建成功后自动发布为正式 Release（`draft: false`，不再停留在草稿）。

发布产物矩阵：

| 平台 | 安装包 |
|------|--------|
| macOS (Apple Silicon) | `.dmg` + `.app.tar.gz` |
| macOS (Intel) | `.dmg` + `.app.tar.gz` |
| Windows x64 | `.msi` + `.exe`（NSIS） |
| Linux x64 | `.deb` + `.rpm` + `.AppImage` |

> 签名：当前为未签名构建，适合内部/测试分发。正式对外发布时在仓库 Secrets 配置
> Apple Developer ID 证书（`APPLE_SIGNING_IDENTITY / APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID`）
> 与 Windows 证书后，tauri-action 会自动签名/公证。

## Git 工作流

### 分支命名

- `feature/xxx` - 新功能
- `fix/xxx` - Bug 修复
- `docs/xxx` - 文档更新
- `refactor/xxx` - 重构

### Commit 规范

遵循 [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]
```

**类型**:

- `feat` - 新功能
- `fix` - Bug 修复
- `docs` - 文档更新
- `style` - 代码格式
- `refactor` - 重构
- `perf` - 性能优化
- `test` - 测试相关
- `chore` - 构建/工具
