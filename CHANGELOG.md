# Changelog

## [0.1.24](https://github.com/shenjingnan/zapmomo/compare/v0.1.23...v0.1.24) - 2026-09-02

### Other

- *(ci)* 缓存 audiocpp 源码树与构建目录，脚本级失败重试免 1 小时全量重编

## [0.1.23](https://github.com/shenjingnan/zapmomo/compare/v0.1.22...v0.1.23) - 2026-09-01

### Fixed

- *(ci)* 上游删除 cuda-toolkit v2 滚动 tag，固定到 v0.2.36

## [0.1.22](https://github.com/shenjingnan/zapmomo/compare/v0.1.21...v0.1.22) - 2026-09-01

### Added

- *(tts)* Windows 引擎启用 CUDA，支持运行时 provider 选择 ([#259](https://github.com/shenjingnan/zapmomo/pull/259))

## [0.1.21](https://github.com/shenjingnan/zapmomo/compare/v0.1.20...v0.1.21) - 2026-09-01

### Added

- *(dmg)* 安装包内置「首次打开修复.command」一键修复 macOS「已损坏」拦截 ([#256](https://github.com/shenjingnan/zapmomo/pull/256))

### Other

- 更新首页截图 ([#255](https://github.com/shenjingnan/zapmomo/pull/255))
- *(deps-dev)* bump @testing-library/react from 16.3.2 to 16.3.3 ([#247](https://github.com/shenjingnan/zapmomo/pull/247))
- *(deps-dev)* bump @biomejs/biome from 2.5.10 to 2.5.11 ([#244](https://github.com/shenjingnan/zapmomo/pull/244))
- *(deps)* bump @tanstack/react-query from 5.102.3 to 5.102.8 ([#250](https://github.com/shenjingnan/zapmomo/pull/250))
- *(deps)* bump react-intersection-observer from 11.0.0 to 11.0.1 ([#252](https://github.com/shenjingnan/zapmomo/pull/252))
- *(deps-dev)* bump @types/node from 26.3.0 to 26.4.0 ([#251](https://github.com/shenjingnan/zapmomo/pull/251))
- *(deps)* bump fumadocs-mdx from 15.3.1 to 15.4.0 ([#243](https://github.com/shenjingnan/zapmomo/pull/243))
- *(deps-dev)* bump @vitejs/plugin-react from 6.1.0 to 6.1.1 ([#248](https://github.com/shenjingnan/zapmomo/pull/248))

## [0.1.20](https://github.com/shenjingnan/zapmomo/compare/v0.1.19...v0.1.20) - 2026-08-31

### Added

- *(voice)* 语音会话开关迁移设置页、状态收敛与麦克风互斥 ([#253](https://github.com/shenjingnan/zapmomo/pull/253))
- *(storage)* 首次下载引导选存储位置 + 下载/导入前磁盘空间校验 ([#242](https://github.com/shenjingnan/zapmomo/pull/242))
- *(dsh)* 桥启停跟随插件安装状态，移除手动开关并新增卸载入口 ([#240](https://github.com/shenjingnan/zapmomo/pull/240))
- *(companion)* 音色行内嵌播放条与文字按钮，移除音色库下拉 ([#239](https://github.com/shenjingnan/zapmomo/pull/239))
- *(voice)* 形象切换 subagent——回复结束后台单次调用自动决策切形象 ([#234](https://github.com/shenjingnan/zapmomo/pull/234))
- *(companion)* 右键菜单「表演」改为「状态切换」，表演功能下线 ([#235](https://github.com/shenjingnan/zapmomo/pull/235))
- *(speaker)* 声纹识别完整落地——CAM++ 声纹引擎、GUI 注册管理、响应门控 ([#233](https://github.com/shenjingnan/zapmomo/pull/233))
- *(bubble)* 语音对话 ASR 识别逐字上屏聊天气泡（流式 partial 聆听通道） ([#232](https://github.com/shenjingnan/zapmomo/pull/232))
- *(integrations)* 插件集成页——dsh 集成插件化（检测/一键安装/心跳在线） ([#231](https://github.com/shenjingnan/zapmomo/pull/231))
- *(voice)* ASR 说完判定接入 Silero VAD 门控，超长句强制断句兜底 ([#230](https://github.com/shenjingnan/zapmomo/pull/230))
- *(llm)* 工具调用结果跨轮回传 LLM(history 持久化工具轮) ([#229](https://github.com/shenjingnan/zapmomo/pull/229))
- *(companion)* 拖动角色窗口时聊天气泡联动跟随（macOS 子窗口丝滑方案） ([#227](https://github.com/shenjingnan/zapmomo/pull/227))
- *(voice)* 播报中的语音打断（ASR barge-in）——说话即打断进入新一轮 ([#226](https://github.com/shenjingnan/zapmomo/pull/226))
- *(companion)* 每角色独立音色，三级解析（目录自带 > 音色库绑定 > 全局默认） ([#225](https://github.com/shenjingnan/zapmomo/pull/225))
- *(dsh)* 事件播报接入 LLM 文案生成（LLM 总结→气泡→TTS 播报） ([#224](https://github.com/shenjingnan/zapmomo/pull/224))
- *(companion)* LLM 工具切换角色形象（set_character_sprite） ([#223](https://github.com/shenjingnan/zapmomo/pull/223))
- *(models)* 移除 8 个 ASR 模型，模型库精简为默认 Zipformer + Qwen3-ASR (audio.cpp) ([#222](https://github.com/shenjingnan/zapmomo/pull/222))
- *(bubble)* 聊天气泡展示用户输入，先用户句后角色回复 ([#220](https://github.com/shenjingnan/zapmomo/pull/220))
- *(chatbox)* 输入条缺省改为显示，首次启动随角色一同出现 ([#219](https://github.com/shenjingnan/zapmomo/pull/219))
- *(companion)* 角色窗口智能穿透（按光标位置动态切换） ([#217](https://github.com/shenjingnan/zapmomo/pull/217))
- *(tts)* 合成前清洗 markdown/emoji 等无意义符号，提升播报速度与准确性 ([#216](https://github.com/shenjingnan/zapmomo/pull/216))
- *(bubble)* 聊天气泡改为手动点击关闭，不再自动消失 ([#215](https://github.com/shenjingnan/zapmomo/pull/215))

### Fixed

- *(release)* 安装说明提取适配 README「应用下载」新锚点 ([#228](https://github.com/shenjingnan/zapmomo/pull/228))

### Other

- *(deps)* bump fumadocs-ui from 16.15.1 to 16.15.4 ([#246](https://github.com/shenjingnan/zapmomo/pull/246))
- *(deps)* bump fumadocs-core from 16.15.1 to 16.15.4 ([#245](https://github.com/shenjingnan/zapmomo/pull/245))
- *(deps)* bump next from 16.3.2 to 16.3.3 ([#249](https://github.com/shenjingnan/zapmomo/pull/249))
- 清理根目录设计文档，归档仍被引用方案至 docs/plans ([#238](https://github.com/shenjingnan/zapmomo/pull/238))
- *(readme)* 新增免费 LLM 推荐表格（智谱 glm-4.7-flash） ([#237](https://github.com/shenjingnan/zapmomo/pull/237))
- *(dev)* .taurignore 排除 frontend 与纯文档目录，修复 dev 监听风暴 ([#221](https://github.com/shenjingnan/zapmomo/pull/221))
- *(readme)* 简化下载按钮为系统图标 + 立即下载 ([#212](https://github.com/shenjingnan/zapmomo/pull/212))
- *(asr)* 移除三个纯英文 Streaming Zipformer ASR 模型 ([#206](https://github.com/shenjingnan/zapmomo/pull/206))

## [0.1.19](https://github.com/shenjingnan/zapmomo/compare/v0.1.18...v0.1.19) - 2026-08-28

### Added

- *(companion)* 伙伴支持独立的尺寸与位置配置 ([#195](https://github.com/shenjingnan/zapmomo/pull/195))
- *(llm)* 接入 Anthropic Messages API，新增 CLI 工具与 prompt caching/thinking ([#197](https://github.com/shenjingnan/zapmomo/pull/197))
- *(bubble)* 聊天气泡独立窗口化——统一回复与 dsh 播报，恒高于角色层级 ([#201](https://github.com/shenjingnan/zapmomo/pull/201))
- *(asr)* 接入 audio.cpp Qwen3-ASR 0.6B 后端（Metal 加速） ([#199](https://github.com/shenjingnan/zapmomo/pull/199))
- *(tts)* 接入 Qwen3-TTS 0.6B/1.7B 音色克隆（audio.cpp 后端） ([#192](https://github.com/shenjingnan/zapmomo/pull/192))
- *(companion)* 新增 galgame 风格文字输入条与回复气泡，修复角色显示问题 ([#182](https://github.com/shenjingnan/zapmomo/pull/182))
- *(companion)* 支持角色包导入（静态立绘 + 人设覆盖 + 音色克隆） ([#181](https://github.com/shenjingnan/zapmomo/pull/181))
- *(llm)* 远程 LLM provider 改用 OpenAI 兼容 Chat Completions ([#180](https://github.com/shenjingnan/zapmomo/pull/180))
- *(tts)* omnivoice SSE 流式分块合成（语音会话首响优化） ([#163](https://github.com/shenjingnan/zapmomo/pull/163))
- *(tts)* 接入 OmniVoice 声音克隆模型与 TTS 句间热切换 ([#162](https://github.com/shenjingnan/zapmomo/pull/162))
- *(tts)* 接入 audio.cpp sidecar 引擎与 PocketTTS 英文模型 ([#160](https://github.com/shenjingnan/zapmomo/pull/160))
- *(companion)* GIF 动图作为桌面伙伴（与 Live2D 并存） ([#159](https://github.com/shenjingnan/zapmomo/pull/159))
- *(asr)* 接入 Qwen3-ASR 离线模型族（29 语言 + 热词） ([#157](https://github.com/shenjingnan/zapmomo/pull/157))
- *(tts)* 支持 Kokoro TTS（103 音色，int8/fp32 双变体） ([#156](https://github.com/shenjingnan/zapmomo/pull/156))
- *(kws)* 接入 GigaSpeech 英文唤醒词模型，补齐 sherpa-onnx 官方 KWS 全家桶 ([#155](https://github.com/shenjingnan/zapmomo/pull/155))
- *(dsh)* 桌宠事件气泡升级为 iOS 风格堆叠 toast ([#154](https://github.com/shenjingnan/zapmomo/pull/154))
- *(asr)* 新增流式 Paraformer ASR 模型族（中英/中粤英） ([#153](https://github.com/shenjingnan/zapmomo/pull/153))
- *(asr)* 支持离线 ASR（SenseVoice/Whisper）与语音会话模型族自适应 ([#152](https://github.com/shenjingnan/zapmomo/pull/152))

### Fixed

- *(asr)* 切回 streaming zipformer 后误报「模型文件缺失」及切换后白屏 ([#203](https://github.com/shenjingnan/zapmomo/pull/203))
- *(llm)* 修复 CLI 在 tokio 上下文调用 provider 时 panic ([#198](https://github.com/shenjingnan/zapmomo/pull/198))
- *(scripts)* 修正 audio.cpp sidecar 构建的 tag 名与 macOS OpenMP 处理 ([#161](https://github.com/shenjingnan/zapmomo/pull/161))
- *(dsh)* 补全 dsh-plugin repository 字段修复 npm provenance 发布 ([#150](https://github.com/shenjingnan/zapmomo/pull/150))

### Other

- *(deps-dev)* bump @types/node from 26.2.0 to 26.3.0 ([#208](https://github.com/shenjingnan/zapmomo/pull/208))
- *(deps)* bump base64 from 0.22.1 to 0.23.1 ([#184](https://github.com/shenjingnan/zapmomo/pull/184))
- *(deps)* bump futures-util from 0.3.32 to 0.3.34 ([#183](https://github.com/shenjingnan/zapmomo/pull/183))
- *(deps-dev)* bump vite from 8.2.1 to 8.2.2 ([#175](https://github.com/shenjingnan/zapmomo/pull/175))
- *(deps)* bump sherpa-onnx from 1.13.5 to 1.13.6 ([#207](https://github.com/shenjingnan/zapmomo/pull/207))
- *(deps)* bump @tanstack/react-query from 5.102.1 to 5.102.3 ([#209](https://github.com/shenjingnan/zapmomo/pull/209))
- *(deps-dev)* bump @types/react-dom from 19.2.4 to 19.2.5 ([#210](https://github.com/shenjingnan/zapmomo/pull/210))
- *(deps)* bump lucide-react from 1.33.0 to 1.34.0 ([#211](https://github.com/shenjingnan/zapmomo/pull/211))
- *(ci)* 屏蔽 windows crate 的 minor/major 升级 ([#205](https://github.com/shenjingnan/zapmomo/pull/205))
- *(deps)* bump fumadocs-core from 16.15.0 to 16.15.1 ([#185](https://github.com/shenjingnan/zapmomo/pull/185))
- *(tts)* 移除 VITS Melo / Matcha / Kokoro / PocketTTS 模型 ([#204](https://github.com/shenjingnan/zapmomo/pull/204))
- *(deps)* bump fumadocs-ui from 16.15.0 to 16.15.1 ([#190](https://github.com/shenjingnan/zapmomo/pull/190))
- *(deps)* bump @tanstack/react-query from 5.101.4 to 5.102.1 ([#188](https://github.com/shenjingnan/zapmomo/pull/188))
- *(deps-dev)* bump @testing-library/user-event from 14.6.5 to 14.6.6 ([#189](https://github.com/shenjingnan/zapmomo/pull/189))
- *(deps)* bump sysinfo from 0.33.1 to 0.39.6 ([#166](https://github.com/shenjingnan/zapmomo/pull/166))
- *(deps)* bump fumadocs-mdx from 15.3.0 to 15.3.1 ([#191](https://github.com/shenjingnan/zapmomo/pull/191))
- *(deps-dev)* bump @biomejs/biome from 2.5.9 to 2.5.10 ([#187](https://github.com/shenjingnan/zapmomo/pull/187))
- *(deps)* bump next from 16.3.1 to 16.3.2 ([#186](https://github.com/shenjingnan/zapmomo/pull/186))
- *(models)* 移除模型库页面及 HF 在线目录能力 ([#196](https://github.com/shenjingnan/zapmomo/pull/196))
- *(kws)* 移除 wenetspeech / gigaspeech 唤醒词模型，仅保留中英双语 zh-en ([#194](https://github.com/shenjingnan/zapmomo/pull/194))
- *(llm)* 移除 llama.cpp 本地模型，LLM 改为纯远程连接 ([#193](https://github.com/shenjingnan/zapmomo/pull/193))
- *(deps)* bump fumadocs-core from 16.14.4 to 16.15.0 ([#171](https://github.com/shenjingnan/zapmomo/pull/171))
- *(deps)* bump lucide-react from 1.31.0 to 1.33.0 ([#173](https://github.com/shenjingnan/zapmomo/pull/173))
- *(deps)* bump tauri-nspanel from `a3122e8` to `c9ec213` ([#168](https://github.com/shenjingnan/zapmomo/pull/168))
- *(deps)* bump cpal from 0.18.1 to 0.18.2 ([#169](https://github.com/shenjingnan/zapmomo/pull/169))
- *(deps-dev)* bump @vitejs/plugin-react from 6.0.5 to 6.1.0 ([#170](https://github.com/shenjingnan/zapmomo/pull/170))
- *(deps-dev)* bump @biomejs/biome from 2.5.8 to 2.5.9 ([#172](https://github.com/shenjingnan/zapmomo/pull/172))
- *(deps)* bump pixi.js from 6.5.10 to 8.20.0 ([#174](https://github.com/shenjingnan/zapmomo/pull/174))
- *(deps-dev)* bump vitest from 4.1.10 to 4.1.11 ([#176](https://github.com/shenjingnan/zapmomo/pull/176))
- *(deps)* bump fumadocs-ui from 16.14.4 to 16.15.0 ([#177](https://github.com/shenjingnan/zapmomo/pull/177))
- *(deps-dev)* bump @testing-library/user-event from 14.6.4 to 14.6.5 ([#178](https://github.com/shenjingnan/zapmomo/pull/178))
- *(deps)* bump fumadocs-mdx from 15.2.3 to 15.3.0 ([#179](https://github.com/shenjingnan/zapmomo/pull/179))

## [0.1.18](https://github.com/shenjingnan/zapmomo/compare/v0.1.17...v0.1.18) - 2026-08-21

### Added

- *(docs)* 文档站首页 hero 落地页（介绍 + 三平台下载） ([#147](https://github.com/shenjingnan/zapmomo/pull/147))
- *(dsh)* 插件源码与 npm 发布 CI（integrations/dsh-plugin + OIDC workflow） ([#145](https://github.com/shenjingnan/zapmomo/pull/145))
- *(tts)* 支持多 TTS 模型（VITS/Matcha）与模型类型建模 ([#144](https://github.com/shenjingnan/zapmomo/pull/144))
- *(app)* BongoCat 兼容键鼠模拟表演系统 ([#143](https://github.com/shenjingnan/zapmomo/pull/143))
- *(dsh)* deepseek-harness 任务状态驱动桌宠播报（HTTP 桥 + 气泡/语音 + 设置页） ([#142](https://github.com/shenjingnan/zapmomo/pull/142))
- *(tts)* 模型与能力支持 TTS「选择模型」弹窗切换 ([#141](https://github.com/shenjingnan/zapmomo/pull/141))

### Fixed

- *(app)* 修复 Windows release 构建失败并给 CI 补 windows 编译腿 ([#139](https://github.com/shenjingnan/zapmomo/pull/139))

### Other

- *(dsh)* 新增 deepseek-harness 集成使用文档 ([#149](https://github.com/shenjingnan/zapmomo/pull/149))
- *(dsh)* deepseek-harness 集成（dsh 桥）用户文档与 SOCKS 代理支持 ([#148](https://github.com/shenjingnan/zapmomo/pull/148))
- *(dsh)* 插件发布跟随 zapmomo 主版本 tag（v*），免单独打插件 tag ([#146](https://github.com/shenjingnan/zapmomo/pull/146))

## [0.1.17](https://github.com/shenjingnan/zapmomo/compare/v0.1.16...v0.1.17) - 2026-08-21

### Added

- *(app)* 桌宠支持修饰键拖拽模式（按住 cmd/ctrl 才能拖动） ([#137](https://github.com/shenjingnan/zapmomo/pull/137))
- *(app)* 支持开机自启动，托盘/设置页双入口切换 + 单实例防护 ([#136](https://github.com/shenjingnan/zapmomo/pull/136))
- *(asr)* 模型与能力支持 ASR「选择模型」弹窗切换 ([#134](https://github.com/shenjingnan/zapmomo/pull/134))
- *(ci)* 百度网盘上传附带更新说明与安装说明文本 ([#133](https://github.com/shenjingnan/zapmomo/pull/133))
- *(app)* 桌宠支持位置锁定，右键/托盘/设置页三入口切换 ([#131](https://github.com/shenjingnan/zapmomo/pull/131))

### Fixed

- *(ci)* Release 发布改用 PAT，触发百度网盘上传 workflow ([#129](https://github.com/shenjingnan/zapmomo/pull/129))

## [0.1.16](https://github.com/shenjingnan/zapmomo/compare/v0.1.15...v0.1.16) - 2026-08-21

### Added

- *(app)* 桌宠支持置顶/置底层级切换并加入右键与托盘菜单 ([#128](https://github.com/shenjingnan/zapmomo/pull/128))
- 桌宠角色窗口支持点击穿透 ([#126](https://github.com/shenjingnan/zapmomo/pull/126))
- *(kws)* 支持多模型下载与切换，文件名未配置时自动探测 ([#125](https://github.com/shenjingnan/zapmomo/pull/125))
- *(app)* 设置页支持自定义全局快捷键 ([#122](https://github.com/shenjingnan/zapmomo/pull/122))

### Fixed

- *(app)* 统一应用显示名为 ZapMomo ([#124](https://github.com/shenjingnan/zapmomo/pull/124))

### Other

- *(readme)* 新增英文 README 并支持中英文切换 ([#119](https://github.com/shenjingnan/zapmomo/pull/119))

## [0.1.15](https://github.com/shenjingnan/zapmomo/compare/v0.1.14...v0.1.15) - 2026-08-20

### Added

- *(ui)* 弹窗外壳迁移 shadcn/ui Dialog（Radix）并修复宽度回归 ([#110](https://github.com/shenjingnan/zapmomo/pull/110))
- *(storage)* 支持自定义模型数据目录（data_dir） ([#108](https://github.com/shenjingnan/zapmomo/pull/108))
- *(companion)* 伙伴页 Live2D 动作/表情展示与预览 ([#109](https://github.com/shenjingnan/zapmomo/pull/109))
- *(release)* 资产名去除版本号，README 提供一键下载最新版按钮 ([#106](https://github.com/shenjingnan/zapmomo/pull/106))
- *(llm)* 内置 LLM 预设一键下载，Release 制品自动上传百度网盘 ([#105](https://github.com/shenjingnan/zapmomo/pull/105))
- *(release)* 安装包资产重命名为平台友好命名并在 Release 说明提供下载引导表格 ([#104](https://github.com/shenjingnan/zapmomo/pull/104))

### Fixed

- *(tauri)* Windows 下角色窗口不再显示应用菜单栏 ([#102](https://github.com/shenjingnan/zapmomo/pull/102))

### Other

- *(readme)* README 重写为用户视角，开发向内容迁移至 CONTRIBUTING.md ([#116](https://github.com/shenjingnan/zapmomo/pull/116))
- *(ci)* 拆分覆盖率 job 并修复 rust-cache 缓存失效 ([#114](https://github.com/shenjingnan/zapmomo/pull/114))
- *(readme)* 头部新增居中徽标（版本/CI/覆盖率/许可/平台） ([#115](https://github.com/shenjingnan/zapmomo/pull/115))
- *(readme)* 开发向内容迁移文档站，新增贡献指南 ([#112](https://github.com/shenjingnan/zapmomo/pull/112))
- *(llm)* 补充 LLM 预设一键下载测试用例 ([#107](https://github.com/shenjingnan/zapmomo/pull/107))
- *(ci)* Node 版本升级 22 → 24，.nvmrc 替换为 .node-version ([#101](https://github.com/shenjingnan/zapmomo/pull/101))
- *(ci)* 用 rust-cache 缓存 cargo 构建产物 ([#99](https://github.com/shenjingnan/zapmomo/pull/99))

## [0.1.14](https://github.com/shenjingnan/zapmomo/compare/v0.1.13...v0.1.14) - 2026-08-19

### Added

- *(live2d)* 模型透明度调节（右键菜单 / 托盘 / 伙伴页 / 概览） ([#97](https://github.com/shenjingnan/zapmomo/pull/97))
- 一键重启应用（设置页 / 角色右键菜单 / 托盘菜单） ([#96](https://github.com/shenjingnan/zapmomo/pull/96))
- 对话记录持久化 + TTS 默认音色选择 ([#94](https://github.com/shenjingnan/zapmomo/pull/94))
- *(voice)* 语音会话编排器（KWS→ASR→LLM→TTS 句级流式 + 唤醒词打断） ([#93](https://github.com/shenjingnan/zapmomo/pull/93))
- *(companion)* 伙伴模型管理器 ([#85](https://github.com/shenjingnan/zapmomo/pull/85))
- *(models)* 新增模型库 ([#84](https://github.com/shenjingnan/zapmomo/pull/84))
- *(tts)* 重构配置页、新增音色库与高级参数，修复合成崩溃 ([#71](https://github.com/shenjingnan/zapmomo/pull/71))
- *(models)* 重构 ASR 配置页并支持高级参数，模型摘要整行进入配置页 ([#70](https://github.com/shenjingnan/zapmomo/pull/70))
- *(kws)* 重构唤醒词配置页并支持启用后自动监听 ([#69](https://github.com/shenjingnan/zapmomo/pull/69))
- *(llm)* 重构 LLM 配置页并开放参数 UI 配置 ([#67](https://github.com/shenjingnan/zapmomo/pull/67))
- *(models)* 重设计 /models 模型概览页为能力链路 + 模型摘要 ([#66](https://github.com/shenjingnan/zapmomo/pull/66))
- *(app-shell)* 重构主界面外壳与侧边栏，新增 Dock 图标隐藏设置 ([#54](https://github.com/shenjingnan/zapmomo/pull/54))

### Fixed

- *(deps)* downgrade pixi.js to 6.5.10 to restore Live2D rendering ([#91](https://github.com/shenjingnan/zapmomo/pull/91))
- *(download)* 删除 enqueue 中遮蔽确定性视图的残留 view_of 调用 ([#90](https://github.com/shenjingnan/zapmomo/pull/90))
- *(download)* enqueue 返回值改为入队时刻的确定性视图 ([#88](https://github.com/shenjingnan/zapmomo/pull/88))

### Other

- README 补充概览页截图并同步 Voice/多页面 GUI 现状 ([#95](https://github.com/shenjingnan/zapmomo/pull/95))
- *(deps)* bump sha2 from 0.10.9 to 0.11.0 ([#76](https://github.com/shenjingnan/zapmomo/pull/76))
- *(license)* 切换许可证为 GPL-3.0 ([#87](https://github.com/shenjingnan/zapmomo/pull/87))
- *(deps)* bump react-router-dom from 6.30.4 to 7.18.2 ([#81](https://github.com/shenjingnan/zapmomo/pull/81))
- *(deps)* bump ureq from 2.12.1 to 3.4.0 ([#77](https://github.com/shenjingnan/zapmomo/pull/77))
- *(deps)* bump fumadocs-ui from 16.14.3 to 16.14.4 ([#78](https://github.com/shenjingnan/zapmomo/pull/78))
- *(deps)* bump fumadocs-core from 16.14.3 to 16.14.4 ([#80](https://github.com/shenjingnan/zapmomo/pull/80))
- *(deps)* bump dtolnay/rust-toolchain from 1.97.1 to 1.100.0 ([#73](https://github.com/shenjingnan/zapmomo/pull/73))
- *(deps)* bump softprops/action-gh-release from 2 to 3 ([#74](https://github.com/shenjingnan/zapmomo/pull/74))
- *(deps)* bump bzip2 from 0.4.4 to 0.6.1 ([#75](https://github.com/shenjingnan/zapmomo/pull/75))
- *(deps)* bump reqwest from 0.12.28 to 0.13.4 ([#79](https://github.com/shenjingnan/zapmomo/pull/79))
- *(deps)* bump next from 16.3.0 to 16.3.1 ([#82](https://github.com/shenjingnan/zapmomo/pull/82))
- *(deps)* bump pixi.js from 6.5.10 to 8.19.0 ([#83](https://github.com/shenjingnan/zapmomo/pull/83))

## [0.1.13](https://github.com/shenjingnan/zapmomo/compare/v0.1.12...v0.1.13) - 2026-08-16

### Other

- *(release)* 让桌面 App 改动也能触发版本发布 ([#62](https://github.com/shenjingnan/zapmomo/pull/62))

## [0.1.12](https://github.com/shenjingnan/zapmomo/compare/v0.1.11...v0.1.12) - 2026-08-16

### Fixed

- *(build)* 修复 macOS llama.cpp 编译失败，声明最低系统版本 13.7.8 ([#59](https://github.com/shenjingnan/zapmomo/pull/59))

### Other

- *(readme)* 补充 TTS/LLM/Live2D 功能文档 ([#57](https://github.com/shenjingnan/zapmomo/pull/57))
- *(license)* 切换许可证为 Apache-2.0 ([#56](https://github.com/shenjingnan/zapmomo/pull/56))

## [0.1.11](https://github.com/shenjingnan/zapmomo/compare/v0.1.10...v0.1.11) - 2026-08-16

### Added

- *(llm)* 集成 llama.cpp 本地大语言模型 ([#52](https://github.com/shenjingnan/zapmomo/pull/52))
- *(tts)* 集成 ZipVoice 文本转语音与音色选择 ([#50](https://github.com/shenjingnan/zapmomo/pull/50))

### Fixed

- *(build)* 将 Live2D Cubism Core 运行时纳入版本管理 ([#55](https://github.com/shenjingnan/zapmomo/pull/55))

## [0.1.10](https://github.com/shenjingnan/zapmomo/compare/v0.1.9...v0.1.10) - 2026-08-15

### Added

- *(live2d)* 角色窗口拖动不抢焦点并从 Dock/Cmd+Tab 隐形 ([#49](https://github.com/shenjingnan/zapmomo/pull/49))
- *(live2d)* 角色窗口位置记忆与百分比缩放 ([#48](https://github.com/shenjingnan/zapmomo/pull/48))
- *(live2d)* 集成 Live2D 模型加载与预览 ([#45](https://github.com/shenjingnan/zapmomo/pull/45))

### Other

- 使用原生 SVG 替换 favicon 内嵌 PNG ([#42](https://github.com/shenjingnan/zapmomo/pull/42))

## [0.1.9](https://github.com/shenjingnan/zapmomo/compare/v0.1.8...v0.1.9) - 2026-08-14

### Added

- *(asr)* 集成 sherpa-onnx 流式语音识别（中英双语） ([#40](https://github.com/shenjingnan/zapmomo/pull/40))

## [0.1.8](https://github.com/shenjingnan/zapmomo/compare/v0.1.7...v0.1.8) - 2026-08-14

### Added

- *(app)* macOS 窗口改用原生阴影并优化启动体验 ([#38](https://github.com/shenjingnan/zapmomo/pull/38))
- *(kws)* 英文关键词自动转 ARPAbet 音素 ([#36](https://github.com/shenjingnan/zapmomo/pull/36))

## [0.1.7](https://github.com/shenjingnan/zapmomo/compare/v0.1.6...v0.1.7) - 2026-08-13

### Fixed

- *(ci)* 修复 Release 工作流漏装 frontend 依赖导致构建失败 ([#34](https://github.com/shenjingnan/zapmomo/pull/34))

## [0.1.6](https://github.com/shenjingnan/zapmomo/compare/v0.1.5...v0.1.6) - 2026-08-13

### Added

- *(app)* 前端迁移到 React 并升级为无边框透明窗口 ([#31](https://github.com/shenjingnan/zapmomo/pull/31))

## [0.1.5](https://github.com/shenjingnan/zapmomo/compare/v0.1.4...v0.1.5) - 2026-08-13

### Other

- 更新 README，清理过时内容 ([#29](https://github.com/shenjingnan/zapmomo/pull/29))

## [0.1.4](https://github.com/shenjingnan/zapmomo/compare/v0.1.3...v0.1.4) - 2026-08-13

### Other

- *(ci)* Release 构建成功后自动发布，不再停留在草稿

## [0.1.3](https://github.com/shenjingnan/zapmomo/compare/v0.1.2...v0.1.3) - 2026-08-13

### Other

- 添加 ZapMomo logo 与 favicon ([#25](https://github.com/shenjingnan/zapmomo/pull/25))

## [0.1.2](https://github.com/shenjingnan/zapmomo/compare/v0.1.1...v0.1.2) - 2026-08-13

### Added

- *(kws)* 内置模型自动下载并修复打包路径失效 ([#23](https://github.com/shenjingnan/zapmomo/pull/23))

## [0.1.1](https://github.com/shenjingnan/zapmomo/compare/v0.1.0...v0.1.1) - 2026-08-13

### Added

- *(docs)* 新增 Fumadocs 中文文档站并部署到 Cloudflare Pages ([#10](https://github.com/shenjingnan/zapmomo/pull/10))

### Fixed

- *(ci)* 修复 release-plz 创建 Release PR 使用 PAT_TOKEN ([#21](https://github.com/shenjingnan/zapmomo/pull/21))

### Other

- *(deps)* bump cpal from 0.15.3 to 0.18.1 ([#14](https://github.com/shenjingnan/zapmomo/pull/14))
- *(deps)* bump actions/download-artifact from 4 to 8 ([#12](https://github.com/shenjingnan/zapmomo/pull/12))
- *(deps)* bump actions/cache from 4 to 6 ([#13](https://github.com/shenjingnan/zapmomo/pull/13))
- *(deps)* bump actions/setup-node from 4 to 7 ([#15](https://github.com/shenjingnan/zapmomo/pull/15))
- *(deps)* bump pnpm/action-setup from 4 to 6 ([#16](https://github.com/shenjingnan/zapmomo/pull/16))
- *(deps-dev)* bump @types/node from 22.20.1 to 26.2.0 ([#17](https://github.com/shenjingnan/zapmomo/pull/17))
- *(deps)* bump pinyin from 0.10.0 to 0.11.0 ([#18](https://github.com/shenjingnan/zapmomo/pull/18))
- *(deps-dev)* bump typescript from 5.9.3 to 7.0.2 ([#19](https://github.com/shenjingnan/zapmomo/pull/19))
- *(docs)* 部署切换为 Cloudflare Pages Git 集成并修复过期链接 ([#20](https://github.com/shenjingnan/zapmomo/pull/20))
- 从 npm 迁移到 pnpm ([#9](https://github.com/shenjingnan/zapmomo/pull/9))

## [0.1.0] - 2026-06-05

### Added

- 项目初始化
- CLI 骨架（clap + tokio）
- 配置管理（TOML 配置读写）
- 双层日志系统（tracing）
- 日期时间工具模块
- CI/CD 配置（GitHub Actions）
- 代码质量工具（fmt, clippy, typos, tarpaulin, codecov）
- Shell 补全生成
