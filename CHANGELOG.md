# 更新日志

本项目所有重要变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Fixed
- 全站「✕」豆腐块修复（`✕` 字形在默认字体缺字，渲染成「口」）：侧栏行悬停删除钮改 **Path 图元自绘叉号**（与标题栏按钮同法）；回收站空态提示文案去掉 ✕ 改说「行右侧的叉号」；Thinking 置灰标记改 `Thinking ×`（U+00D7 字体普遍自带）

### Changed
- 设置界面修复与自适应打磨（用户清单）：
  - **返回钮移入左侧导航栏顶部**（顶栏取消，"设置"标题随迁）；导航栏/提供商列表/自定义输入宽全部改 `Math.max/min + root.width 比例` 随窗口收放；远端弹层与回收站浮层改居中自适应；模型框从固定 250px 改 stretch 吃剩余高度
  - 模型管理区重排：框在下、按钮行贴框下——左「测试连通」，右「自定义」「拉取远端」；**测试结果文案显示在按钮正下方**；「自定义」改为开合式输入行（请求名+别名+添加），拉取远端＋收录与自定义新建等价（同一条普通模型行）
  - 模型行两级化：默认单行「别名（或请求 ID）+ 能力摘要 + 编辑 + −」；点「编辑」展开别名输入与能力勾选（完成收起，改动即时落盘）；**删除钮 ✕ 统一改减号 −**；切换提供商自动收起编辑/自定义态
  - 单行输入文本上下居中全面修复：FieldRow/模型别名/自定义输入统一 `height 占满 + vertical-alignment: center`（原 y 算术致 placeholder 居中而文本偏上）
  - 模型行二次优化：能力胶囊**外置为常驻第二行**（不再藏进编辑态，直接勾选）；「编辑」按钮改名「别名」（仅切换名称行输入态）；名称**优先显示别名**，请求 ID 作灰色后缀跟名；能力摘要文本移除（胶囊已常显）
  - 模型行三次微调：编辑态**请求 ID 不再隐藏**（别名/ID 双文本常显，有别名才换主次样式）；「别名」钮**原地**切「完成」文案与配色（单钮）；「自定义」输入行补边框底色样式（原裸 TextInput 不可见）
  - 详情卡按钮行重排：删除类（删除/彻底删除）恒左，「完成」恒最右，「使用此提供商/恢复」居中右；提供商页内容补 `horizontal-stretch` 修不随窗口撑满
  - 返回钮缩小（26px 高、58px 宽）并与「设置」标题**并排**（返回在左，收进导航卡首行）
  - 别名编辑三修：**逐字符只静默落库、不再重建模型列表**（原重建销毁 TextInput 致每敲一字失焦）——「完成」收起时经新 `prov-model-commit` 回调统一回刷标签与下拉；编辑态不再隐藏显示名（别名/ID 双文本常显）；编辑态撤掉尾部 spacer（与输入框抢 stretch + TextInput 隐式宽随字数变 = 输入框前后抖动）
  - 「大下巴」根因：详情卡内模型区与尾部 filler **两个 `vertical-stretch:1` 平分剩余高度**，空隙卡在执行按钮上方——删掉尾部 filler，模型列表独吞剩余空间、按钮贴回内容
  - 「大下巴」真根因（补修）：Window 根变多子元素后 `if` 块（主三框 / SettingsPage）**不再自动撑满窗口**，只有 implicit 高——两处显式 `width/height: root.*`，整页贴窗、随窗口 100% 自适应

### Added
- 无边框窗口 + 自绘标题栏：去掉 Windows 原生外框（Slint `no-frame`），左上角三键（关闭贴最左角，其后最大化·还原/最小化；图标为 Path 描边自绘，不受字体缺字影响；关闭悬停红底白叉）；拖拽区按住即原生移窗（winit `drag_window`，保留 Win11 贴边 snap），双击切最大化，窗口外圈 6px 可拖拽缩放；回收站遮罩只盖内容区，标题栏恒在最上层可窗控
- 思考能力消费（Step C，能力位首个消费点）：当前模型未勾选「思考」→ Thinking 按钮置灰显「Thinking ✕」，**发送时即使开关开着也强制关**（请求带 `reasoning.effort=none`）；未在提供商登记的模型视为支持，绝不误关
  - 从设置页返回对话时自动重拉模型别名/能力表（页内改过即刻反映到下拉与开关）
  - 模型下拉数据源自此完全跟随当前提供商的模型列表（Step A 视图化后自然成立）
  - 其余能力位（图片/语音/视频/工具）仍只存只显——尚无消费场景（多模态/工具调用在未到位的功能上）
