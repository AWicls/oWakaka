//! oWakaka 库目标：Windows 本地 AI 对话客户端的全部逻辑。
//!
//! 当前进度（P0 · 第五步）：OpenAI chat/responses 双接口通信（默认流式 SSE）+ 三框对话
//! UI（侧栏多会话，SQLite 持久化重启不丢）；配置、persona 角色设定均已入库（DB-1~3）。
//! 规划顺序：主题自适应/交互打磨（P0）→ RAG（P1）→ A2A + MCP 客户端（P2，MCP 服务端后置）。
//!
//! 模块结构：
//! - [`ai`]：LLM 集成层（配置加载、chat 客户端、线格式 DTO）
//! - [`db`]：SQLite 持久化（会话/消息 + 设置 kv + persona 角色设定，主线程单连接同步读写）
//! - [`ui`]：Slint 对话壳，唯一接口 [`ui::run`]；`main.rs` 仅转发至此
//!
//! 配置与凭据不进代码库：设置 JSON 存 `data/owakaka.db`（kv 表），`config.toml`
//! 只剩 `api_key`（库中放配置、凭据留文件）；测试读全量 `config.test.toml`，
//! 模板见 `config.example.toml`。

pub mod ai;
pub mod db;
pub mod ui;
