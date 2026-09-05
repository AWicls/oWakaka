# 更新日志

本项目所有重要变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Added
- OpenAI Responses API（`/responses`）接入：
  - 线格式 DTO（`dto/openai_response/`）：请求 `input` 消息项、非流式 `output` 条目（`Response::assistant_text()`/`reasoning_text()` 提取正文与思考摘要）、流式语义事件（`Event`），`extra` 平铺透传扩展字段
  - `Client::respond()`（非流式）与 `Client::respond_stream()`（流式）：语义事件按 `type` 分发，回答 `response.output_text.delta`、思考 `response.reasoning_summary_text.delta`（兼容 `reasoning_text.delta`）；`response.completed`/`[DONE]`/EOF 终止，`error`/`response.failed` 报新错误变体 `ChatError::Stream`
  - 统一对话入口 `Client::generate()`：按配置在两端点族 × 流式/非流式四路径分发，非流式把整段回复折算成事件现场回调（先思考后正文），UI 侧唯一调用点
- 配置项（老配置文件缺字段照常加载）：
  - `api`：缺省 `"chat"` 兼容接口保底，手动设 `"responses"` 启用新接口
  - `stream`：缺省 `true` 流式，手动设 `false` 整段一次性返回
- 测试：Responses SSE 事件分发 / 请求转换 / 非流式折算 3 个离线单测 + 3 个 DTO doctest；真实网关测试 `respond_roundtrip`/`respond_stream_roundtrip`（`#[ignore]`）
- 三框对话布局与会话侧栏（UI 结构打磨第一步）：
  - 左侧历史会话栏（圆角卡片，初始宽 190px）+ 右上对话区 + 右下输入区，卡片间留白，默认窗口 1120×720
  - 侧栏顶部「+ 开始新对话」；内存多会话：可新建/点击切换，标题取首条输入摘要（24 字截断）；重启不保留（SQLite 持久化待后续）
  - 可拖拽分隔条：悬停/拖动显现小白条，纵向调侧栏宽（150–420px），横向调输入区高（64–260px），光标显示 col/row-resize
- 会话隔离并行生成与停止：
  - 流式事件携带 `(会话号 sid, 代次号 gen_id)` 路由写回发起会话；切换只改可见模型，多会话 tokio 后台任务并行、互不串扰
  - 生成中不再锁定输入框；当前会话生成中发送按钮变为「■ 停止」，点击经 oneshot 取消令牌丢弃请求 future、连接即断；停止后重发按代次号丢弃旧轮迟到事件
  - 「●/○ Thinking」切换钮（输入区左下，默认开）：关闭时请求平铺透传 `reasoning.effort=none`，开启不发消息（跟随网关默认）；进行中的生成不受开关切换影响
  - 输入编辑器改为自绘无框区（原生 `TextInput` + ScrollView 光标跟随滚动）：与卡片同色、无边框线，编辑区与按钮行以透明间距分隔
  - 对话区滚动条常显（fluent 默认悬停才显形）；视口宽钉死可见宽，修复长消息致内容偏右、无左右边距
- VS Code 现代双主题配色系统（色值取自官方 dark_modern/light_modern 主题文件）：
  - 新增 `ui/theme.slint`：`Theme` 全局（浅色/深色/跟随系统三态，默认为跟随系统，读内置调色板的系统深浅色）+ 17 个语义色 token，全组件硬编码色清零
  - 会话栏左下角主题按钮（SVG Path 图标：太阳/月亮/显示器），点击弹出菜单选择，选中项带对勾，点空白关闭；图标组件 `Icon` 亦为 SVG path 实现
  - 发送/停止按钮改自绘胶囊（fluent Button 无法随应用主题变色），禁用/悬停态取 token；用户气泡文字改按强调色自适应（原固定白字在浅色助手气泡上不可读）
  - 已知边界：内置 ScrollView 滚动条颜色仍跟随系统调色板，与应用强制主题不一致时观感偏差，留待滚动区自绘专项

### Fixed
- 生成中新建/切换会话导致输出串入新会话：原实现把"增量写入目标"随切换改写，现按事件自带 sid 路由，切换不再影响写入

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
