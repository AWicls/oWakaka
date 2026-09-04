//! oWakaka 库目标：Windows 本地 AI 对话客户端的后端逻辑。
//!
//! 当前进度（P0 · 第二步）：OpenAI chat 兼容 API 通信闭环，默认流式（SSE），非流式保留为兜底。
//! 规划顺序：Slint UI → SQLite 会话持久化（P0）→ RAG（P1）→ A2A（P2）。
//!
//! 运行凭据不进代码库：正式代码读 `config.toml`（`Config::load`），测试读 `config.test.toml`，
//! 模板见 `config.example.toml`。

pub mod ai;
