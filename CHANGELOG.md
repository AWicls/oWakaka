# 更新日志

本项目所有重要变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Added
- 角色设定入库并注入请求（DB-3，persona）：
  - 新增 `persona` 表（一 kind 一活跃行：`assistant` 存系统提示词+温度，`user` 存人设描述；温度 NULL = 不发字段）；`SCHEMA_VERSION` 2→3 为**纯补表例外**（v2 库只补建表不清数据，其余版本差不匹配仍整库重建）
  - 发送链路：每次发送现读 DB 活跃行组装 `TurnOptions.persona`——助手提示词作首条 system 消息、用户人设并入【用户人设】附段、温度并入请求体 `extra`（与 `reasoning` 键共存互不顶掉）；未设定角色时请求与旧行为逐位一致
  - 设置弹窗新增三字段：助手设定（多行 TextEdit）、用户人设（多行）、温度（单行，0–2 校验、留空=不发）；保存与设置一并落盘，重启回显
  - 测试：persona 存取自成一体/温度可清空 doctest、v2→v3 补表不丢数据单测、system 前置+温度注入/reasoning 合并不被顶掉/无人设零变化单测 3
- 配置存储入 DB（DB-2，`config.toml` 瘦身为仅密钥）：
  - 新增 `kv` 表（JSON 值 CHECK 把关）；运行配置（provider/base_url/model/api/stream/[models] 别名）整体作设置 JSON 存 `kv("config")`，**密钥序列化即丢、永不进 DB**（`Config.api_key` skip_serializing + `Secrets` 独立结构，均 Debug 恒脱敏）
  - 读取两路组装（`Config::load(db)`）：设置取 DB、密钥取 `config.toml`；DB 无配置时回落老式全量 toml——**首次保存设置/切换模型自动完成拆分迁移**，老配置零改动照常可用；此后 `config.toml` 只剩 `api_key` 一行
  - 影响路径同步改造：`Client::load(db)`、设置弹窗保存、模型切换回写、下拉初始值；模板 `config.example.toml` 重写（test.toml 保持全量格式，`#[ignore]` 真实测试不碰 DB）
  - `SCHEMA_VERSION` 1→2：按开发期重建策略，**既有测试会话数据清库一次**
  - 测试：kv 存取自成一体/非法 JSON 拒绝、两路组装与拆分迁移往返、`Secrets` 容缺省/脱敏 doctest 3 + store_toml 密钥不落盘断言收紧
- SQLite 会话持久化（DB-1，`data/owakaka.db`，rusqlite bundled）：
  - 新模块 `db`：混合式表（查询字段成列 + `payload` JSON 叶子），session/message 两表；`persona_id`、`origin` 预埋列；时间戳 SQLite 生成 RFC3339 UTC；`user_version` 不匹配即整库重建（开发期策略，无 backfill）
  - 落库时机：发送即写 user 消息、轮次收尾（Done/Error/停止）写 assistant 终稿一条（流式增量不落库）；会话标题首答后同步；重启后侧栏与气泡（含思考折叠态）按原样恢复，默认可见 = 最近活动会话
  - 降级路径：库打开失败回落内存库、单条读写失败仅记日志，均不阻断对话；`data/` 进 .gitignore
  - 测试：文件库"重启往返"/payload 缺省容错 doctest 2 + 旧版本重建/会话内序号 单测 2
- UI 模型切换与程序内设置弹窗：
  - 输入区左下角新增模型按钮（Thinking 左侧）：点击向上弹出清单 = config `[models]` 别名 ∪ 远端 `GET /models`（首次打开异步拉取，失败下回重试），选中即切换激活模型（后续发送携带）并回写 `config.toml model` 字段
  - 会话栏左下角新增设置按钮（主题按钮旁，滑杆图标）：打开程序内居中弹窗（遮罩点击关闭），可编辑并落盘 提供商（通用兼容/小米 MiMo 二选一）、API 根地址、API 密钥（留空=不改）、激活模型；校验不过不落盘并在弹窗内回显错误，保存成功后客户端缓存失效、下一次发送即用新配置
  - 别名表编辑暂不支持（`[models]` 仍可手工改文件，下轮并入弹窗）
- 提供商定制接口落地（`ai::provider::Customization`，一家一个子模块实现）：
  - 小米 MiMo 真实定制：鉴权改厂商首选 `api-key` 头（替代通用 Bearer）；请求体发送前剔除厂商不支持字段（`background`/`previous_response_id`/`context_management`）；思考开启时显式注入 `reasoning.effort="low"`（MiMo 各档等效），关闭沿用 `effort=none`
  - 定制注入点覆盖 chat/responses 两端点族 × 流式/非流式全路径与 `GET /models`；通用（custom）路径零行为变化
  - 测试：鉴权头/字段注入/不支持字段剔除 3 个离线单测
- 定制提供商与配置规范化（顶级 `provider` 字段，缺省 `"custom"`）：
  - `provider = "xiaomi_mimo"`（小米 MiMo）：内置官方端点 `https://api.xiaomimimo.com/v1`（显式 `base_url` 可覆盖为专属网关），接口族锁死 Responses（显式配 chat 校验报错）；线格式复用通用 responses 实现，不另建客户端
  - `model` 字段语义升级为"当前激活模型"；新增 `[models]` 别名表（模型 ID → UI 显示名）可读写回存；老配置零改动照常可用
  - `Client::list_models()`：`GET /models`（OpenAI 兼容格式）拉取远端可用模型清单，UI 模型切换（下一步）数据源
  - `TurnOptions.model`：轮次请求级模型覆盖（None/空回落配置默认）；`Client::load()` 建客户端前先过 `Config::validate()` 完备性检查
  - 测试：配置解析/校验/别名读写 doctest、MiMo 端点解析与请求模型覆盖单测各 1；真实网关测试 `list_models_roundtrip`（`#[ignore]`）
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
