# 更新日志

本项目所有重要变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.2.0] - 2026-09-05

### Added
- OpenAI chat 兼容通信层（`src/ai/`）：
  - 非流式 `Client::chat()`：任意提供 `/chat/completions` 的网关可用；错误分层 `ChatError::{Http, Api, Decode}`，服务端错误与原始响应体不丢
  - 流式 `Client::chat_stream()`（默认推荐入口）：零新增依赖手写 SSE 行缓冲，`[DONE]`/EOF 双终止，坏行立即 `Decode` 中止；`StreamEvent::{Reasoning, Content}` 分离推理模型思考增量（`reasoning_content`/`reasoning`）与回答增量
  - 公共兼容 DTO（`dto/openai_chat/`）：请求/响应/SSE 分块（`Chunk`/`ChunkChoice`/`Delta`），`#[serde(flatten)]` 自定义字段平铺透传；`Message.content` 为 `Option<String>` 容纳 tool_calls/纯推理响应
- 配置体系：`Config`（base_url/api_key/model 三要素，`Debug` 输出对密钥恒脱敏并有 doctest 锁定）+ 通用 TOML 读写工具 `load_toml`/`store_toml`；凭据文件 `config.toml`（正式）与 `config.test.toml`（测试）均不入库，模板 `config.example.toml`
- Slint 对话壳（`ui/app.slint` + `src/ui.rs`，唯一接口 `ui::run()`）：
  - 3:2 窗口；微信式左右气泡，回答气泡宽度随内容自适应（上限 82%，靠向发言侧）
  - 思考过程面板：`Thinking……` 按钮三态（收起 / 部分 ≤5 行随内容自适应 / 全开），流式贴底自动跟随，思考完成自动收起，手动切换后状态固定
  - 消息列表自动粘底：流式增量保持可见，滚离底部暂停跟随、回到底部恢复
  - 独立多行输入区：高度随内容自适应（上限 8 行后内部滚动），Ctrl+Enter 发送、Enter 换行，发送按钮位于输入区右下
  - 回答一键复制（arboard 系统剪贴板）；网络错误与配置缺失以内联气泡反馈且不阻塞 UI
  - 线程模型：tokio 后台网络任务 + mpsc 回传 + Slint Timer 30ms 主线程刷新
- 测试与文档：ai/ui 模块链完整 `//!`/`///` 文档注释（含线程模型图）；离线单测 `sse_line_dispatch`（SSE 六类行分发）+ 5 个可离线运行 doctest；真实网关测试标 `#[ignore]`，`cargo test -- --ignored` 手动验证
- 构建与依赖：`build.rs`（slint-build）；reqwest、tokio、toml、slint、arboard

### Changed
- crate 拆分为 lib（`src/lib.rs`，目标名 `o_wakaka` 符合 snake_case）+ 薄 bin，使 `///` 文档测试可被 `cargo test` 执行
- UI 逻辑从 `main.rs` 收口至库模块 `src/ui.rs`，`main.rs` 瘦化为纯转发入口
- 流式分块 DTO 独立建模（`Delta` 与请求侧 `Message` 不同构），容忍厂家增量差异

## [0.1.0] - 2026-09-04

### Added
- 项目基线：Rust 二进制骨架（edition 2024）、OpenAI chat/response DTO 模块骨架
- Git 工作流规范（.github/instructions/git-workflow.instructions.md）：干净工作区门禁、细粒度提交、Conventional Commits + 中文、Keep a Changelog、SemVer tag
- 实施教练 agent（.github/agents/owakaka-mentor.agent.md）：阶段门禁 + 计划门禁 + 成本控制
- CHANGELOG.md 与 .gitignore（含密钥、数据库文件忽略）
