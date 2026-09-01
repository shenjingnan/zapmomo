# CLAUDE.md - ZapMomo

本文档为 Claude Code 提供项目上下文和开发规范。

## 项目概述

**ZapMomo** 是一个开源的实时桌面 AI 伴侣（An open-source, real-time desktop AI companion with voice, memory, and a customizable virtual character），提供语音交互（KWS 唤醒词）、Tauri 2 桌面 GUI 与通用工具模块。

## 技术栈

| 技术           | 版本  | 用途                         |
| -------------- | ----- | ---------------------------- |
| Rust           | 1.97 | 编程语言 / 编译 / 测试 / Lint / Format |
| clap           | 4.x   | CLI 参数解析                 |
| tokio          | 1.x   | 异步运行时                   |
| sherpa-onnx    | 1.x   | 关键词唤醒词检测（KWS）       |
| serde          | 1.x   | JSON/TOML 序列化/反序列化    |
| tracing        | 0.1   | 日志和诊断                   |
| Tauri          | 2.x   | 桌面应用框架（workspace 成员 `src-tauri/`） |
| React + Vite   | 19.x  | 桌面 GUI 前端（`src-tauri/frontend/`，Tailwind + shadcn/ui） |

## 快速命令参考

```bash
# 开发
cargo run                           # 直接运行（无参进入帮助）
cargo run -- config                 # 显示配置
cargo run -- greet --name World     # 向用户问好
cargo run -- completion bash        # 生成 shell 补全

# KWS 唤醒词
cargo run -- kws test               # 离线检测 wav（需先下载模型）
cargo run -- kws run                # 实时监听麦克风
cargo run -- kws devices            # 列出输入设备

# 测试
cargo test                          # 运行测试
cargo test -- --test-threads=1      # 单线程测试（避免 env 竞争）

# 代码质量
cargo fmt                           # 格式化代码
cargo fmt --check                   # 格式检查
cargo clippy                        # Lint 检查
cargo clippy -- -D warnings         # 严格 Lint 检查
cargo test                          # 测试
cargo fmt --check && cargo clippy -- -D warnings && cargo test   # 完整检查

# 桌面应用（Tauri 2，位于 src-tauri/，path 依赖根 crate 库）
pnpm install                        # 首次：安装 @tauri-apps/cli
pnpm tauri dev                      # 开发模式（KWS 控制面板）
pnpm tauri build                    # 构建当前平台安装包（macOS: .app/.dmg）
cargo check -p zapmomo-app              # 仅检查 tauri crate（Linux 需 webkit 依赖）
cargo clippy -p zapmomo-app -- -D warnings   # tauri crate Lint
scripts/fetch-audiocpp-dev.sh       # 首次跑 tauri dev 前：放置 audio.cpp sidecar（Windows 用 --build，检测到 CUDA Toolkit 自动启用 CUDA 后端）

# 构建
cargo build                         # 调试构建（默认只构建根 CLI crate）
cargo build --release               # 发布构建

# 文档
cargo doc --open                    # 生成并打开 API 文档

# 覆盖率
cargo tarpaulin                     # 生成覆盖率报告
```

## 代码风格规范

由 `cargo fmt` 和 `cargo clippy` 强制执行（Rust Edition 2024）：

- **缩进**: 2 空格
- **行宽**: 最大 100 字符

### 命名约定

| 类型      | 约定                 | 示例           |
| --------- | -------------------- | -------------- |
| 文件      | snake_case           | `my_module.rs` |
| 类/结构体 | PascalCase           | `MyStruct`     |
| 函数/变量 | snake_case           | `my_function`  |
| 常量      | SCREAMING_SNAKE_CASE | `MAX_COUNT`    |
| 类型/trait| PascalCase           | `UserConfig`   |
| 枚举      | PascalCase           | `ModelRole`    |

## 项目结构

```
├── Cargo.toml           # 项目配置和依赖（workspace 根）
├── rust-toolchain.toml  # Rust 工具链版本（1.97.1）
├── src/
│   ├── main.rs          # 入口文件
│   ├── lib.rs           # 库入口 + 测试工具（test_util 临时 HOME 隔离）
│   ├── cli.rs           # CLI 命令定义
│   ├── config/
│   │   ├── mod.rs       # 配置模块入口
│   │   └── settings.rs  # TOML 配置管理（含 [kws] 段）
│   ├── kws/             # 关键词唤醒词检测（sherpa-onnx）
│   │   ├── mod.rs       # KwsEngine + 离线/实时检测
│   │   ├── config.rs    # KWS 配置解析与默认值
│   │   ├── token.rs     # 汉字 → ppinyin token 转换
│   │   └── reaction.rs  # Reaction 可插拔反应（控制台 / GUI / 测试）
│   ├── audio.rs         # cpal 麦克风采集 + 重采样
│   ├── speaker/         # 声纹识别（CAM++ embedding：注册/验证/识别 + JSON 档案）
│   ├── logging.rs       # tracing 双层日志
│   └── datetime.rs      # 日期时间工具
├── models/              # 模型资产（本体不入库，按清单下载）
│   ├── manifest.json    # 模型清单（source / sha256 / license）
│   └── THIRD_PARTY_NOTICES.md
├── src-tauri/           # Tauri 2 桌面应用（workspace 成员）
│   ├── src/lib.rs       # commands + 监听线程 + TauriReaction
│   ├── frontend/        # React + Vite + TypeScript 控制面板（Tailwind + shadcn/ui）
│   ├── tauri.conf.json  # Tauri 配置（打包目标/图标/权限文案）
│   ├── capabilities/    # 权限声明
│   └── icons/           # 应用图标
├── tests/               # 集成测试
├── package.json         # Tauri CLI（@tauri-apps/cli）
├── scripts/             # 模型下载 / 模型测试 / 图标生成等脚本
├── .github/             # CI/CD 配置（含 release.yml 发布流水线）
└── .githooks/           # Git hooks
```

## 发布流程（桌面安装包）

`release-plz` 负责版本/tag/changelog/crates.io；push `vX.Y.Z` tag 后由
`.github/workflows/release.yml`（tauri-action）在 Windows/macOS/Linux 原生 runner
构建安装包并附到草稿 Release。详见 CONTRIBUTING.md「发布流程」。

## 已知限制

- **开发模式重启会白屏**：`pnpm tauri dev` 下点击「重启」（设置页 / 右键菜单 / 托盘）
  后新进程白屏。根因是 Tauri 内置重启不重跑 `beforeDevCommand`，而 `tauri dev`
  在应用退出时拆掉 Vite dev server（[tauri#6163](https://github.com/tauri-apps/tauri/issues/6163)），
  新进程连不上 `localhost:1420`。生产打包版正常。详见 CONTRIBUTING.md「一键重启」。

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
