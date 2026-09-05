//! oWakaka 库目标：Windows 本地 AI 对话客户端的全部逻辑。
//!
//! 当前进度（P0 · 第五步）：OpenAI chat/responses 双接口通信（默认流式 SSE）+ 三框对话 UI（侧栏多会话，SQLite 持久化重启不丢）。
//! 规划顺序：配置/角色设定入库（DB-2/3）→ 主题自适应/交互打磨（P0）→ RAG（P1）→ A2A + MCP 客户端（P2，MCP 服务端后置）。
//!
//! 模块结构：
//! - [`ai`]：LLM 集成层（配置加载、chat 客户端、线格式 DTO）
//! - [`db`]：SQLite 会话/消息持久化（主线程单连接同步读写）
//! - [`ui`]：Slint 对话壳，唯一接口 [`ui::run`]；`main.rs` 仅转发至此
//!
//! 运行凭据不进代码库：正式代码读 `config.toml`（`Config::load`），测试读 `config.test.toml`，
//! 模板见 `config.example.toml`。

pub mod ai;
pub mod db;
pub mod ui;
