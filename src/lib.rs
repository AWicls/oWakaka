//! oWakaka 库目标：Windows 本地 AI 对话客户端的后端逻辑。
//!
//! 当前进度（P0 · 第一步）：OpenAI chat 兼容 API 的非流式通信闭环。
//! 规划顺序：流式响应 → Slint UI → SQLite 会话持久化（P0）→ RAG（P1）→ A2A（P2）。
//!
//! 运行凭据放项目根目录 `config.local.toml`（模板见 `config.example.toml`），不进代码库。

pub mod ai;