- 会话删除 UI（两级删除落地）：侧栏行悬停现 ✕ → **逻辑删除进回收站**；侧栏底部新增「回收站」按钮（有存货时高亮计数）
  - 回收站浮层：每条「恢复」（放回侧栏末尾，消息完整）/「彻底删除」（连带物理删全部消息，不可找回）
  - 删除生成中的会话：先取消在途请求，并整体作废删除点之后的轮次代次（防索引前移后流式事件串写错会话）；删掉最后一个会话自动补「新对话」，侧栏恒 ≥1
  - db 新增 `list_deleted` / `load_session`（回收站数据源）；行 ✕ 用 TouchArea 坐标分区实现（嵌套事件归属不可靠）
  - 测试：回收站列表/单会话装载/彻底删后清空 单测
- 主题持久化：主题选择存 `kv("ui")`（`{"theme":0|1|2}`），重启保留不再回退跟随系统
  - 实现为「双向链桥」：AppWindow `ui-theme <=> Theme.mode` + changed 触发回调落库——侧栏菜单/设置页写 Theme.mode 的旧路径零改动
- 模型管理（Step B）：提供商详情卡内「可用模型」区替换 Step A 的别名文本简化位
  - 数据结构升级：`models` 从 `id→别名` map 变为对象列表（请求名/别名/五能力位：思考·图片·语音·视频·工具）；旧 map JSON 读出即自动升级（`de_models` 兼容，无清库）
  - 行级即时落盘（不占全局「完成」）：`自定义+`（手填请求名+别名）；「拉取远端」异步 `GET /models` 弹层清单——未收录排前，**＋号**一键收录（✓=已在列，可连加）；行内改别名、点能力胶囊勾选、✕ 移除（若移除的是当前模型则指针清空）
  - 无别名模型也进模型下拉（别名空 = 显示请求名）；切换提供商清空远端缓存清单
  - **能力位本期只存只显**，消费（按能力裁剪请求/禁用不支持的思考开关）在 Step C
  - 测试：＋添加/覆写/排序/旧 map 兼容 doctest、upsert+remove 回环、active_model 清空单测
- 多提供商档案（Step A）：设置「提供商」页 = 列表竖栏 + 详情卡
  - 数据：`kv("providers")` JSON 整包（条目列表 + 使用中指针 + 全局当前模型），密钥按提供商存 `config.toml` 的 `[keys]` 表（DB 依旧零密钥）；**旧单配置启动时一次性全量迁移**（旧 kv 行删除、toml 重写为纯 `[keys]`，幂等；`config.test.toml` 测试通道不变）
  - UI：左列表「＋ 新增」→ 弹层选 自定义/Xiaomi MiMo（Provider 枚举驱动）；详情卡字段 = 显示名/基础地址/**路径后缀**（生效地址=base+后缀，斜杠规范化）/密钥（留空=不改，不回显）/接入方案（定制厂商锁定）/模型别名（每行 `id=显示名`）
  - 操作即时落盘：「测试连通」用表单当前值异步 `GET /models`（回显模型数/耗时/错误）；「完成」保存条目；「使用此提供商」切 active（下一次发送生效）；「删除」→ 灰显条目 →「恢复」/「彻底删除」（连带清 `[keys]` 密钥）；删除使用中条目自动回落到下一个可用
  - `Config` 退化为**运行时视图**：load = 使用中条目摊平（拼接完成、密钥装配、锁定族覆盖），`Client`/模型下拉/`message.model` 全链路零感知；模型切换回写 `providers.active_model`
  - 全局「保存」按钮自此只服务 AI 助手/用户设定两页；提供商编辑不走它
  - 删除宪法入库：一切删除 = 逻辑删（可恢复）→ 彻底删（物理清）两级；session 表补 `deleted_at` 列（`SCHEMA_VERSION` 4，v2/v3 库补列/补表保数据，其余版本差仍重建），`soft_delete_session`/`restore_session`/`purge_session` API 与 doctest 就位——**侧栏会话删除 UI 留下一小步**
  - 测试：join 拼接/别名往返/视图装配/迁移「旧的不留+幂等」doctest 4、persona 式 CRUD+active 回落+密钥连带清除单测、v2/v3 补结构单测
- 设置界面：**全屏设置页**（点侧栏齿轮整页切换，左上「返回」回对话，不再是弹窗）
  - 样式与整体美学统一：窗底 + 双圆角卡片（左导航 148px / 右内容卡），顶栏返回钮用新增左箭头 Icon（which=6）
  - 左导航四分区：**界面**（主题三选：浅色/深色/跟随系统，与侧栏菜单同源 `Theme.mode`，即时生效暂不持久）｜**提供商**（原表单）｜**AI 助手**（system prompt + 温度）｜**用户设定**（人设）
  - 编辑值存页面级属性，切分区不丢未保存内容；保存一次性提交全部字段后**留在页面回显状态**（旧弹窗"保存即关"改为手动返回）；返回 = 放弃编辑（页面重建即重置）；Rust 侧校验落盘逻辑零改动
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
