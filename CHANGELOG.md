# 更新日志

本项目所有重要变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Fixed
- 生成中滚到底部跳回顶部：`viewport-y` 为负值语义（0=顶部、`height-viewport-height`=底部），修正贴底公式符号
- 思考完成后部分态无法滚动：内嵌 ScrollView 滚轮被外层截获，改为自绘滚动视图（TouchArea `scroll-event` + `accept` 消费滚轮），并支持贴底哨兵（滚回底部自动恢复跟随新内容）
- "部分"态思考块从固定 3 行改为**最多 5 行、随内容自适应高度**
- 思考过程收起态内容外泄背景：思考面板改用 ScrollView（原生裁剪）

### Added
- 思考过程展示（推理模型 `reasoning_content`/`reasoning` 增量）：client 层 `StreamEvent::{Reasoning, Content}` 事件分流；三态（收起/部分/全开，默认部分=最新 3 行），思考完成自动收起、手动切换后固定状态
- 回答气泡下方最右侧"复制"按钮，经 arboard 写系统剪贴板（Slint 无剪贴板 API，新增依赖 arboard）
- 自动粘底滚动：新内容与流式增量始终保持可见

### Changed
- 滚动粘底改为"状态机"模式（消息列表与思考面板共用）：内容增长仅当处于底部时自动跟底；生成中可自由上滑查看历史，滑回底部自动恢复跟底；思考完成后部分态可正常滚动回看
- 思考入口改为回答气泡上方左侧的 `Thinking……` 小按钮（原整卡可点）；"部分"态思考面板 3 行高且可滚动回看
- 思考面板样式：透明灰底 + 半透明细灰边框 + 圆角（Slint 1.17 核心不支持虚线描边，以半透明实线近似"虚灰框"观感）
- 回答气泡宽度改用 `Text.preferred-width` 精确测量并对齐内容（上限 82%，靠向发言侧）；收窄内边距（28→24px）与最小宽（64→48px）；思考面板固定占宽 82% 不受回答限制

### Changed（历史轮次）

### Changed
- UI 逻辑从 `main.rs` 迁入库目标新模块 `src/ui.rs`：`ui::run()` 为 UI 专属接口，`main.rs` 瘦身为纯转发入口；内部拆出 `snapshot_history`/`ensure_client`/`spawn_chat`/`append_assistant` 具名私有函数并补全模块级/条目级文档注释（含线程模型图）

### Added
- P0 对话壳（Slint，`ui/app.slint` + `src/ui.rs`）：3:2 窗口、左右聊天气泡（用户蓝/助手灰）、底部输入框+发送按钮；流式增量逐字入气泡（默认路径），网络错误与配置缺失以内联气泡反馈且不阻塞 UI；tokio 后台任务 + mpsc + Slint Timer 30ms 轮询刷新
- 构建脚本 `build.rs` 与依赖 slint / slint-build
- 流式对话 `Client::chat_stream`（SSE，**默认推荐入口**）：请求体注入 `stream: true`，逐段回调文本增量，`data: [DONE]` 或 EOF 结束；坏 `data:` 行报 `ChatError::Decode` 立即中止；零新增依赖（复用 reqwest 核心 chunk API）
- 流式分块 DTO（`dto/openai_chat/chunk.rs`）：`Chunk`/`ChunkChoice`/`Delta`，容忍 delta 缺省 role/content、末块仅 finish_reason，附解析 doctest
- 离线单测 `sse_line_dispatch`：验证注释行/心跳/非 data 字段/增量/哨兵/坏 JSON 六类行分发规则
- ai 模块链完整文档注释（`//!` 模块级 + `///` 条目/字段级），含 4 个可离线运行的文档测试：配置加载、请求 extra 平铺序列化、响应未知字段收集、客户端构造
- OpenAI chat 兼容非流式客户端（`src/ai/client.rs`）：通用 `Client::from_config` + `chat()`，任意提供 `/chat/completions` 的网关可用；非 2xx 返回带状态码与原始响应体的 `ChatError`
- TOML 配置加载（`src/ai/config.rs`）：`base_url`/`api_key`/`model` 三要素，模板 `config.example.toml` 入库；`Config::load()` 供正式代码读取 `config.toml`
- chat 响应 DTO 补全 `Choice`；接线 ai 模块链进入编译；真实请求测试 `#[ignore]`，用 `cargo test -- --ignored` 验证
- 依赖：reqwest、tokio、toml

### Fixed（五轴质量评审修复轮）
- `Config` 改为手动 `Debug` impl 恒脱敏 `api_key`（防 `{:?}`/日志泄漏），并新增 doctest 断言锁定该行为
- chat DTO 健壮性：`Response.choices` 加 `#[serde(default)]` 容忍 content_filter 场景空数组；`Message.content` 改为 `Option<String>` 以容纳 tool_calls/纯推理响应的 `content: null`（否则整个响应解析失败）
- 新增 `ChatError::Decode`：2xx 但响应体非合法 JSON 时携带解析错误与原始响应体，不再退化为难排查的传输错误
- lib 目标更名 `o_wakaka`（crate snake_case 规范）、`openai_response` 占位 `input`→`Input`；`cargo fmt`/`cargo clippy --all-targets` 达成零警告

### Changed
- 通用 TOML 配置读写工具（`load_toml`/`store_toml`，任意 serde 类型可用）取代 `Config::from_file`；文档测试改为在系统临时目录自造配置做读写往返，不再依赖本地 `config.test.toml`（该文件现仅供 `#[ignore]` 真实凭据测试）

## [0.1.0] - 2026-09-04

### Added
- 项目基线：Rust 二进制骨架（edition 2024）、OpenAI chat/response DTO 模块骨架
- Git 工作流规范（.github/instructions/git-workflow.instructions.md）：干净工作区门禁、细粒度提交、Conventional Commits + 中文、Keep a Changelog、SemVer tag
- 实施教练 agent（.github/agents/owakaka-mentor.agent.md）：阶段门禁 + 计划门禁 + 成本控制
- CHANGELOG.md 与 .gitignore（含密钥、数据库文件忽略）
