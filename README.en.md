<div align="right">

[简体中文](README.md) | **English**

</div>

<div align="center">
  <img src="docs/public/logo.svg" alt="ZapMomo Logo" width="300" />

  <p>
    <a href="https://github.com/shenjingnan/zapmomo/releases"><img src="https://img.shields.io/github/v/release/shenjingnan/zapmomo" alt="GitHub Release" /></a>
    <a href="https://crates.io/crates/zapmomo"><img src="https://img.shields.io/crates/v/zapmomo" alt="crates.io version" /></a>
    <a href="https://crates.io/crates/zapmomo"><img src="https://img.shields.io/crates/d/zapmomo" alt="crates.io downloads" /></a>
    <a href="https://github.com/shenjingnan/zapmomo/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/shenjingnan/zapmomo/ci.yml?branch=main&label=CI" alt="GitHub Actions CI status" /></a>
    <a href="https://codecov.io/gh/shenjingnan/zapmomo"><img src="https://codecov.io/gh/shenjingnan/zapmomo/graph/badge.svg" alt="Codecov coverage" /></a>
    <br />
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPL--3.0-blue" alt="License: GPL-3.0" /></a>
    <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.97%2B-dea584?logo=rust" alt="Rust 1.97+" /></a>
    <a href="#app-download"><img src="https://img.shields.io/badge/Windows-0078D6?logo=windows&logoColor=white" alt="Windows support" /></a>
    <a href="#app-download"><img src="https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=white" alt="macOS support" /></a>
    <a href="#app-download"><img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black" alt="Linux support" /></a>
  </p>
</div>

An open-source, real-time desktop **AI companion** with voice, memory, and a customizable virtual character.

<div align="center">
  <img src="docs/public/screenshots/home.png" alt="ZapMomo desktop app Overview page" width="760" />
</div>

<details>

<summary>✨ Features</summary>

- **Voice Wake Word (KWS)** — sherpa-onnx based zipformer keyword spotting with live microphone listening and offline wav detection; custom keywords typed directly in Chinese, auto-converted to pinyin tokens
- **Speech Recognition (ASR)** — sherpa-onnx streaming recognition (Zipformer / Paraformer, bilingual Chinese-English), real-time captions with automatic punctuation; hotword boosting on Zipformer, plus offline SenseVoice/Whisper options
- **Text-to-Speech (TTS)** — sherpa-onnx ZipVoice zero-shot voice cloning (bilingual), with built-in voices and custom reference audio
- **Local LLM** — llama.cpp local inference (any GGUF, streaming chat + agent tool calls), or an OpenAI-compatible remote API
- **Voice Session** — wake word → ASR → LLM sentence-level streaming reply → TTS playback, with wake-word barge-in and hands-free follow-up
- **Live2D Virtual Character** — persistent character window (Cubism 2/3/4/5) with position memory and percentage scaling; drag without stealing focus; GIF companions and character packs (static portrait + persona + voice cloning) are also supported
- **Desktop App** — Tauri 2 GUI (multi-page control panel: Overview / Chat / Companion / Models / Settings, plus a persistent character window), with installers for Windows / macOS / Linux
- **deepseek-harness Integration** — the desktop companion reacts to [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) (dsh) task state in real time, announcing task started / finished / failed / interrupted with a speech bubble + voice ([usage guide](docs/content/docs/desktop-app/dsh-bridge.mdx))
- **CLI** — `kws` / `asr` / `tts` / `llm` / `voice` subcommands covering every capability, with bash / zsh / fish / powershell / elvish autocompletion

</details>

## App Download

Click a button below to grab the latest installer for your system (no GitHub login required; always points to the latest release):

