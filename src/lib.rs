//! oWakaka 库目标：Windows 本地 AI 对话客户端的后端逻辑。
//!
//! 当前进度（P0 · 第三步）：OpenAI chat 兼容通信（默认流式 SSE）+ Slint 对话壳最小闭环。
//! 规划顺序：SQLite 会话持久化 → 主题自适应/交互打磨（P0）→ RAG（P1）→ A2A（P2）。
//!
//! 运行凭据不进代码库：正式代码读 `config.toml`（`Config::load`），测试读 `config.test.toml`，
//! 模板见 `config.example.toml`。

pub mod ai;
