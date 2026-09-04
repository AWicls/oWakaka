# oWakaka

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Rust Edition](https://img.shields.io/badge/edition-2024-orange)

一个基于 Rust 的 Windows 本地 AI 对话客户端，同时也是一个进行中的学习项目：**探索如何通过合理控制 AI agent 的成本与节奏，把软件架构从一个想法增量地做到可用**。

技术栈贯穿 Rust 原生：[Slint](https://slint.dev) 声明式 UI + tokio/reqwest 异步通信 + SQLite 持久化（规划中）。

## 当前能力（v0.2.0）

- **对话通信**：OpenAI Chat Completions 兼容层，可对接任何提供 `/chat/completions` 的网关（OpenAI 官方、OneAPI、vLLM、国产兼容层等）
- **流式优先**：SSE 流式响应为默认路径，逐字回显；非流式保留为兜底
- **思考过程展示**：推理模型的 `reasoning_content` 增量以独立面板呈现，支持 收起 / 部分（最多 5 行自适应）/ 全开 三态，自动跟随 + 自由回看
- **聊天界面**：3:2 窗口、微信式左右气泡、气泡宽度内容自适应（≤82%）、多行输入框（自适应 ≤8 行，Ctrl+Enter 发送）、消息自动粘底、回答一键复制
- **健壮性**：错误分层（传输失败 / 服务端非 2xx / 响应形状异常）且原始信息不丢；密钥全程脱敏（Debug 输出恒 `[REDACTED]`）

## 快速开始

### 环境

- Windows（当前不考虑跨平台）
- Rust stable（edition 2024）

### 配置

```bash
cp config.example.toml config.toml
```

编辑 `config.toml`：

```toml
base_url = "https://api.openai.com/v1"   # 任意 chat 兼容网关
api_key  = "sk-..."
model    = "gpt-4o-mini"
```

> `config.toml`（正式）与 `config.test.toml`（测试凭据）均已在 `.gitignore` 中，密钥不进仓库。

### 运行

```bash
cargo run
```

### 测试

```bash
cargo test              # 离线：单元测试 + 文档测试（全部可离线运行）
cargo test -- --ignored # 联网：真实网关往返测试（需 config.test.toml）
```

## 项目结构

```text
src/
├── lib.rs            # 库目标（业务逻辑，doctest 依托）
├── main.rs           # 可执行入口：纯转发 ui::run()
├── ui.rs             # Slint 对话壳：线程模型、气泡模型、剪贴板
└── ai/
    ├── client.rs     # OpenAI chat 兼容客户端（非流式 + SSE 流式 + StreamEvent）
    ├── config.rs     # 配置与通用 TOML 读写工具（load_toml/store_toml）
    └── dto/          # 线格式 DTO（公共兼容层）
        ├── openai_chat/    # 请求 / 响应 / SSE 分块
        └── openai_response/ # Responses API（占位，未启用）
ui/app.slint          # 窗口、气泡、输入区声明式定义
```

设计约定：DTO 只描述数据形状，客户端只管传输与错误映射；自定义字段经 `#[serde(flatten)]` 平铺透传，兼容各家网关扩展。

## 路线图

| 阶段 | 内容 | 状态 |
|---|---|---|
| **P0** 干净对话客户端 | OpenAI 兼容通信 → 流式 → Slint UI 壳 | ✅ 基本完成（v0.2.0） |
| P0 收尾 | SQLite 会话持久化、浅深色主题自适应、交互打磨 | 🔜 |
| P1 外挂知识库 RAG | embedding / ranker / 文档解析全部走外部 API | ⬜ |
| **P2** 生态接入 | **A2A**：与其他 agent 工具通话并协作完成任务；**MCP**：以客户端/宿主为主，让对话接入外部工具服务器（文件、搜索等），对外暴露 MCP 服务端为后置扩展 | ⬜ |

Anthropic 及定制化厂家 API 按 P0 约定的顺序接入：一家实现不预建 trait，第二家接入时再提炼抽象。

## 开发方式

本项目以「小步、准、可验证」的增量方式推进：每个改动先对齐*目标 / 最小验收标准 / 明确不做的事*，一次交付一个可运行的垂直切片；改动一个 commit（Conventional Commits + 中文描述），版本遵循 SemVer，详见 [CHANGELOG](CHANGELOG.md) 与 [.github/instructions](.github/instructions/)。

## License

[MIT](LICENSE)