| OS | Chip / Arch | Download |
| --- | --- | --- |
| Windows 10 / 11 | x64 | [![Download](https://img.shields.io/badge/Download-0078D6?style=for-the-badge&logo=windows&logoColor=white)](https://github.com/shenjingnan/zapmomo/releases/latest/download/ZapMomo_Windows_x64.exe) |
| macOS 13+ | Apple Silicon (M1/M2/M3/M4) | [![Download](https://img.shields.io/badge/Download-8E8E93?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/shenjingnan/zapmomo/releases/latest/download/ZapMomo_macOS_arm64.dmg) |
| macOS 13+ | Intel | [![Download](https://img.shields.io/badge/Download-8E8E93?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/shenjingnan/zapmomo/releases/latest/download/ZapMomo_macOS_x64.dmg) |
| Ubuntu / Debian | amd64 | [![Download](https://img.shields.io/badge/Download-A80030?style=for-the-badge&logo=linux&logoColor=white)](https://github.com/shenjingnan/zapmomo/releases/latest/download/ZapMomo_Linux_amd64.deb) |
| Fedora / RHEL | x86_64 | [![Download](https://img.shields.io/badge/Download-294172?style=for-the-badge&logo=linux&logoColor=white)](https://github.com/shenjingnan/zapmomo/releases/latest/download/ZapMomo_Linux_x86_64.rpm) |

- For Windows enterprise mass deployment there is also an [MSI build](https://github.com/shenjingnan/zapmomo/releases/latest/download/ZapMomo_Windows_x64.msi); on Linux there is also an [AppImage](https://github.com/shenjingnan/zapmomo/releases/latest/download/ZapMomo_Linux_amd64.AppImage) that runs without installation.
- See [Releases](https://github.com/shenjingnan/zapmomo/releases) for the full version history and changelogs.
- 🍎 Not sure which chip your Mac has?  menu (top-left) → "About This Mac": "Chip: Apple M…" → arm64; "Processor: Intel…" → x64.
- KWS / ASR / TTS model assets are not bundled with the installers. On first use, download them via the app's "Models" page or the CLI `install-model` command.

### First Launch on macOS (Unsigned)

The project has no Apple Developer certificate, so installers are **unsigned**. Every freshly downloaded build from Releases is blocked on first launch ("cannot verify developer"). Drag the app into "Applications", then run:

```bash
xattr -cr "/Applications/ZapMomo.app"
```

After that the app launches normally. If the app is not in "Applications", replace the path with its actual location; or right-click the app → "Open" → click "Open" again.

<details>

<summary>🎙️ Keyword Wake Word (KWS)</summary>

Integrates a sherpa-onnx keyword-spotting model (bilingual Chinese-English zipformer): say the wake word → the app reacts.

### Quick Start

```bash
# 1. Download the model (~31 MB, installed to ~/.zapmomo/models/<name>, never committed to the repo)
cargo run -- kws install-model

# 2. Offline check (no mic needed): detects "文森特卡索" (Vincent Cassel) and "法国" (France) in the bundled wav
cargo run -- kws test

# 3. Live listening: say the wake word and reactions print to the console (mic permission required on first run)
cargo run -- kws run

# 4. List available input devices
cargo run -- kws devices
```

### Model Source & Verification

Models are **not distributed with the code**. The built-in `kws install-model` command (or `scripts/download-kws-model.sh`) downloads them according to the `models/manifest.json` manifest:

- **Manifest** — `models/manifest.json` (committed to the repo) records each model's `name / version / source / sha256 / license`
- **Verification** — after downloading, the archive's sha256 is computed and compared against the manifest; **on mismatch the download is deleted and an error is raised**. Archives are extracted into a temp directory first and then moved atomically, so no corrupted half-written models are left behind
- **Idempotent** — skipped if the model already exists and is intact
- **Compliance** — third-party sources and licenses are listed in `models/THIRD_PARTY_NOTICES.md`
- ASR and TTS reuse the same manifest mechanism (see the "Speech Recognition" and "Text-to-Speech" sections below)

### Commands

| Command | Description |
|------|------|
| `kws run` | Listen to the microphone live and detect wake words. `--duration secs` to time-limit, `--device name` to pick a device, `--keywords` to add keywords (typed directly in Chinese, `/`-separated) |
| `kws test` | Offline wav detection (defaults to the bundled `test_wavs/zh_3.wav`). `--wav` to specify a file |
| `kws devices` | List available input devices |
| `kws install-model` | Download and install the KWS model (default `~/.zapmomo/models/<name>`). `--model-dir` to pick a directory, `--force` to reinstall |

### Configuration

Add a `[kws]` section to `~/.zapmomo/settings.toml` to override defaults (all optional):

```toml
[kws]
model_dir = "/path/to/model"              # Model directory (supports ${env.VAR})
provider = "cpu"                           # Inference backend, default cpu
num_threads = 4                            # Inference threads, default 2
chunk_size = 3200                          # Samples fed to the model per chunk (@16k), default 3200
sample_rate = 16000                        # Model input sample rate, default 16000
keywords_score = 1.0                       # Keyword boosting score
keywords_threshold = 0.25                  # Trigger threshold: higher = fewer false triggers (0.15~0.5)
encoder = "encoder-...-chunk-16-left-64.int8.onnx"   # int8 variants inside the model dir are selectable
decoder = "decoder-...-chunk-16-left-64.onnx"
joiner  = "joiner-...-chunk-16-left-64.onnx"
tokens  = "tokens.txt"
keywords_file = "/path/to/keywords.txt"    # Custom keywords file
debug = false
```

### Custom Wake Words

**Just type Chinese**: `--keywords 你好小智` is automatically split by the built-in pinyin conversion (`src/kws/token.rs`) into ppinyin tokens the model can encode (`你好小智` → `n ǐ h ǎo x iǎo zh ì`) — no external tools needed. Separate multiple keywords with `/` or newlines.

A keywords file (default `<model_dir>/test_wavs/keywords.txt`, override with `[kws] keywords_file`) holds one keyword per line. It also accepts Chinese directly, as well as the precise "token + `@display`" format:

```
你好小智 @你好小智                      # Chinese: write it directly, auto-converted to ppinyin
w én s ēn t è k ǎ s uǒ @文森特卡索     # Chinese: precise tokens (initials + toned finals)
L AY1 T AH1 P @LIGHT_UP                  # English: ARPAbet phonemes
```

v1 ships with the model's bundled bilingual keyword set (see `test_wavs/keywords.txt`).

</details>

<details>

<summary>🗣️ Speech Recognition (ASR)</summary>

Integrates a sherpa-onnx streaming ASR model (bilingual Chinese-English zipformer) that turns microphone audio into text in real time (code-switching supported).

### Quick Start

```bash
# 1. Download the model (~790 MB: ASR int8 + punctuation, installed to ~/.zapmomo/models/<name>, never committed)
cargo run -- asr install-model

# 2. Offline transcription (no mic needed): prints the transcript of the bundled wav (final result gets punctuation)
cargo run -- asr test

# 3. Live transcription: speak and see captions (mic permission required on first run, Ctrl-C to exit)
cargo run -- asr run

# 4. List available input devices
cargo run -- asr devices
```

Model source, sha256 verification, and idempotent installation are identical to `[kws]` (see "Model Source & Verification" above).

### Punctuation & Hotwords

- **Punctuation restoration (on by default)** — `install-model` also downloads a punctuation model, and the **final result gets punctuation automatically** (e.g. 「昨天是 Monday，是星期三。」). If the punctuation model is missing, ASR still works, just without punctuation (graceful degradation, no error).
- **Hotword boosting** — boosts proper nouns and easily-misheard words. On the CLI use `--hotwords "尼日尔河 文森特卡索"` (space-separated, Chinese directly), or set `[asr] hotwords` in `settings.toml`.

### Commands

| Command | Description |
|------|------|
| `asr run` | Live microphone transcription. `--duration secs` to time-limit, `--device name` to pick a device, `--hotwords "word1 word2"` for hotwords |
| `asr test` | Offline wav transcription (defaults to the bundled `test_wavs/0.wav`). `--wav` to specify a file, `--hotwords` for hotwords |
| `asr devices` | List available input devices |
| `asr install-model` | Download and install the ASR + punctuation models (default `~/.zapmomo/models/<name>`). `--model-dir` to pick a directory, `--force` to reinstall |

### Configuration

Add an `[asr]` section to `~/.zapmomo/settings.toml` to override defaults (all optional):

```toml
[asr]
model_dir = "/path/to/model"              # Model directory (supports ${env.VAR})
provider = "cpu"                           # Inference backend, default cpu
num_threads = 4                            # Inference threads, default 2
decoding_method = "greedy_search"          # greedy_search | modified_beam_search
enable_endpoint = true                     # Endpoint detection (auto-split utterances on silence)
rule1_min_trailing_silence = 2.4          # Trailing-silence threshold for splitting (seconds)
rule2_min_trailing_silence = 1.2
rule3_min_utterance_length = 20.0
hotwords = "你好小智 文森特卡索"          # Hotwords (space-separated, Chinese directly), optional
enable_punctuation = true                  # Add punctuation to the final result automatically, default true
punctuation_model = "model.onnx"           # Punctuation model filename (relative to the punctuation model dir)
encoder = "encoder-epoch-99-avg-1.int8.onnx"
decoder = "decoder-epoch-99-avg-1.onnx"    # Official int8 recipe: fp32 decoder
joiner  = "joiner-epoch-99-avg-1.int8.onnx"
tokens  = "tokens.txt"
debug = false
```

</details>

<details>

<summary>🔊 Text-to-Speech (TTS)</summary>

Integrates sherpa-onnx's ZipVoice zero-shot voice-cloning model (bilingual Chinese-English) to synthesize text into wav (offline batch synthesis, no streaming feed).

### Quick Start

```bash
# 1. Download the model (~156 MB: TTS main package + vocoder, installed to ~/.zapmomo/models/<name>, never committed)
cargo run -- tts install-model

# 2. List built-in voices (Lei Jun, a news-anchor female voice, etc.)
cargo run -- tts voices

# 3. Synthesize speech (default voice: Lei Jun; --voice switches built-in voices, --speed adjusts speed)
cargo run -- tts run --text "你好，我是 ZapMomo"
cargo run -- tts run --text "你好" --voice news-female --speed 1.2
```

- **Zero-shot voice cloning** — use a built-in voice via `--voice <name>`, or clone any voice with `--reference-wav <audio>` + `--reference-text <transcript>`
- **Output** — defaults to `~/.zapmomo/tts/<timestamp>.wav`; `--output` to specify a path
- Model source, sha256 verification, and idempotent installation are identical to KWS/ASR (see "Model Source & Verification" above)

### Commands

| Command | Description |
|------|------|
| `tts run` | Synthesize text into wav. `--text` required; `--voice` built-in voice, `--speed` speaking rate, `--reference-wav/--reference-text` custom reference voice, `--output` output path |
| `tts voices` | List built-in voices (parsed from the model package's `test_wavs/prompt.txt`) |
| `tts install-model` | Download and install the TTS main package + vocoder (default `~/.zapmomo/models/<name>`). `--model-dir` to pick a directory, `--force` to reinstall |

### Configuration

Add a `[tts]` section to `~/.zapmomo/settings.toml` to override defaults (all optional):

```toml
[tts]
model_dir = "/path/to/model"              # Model directory (supports ${env.VAR})
encoder = "encoder.int8.onnx"
decoder = "decoder.int8.onnx"
vocoder = "vocos_24khz.onnx"              # Vocoder (downloaded together by install-model)
tokens = "tokens.txt"
lexicon = "lexicon.txt"
data_dir = "espeak-ng-data"
reference_wav = "test_wavs/leijun-1.wav"  # Reference audio for the default voice
reference_text = "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系."  # Transcript of the reference audio
num_steps = 4                             # Diffusion decoding steps (quality/speed trade-off)
speed = 1.0                               # Speaking rate
provider = "cpu"                          # Inference backend, default cpu
num_threads = 2                           # Inference threads
debug = false
```

</details>

<details>

<summary>🧠 Local LLM</summary>

A local LLM based on llama.cpp (Rust bindings `llama-cpp-2`) with streaming chat and agent tool calls; alternatively connect to a remote API or `llama-server` via the OpenAI-compatible `/v1/responses` interface.

LLM models are **GGUF files**: the built-in manifest offers several one-click presets (in the app's "AI Brain (LLM)" page / model library), and you can also drop your own GGUF into `~/.zapmomo/models/<any dir>/` for auto-discovery, or point `[llm] model_path` at it.

### Quick Start

```bash
# 1. Get a model: one-click download in the desktop app's "AI Brain (LLM)" page (Qwen3-0.6B / 4B presets),
#    or download the recommended Qwen3-4B-Instruct-2507 yourself (Q4_K_M, ~2.5 GB) into ~/.zapmomo/models/
# 2. Verify the model loads
cargo run -- llm load

# 3. Single-turn chat (streaming output)
cargo run -- llm chat --text "你好，你是谁？"
```

- **Recommended model** — Qwen3-4B-Instruct-2507 (`Qwen3-4B-Instruct-2507-Q4_K_M.gguf`); any GGUF works and is auto-discovered
- **Backend** — pure CPU by default; Metal acceleration is reserved (`gpu_layers` is configurable; blocked on the llama-cpp-2 0.1.154 Metal logits crash, to be enabled after upgrading the dependency)
- **Agent** — loops provider calls and tool executions until a plain-text reply is produced (max 10 rounds to prevent infinite loops)
- **Remote** — set `base_url / api_key / model` to use an OpenAI-compatible `/v1/responses` endpoint (official API or `llama-server`)

### Commands

| Command | Description |
|------|------|
| `llm load` | Load a model and print its info (architecture / context). `--model-path` to pick the GGUF |
| `llm chat` | Single-turn chat (load + streaming generation). `--text` required, `--model-path` to pick the GGUF |

### Configuration

Add an `[llm]` section to `~/.zapmomo/settings.toml` to override defaults (all optional):

```toml
[llm]
enabled = false                    # Enable or not (the desktop app lazy-loads by default)
provider = "local"                 # local (llama.cpp) | http (OpenAI-compatible)
model_path = "/path/to/model.gguf" # Absolute path to the GGUF (supports ${env.VAR})
system_prompt = "你是 ZapMomo，一个友好的桌面 AI 伙伴。请用简洁自然的中文回答，语气亲切，不要啰嗦。"  # Default system prompt (Chinese)
context_size = 8192                # Context window (tokens)
batch_size = 512                   # Batch size per decode
max_tokens = 512                   # Maximum tokens to generate
temperature = 0.7
top_p = 0.8
top_k = 20
min_p = 0.05
repeat_penalty = 1.05
seed = 0                           # Random seed; 0 = random
threads = 0                        # CPU threads; 0 = auto (physical cores - 2)
gpu_layers = 0                     # Layers offloaded to GPU; -1 = all (Metal), 0 = pure CPU
enable_thinking = false            # Qwen3 thinking mode (emits <think> blocks)
auto_load = false                  # Auto-load the model on app startup
# --- http provider only ---
# base_url = "http://127.0.0.1:8080/v1"  # OpenAI-compatible base URL
# api_key = ""                            # API key (can be empty for a local server)
# model = "qwen3-4b"                      # Model name
```

</details>

<details>

<summary>💬 Voice Session</summary>

Chains KWS / ASR / LLM / TTS into one complete conversation pipeline: **wake word → recognize → think → sentence-level streaming playback**.
sherpa-onnx's TTS only synthesizes whole utterances at once, so "streaming output" is approximated by a sentence-level pipeline: LLM streams tokens → sentence splitting → a dedicated synthesis thread synthesizes sentence by sentence → plays while synthesizing.

- **Wake-word barge-in** — wake-word listening stays active during playback/thinking; saying the wake word again interrupts immediately and returns to listening
- **Hands-free follow-up** — after a reply finishes playing, listening resumes automatically without re-waking

### Quick Start

```bash
# Start a voice session: say the wake word to wake, converse with voice replies, Ctrl-C to exit
cargo run -- voice run
```

### Commands

| Command | Description |
|------|------|
| `voice run` | Run a full voice session (wake → recognize → chat → sentence-level streaming playback). `--keywords` wake words, `--voice` TTS voice, `--speed` speaking rate, `--max-turns` turn limit |

### Configuration

Add a `[voice]` section to `~/.zapmomo/settings.toml` to override defaults (all optional):

```toml
[voice]
enabled = true                # Enter wake-waiting automatically on app startup, default true
keywords = "你好小智"          # Session wake words (Chinese directly, /-separated); defaults to the KWS model's built-ins
voice = "leijun-1"             # TTS voice id for replies
speed = 1.0                    # Playback speaking rate
max_turns = 0                  # Maximum conversation turns; 0 = unlimited (Ctrl-C to exit)
history_max = 12               # Cap on history messages passed to the LLM
barge_in = true                # Wake-word interruption during playback/thinking, default true
follow_up = true               # Auto-listen after a reply finishes (hands-free follow-up), default true
welcome_text = "你好，我在。"  # Welcome message after waking
```

</details>

## Desktop App (Tauri 2)

A desktop GUI reusing the same KWS / ASR / TTS / LLM / Voice / audio / configuration logic, composed of a "Control Panel" + a "Persistent Character Window":

- **Control Panel** — multi-page GUI: **Overview** (current companion and AI capability status), **Chat** (LLM chat with persisted history), **Companion** (import and switch Live2D companions), **Models** (KWS / ASR / LLM / TTS listening, synthesis, chat, and model downloads), **Settings** (microphone device, TTS voice, etc.)
- **Persistent Character Window** — the floating Live2D character, see "Live2D Virtual Character" below
- **Voice Session** — full wake → chat → voice-reply pipeline, see "Voice Session" above

Desktop code lives in `src-tauri/` (frontend is React + Vite + TypeScript). For dev mode and building installers (`pnpm tauri dev` / `pnpm tauri build`) see the [contributing guide](docs/content/docs/contributing/index.mdx) (Chinese).

> Packaged builds ship a built-in "Download models" button: if models are missing on first use, click it in the "Configuration" panel to
> download them automatically to `~/.zapmomo/models/<name>` (works for KWS / ASR / TTS; the CLI
> `zapmomo kws|asr|tts install-model` works too). For unsigned macOS installers see "First Launch on macOS" above.

### One-Click Restart

"Restart" is available in the Settings panel ("General"), the character's context menu, and the tray menu: the app quits and relaunches itself, for configuration changes that only take effect after a restart.

- **Packaged (production)** — works fine: frontend assets are embedded (`asset://`) and load directly after restart.
- **Dev mode (`pnpm tauri dev`)** — the new process **white-screens** after restart (known Tauri issue [tauri#6163](https://github.com/tauri-apps/tauri/issues/6163)); rerun `pnpm tauri dev` manually when you need a restart. See the [contributing guide](docs/content/docs/contributing/index.mdx) (Chinese).

<details>

<summary>🎭 Live2D Virtual Character</summary>

A persistent character window: renders a Live2D character (breathing / blinking auto animations), separate from the settings panel and floating on its own.

- **Persistent & unobtrusive** — move it by dragging with the left button (no focus stealing, no interference with other apps); hidden from Dock / Cmd+Tab on macOS; the native context menu can hide the character / scale it
- **Position memory + percentage scaling** — the position is remembered on close; scaling range 25% ~ 200% (adjustable from the settings panel, `cmd/ctrl + wheel`, and the context menu)
- **Size-adaptive** — the window size adapts to the model's real bounding-box aspect ratio
- **Formats** — supports Cubism 2 / 3 / 4 / 5 (`.model3.json` / `model.json`)
- **Model source** — users provide their own Live2D model directories (not manifest-downloaded), default `~/.zapmomo/models/live2d`; the Cubism Core runtime is versioned with the repo
- **GIF companions** — import a `.gif` file directly from the Companion page
- **Character packs** — import a character-pack directory (`character.md` persona + `character.png` static portrait, plus optional `voice/reference.wav` + `voice/reference.txt` for voice cloning). When active, the persona overrides the global system prompt, and clone-capable TTS models (ZipVoice / OmniVoice) use the character's voice; switching back to another companion restores the global configuration

Add a `[live2d]` section to `~/.zapmomo/settings.toml` to override defaults (all optional):

```toml
[live2d]
model_dir = "/path/to/live2d-model"      # Model root directory (containing .model3.json / model.json)
window_position = { x = 100, y = 100 }   # Character window position memory
window_scale = 1.0                       # Window scaling (0.25 ~ 2.0)
```

</details>

### deepseek-harness Integration (dsh Bridge)

The desktop companion reacts to [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) (dsh) task state in real time: when a task **starts / finishes / fails / is interrupted**, the Live2D character announces it with a speech bubble + voice. It takes one command to connect:

```bash
dsh plugin --profile web add @zapmomo-ai/dsh-plugin
```

Then restart `dsh web` (the "External sensing · dsh bridge" settings section is enabled by default). See the [docs site page](docs/content/docs/desktop-app/dsh-bridge.mdx) (Chinese) for details.

## Contributing

Contributions to ZapMomo are welcome! The following content is for contributors and lives in the docs site (Chinese):

- [Contributing](docs/content/docs/contributing/index.mdx) — dev environment setup, common commands, testing, and the Git workflow
- [Project structure](docs/content/docs/development/project-structure.mdx) — the repo directory tree and each module's responsibilities
- [Dependencies](docs/content/docs/development/dependencies.mdx) — what each crate dependency is used for
- [Release process](docs/content/docs/contributing/release.mdx) — release-plz + tauri-action three-platform automatic builds

## License

[GPL-3.0](LICENSE)
