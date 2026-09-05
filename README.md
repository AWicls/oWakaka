# oWakaka

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Rust Edition](https://img.shields.io/badge/edition-2024-orange)

一个基于 Rust 的 Windows 本地 AI 对话客户端，同时也是一个进行中的学习项目：**探索如何通过合理控制 AI agent 的成本与节奏，把软件架构从一个想法增量地做到可用**。

技术栈贯穿 Rust 原生：[Slint](https://slint.dev) 声明式 UI + tokio/reqwest 异步通信 + SQLite（rusqlite bundled）持久化。

## 当前能力（v0.3.1）

- **多提供商档案**：设置页「提供商」列表 + 详情卡管理多家网关；OpenAI 兼容 **chat / responses 双接口族** × 流式/非流式四路径；定制厂商（小米 MiMo：`api-key` 头鉴权、不支持字段剔除、内置端点、接口族锁定）；「测试连通」用表单当前值即时验证；密钥按提供商存 `config.toml` 的 `[keys]`，**DB 零密钥**；旧式单配置首次启动自动迁移
- **模型管理**：每提供商登记模型清单（自定义添加 / 异步拉取远端 `/models` 一键收录），别名 + 五能力位（思考/图片/语音/视频/工具）；对话页模型下拉跟随当前提供商；**思考能力消费**——未勾「思考」的模型置灰 Thinking 按钮且发送强制关（`reasoning.effort=none`）
- **多助手（persona）**：一个默认助手 + 任意自定义助手，各配**头像**（色板圆底首字 / 本地 png/jpg 图片）、名称、系统提示词、温度、**开场白**、**默认模型**；两级删除（回收站语义，默认助手不可删）；**会话创建即绑定助手**，发送走绑定助手的提示词/温度（悬空回落默认），聊天卡头栏显示当前会话助手，开场白持久化为第一条气泡，默认模型自动切下拉；用户人设全局注入 system 段
- **会话持久化**：SQLite 存储会话/消息（含思考全文与折叠展示态），重启原样恢复；会话两级删除——侧栏逻辑删进**回收站**（恢复 / 彻底删除连带消息物理清除）；多会话并行生成互不串扰（事件按会话+代次路由）
- **对话体验**：SSE 流式逐字回显（非流式兜底），推理过程独立面板三态（收起/部分/全开）自动跟随 + 自由回看；微信式左右气泡、多行输入（Ctrl+Enter 发送）、消息自动粘底、回答一键复制、生成中可停止
- **界面**：无边框窗口 + 自绘标题栏（原生拖移、Win11 贴边 snap、双击最大化、边缘拖缩放）；VS Code 现代深浅双主题 + 跟随系统（持久化重启保留）；三框布局可拖拽调宽/调高
- **健壮性**：错误分层（传输失败 / 非 2xx / 响应形状异常）原始信息不丢；密钥全程脱敏（Debug 输出恒 `[REDACTED]`）；DB/配置读写失败降级不阻断对话

## 快速开始

### 环境

- Windows（当前不考虑跨平台）
- Rust stable（edition 2024）

### 配置

提供商 / 模型 / 助手等设置全部在应用内「设置」页完成（落 SQLite `data/owakaka.db`）；`config.toml` **只存 API 密钥**（永不进 DB）：

```bash
cp config.example.toml config.toml
```

```toml
[keys]
p1 = "sk-..."   # 提供商 id → API 密钥（条目在设置页管理）
```

> 也兼容旧式全量 `config.toml`（`base_url`/`api_key`/`model`…）：首次启动自动一次性迁移——设置进库、密钥留文件。
> `config.toml`（正式）与 `config.test.toml`（测试凭据，全量格式、不碰 DB）均已在 `.gitignore` 中，密钥不进仓库。

### 运行

```bash
cargo run
```

### 测试

```bash
cargo test              # 离线：单元测试 + 文档测试（全部可离线运行）
cargo test -- --ignored # 联网：真实网关往返测试（需 config.test.toml）
```

## 项目结构

```text
src/
├── lib.rs            # 库目标（业务逻辑，doctest 依托）
├── main.rs           # 可执行入口：纯转发 ui::run()
├── db.rs             # SQLite 句柄与 schema（混合式表、开发期版本策略）
│   └── db/           # kv 配置 ｜ persona 多助手 ｜ session 会话消息与两级删除
├── ui.rs             # Slint 壳装配：Ctx、发送/停止、回流 Timer 路由
│   └── ui/           # host 会话簿记 ｜ bubbles ｜ models 下拉 ｜ prov 提供商页 ｜ settings 设置页 ｜ frame 窗控
└── ai/
    ├── client.rs     # 统一对话入口（chat/responses × 流式/非流式分发）
    │   └── client/   # chat、responses 端点族实现与 turn 请求组装（人设/温度/思考注入）
    ├── config.rs     # 运行时配置视图 + TOML 密钥读写（load_toml/store_toml）
    ├── provider.rs   # 提供商枚举（定制厂商接口族锁定/内置端点）
    ├── providers.rs  # 多提供商档案（kv JSON 整包 + 模型/别名/能力位）
    │   └── provider/ # 厂商定制实现（xiaomi_mimo）
    └── dto/          # 线格式 DTO（openai_chat ｜ openai_response）
ui/                   # app/bubble/input/session/settings/splitter/theme 声明式 UI（.slint）
data/owakaka.db       # 运行库（gitignore）
```

设计约定：DTO 只描述数据形状，客户端只管传输与错误映射；自定义字段经 `#[serde(flatten)]` 平铺透传，兼容各家网关扩展；DB 走混合式表——要查询的字段成列，其余进 `payload` JSON 叶子。

## 路线图

| 阶段 | 内容 | 状态 |
|---|---|---|
| **P0** 干净对话客户端 | OpenAI 兼容通信（chat/responses、流式）→ Slint UI 壳 → SQLite 会话持久化 → 多提供商档案与模型管理 → 多助手（persona） | ✅ 完成（v0.3.0） |
| P0 扩展 | Anthropic API、更多定制厂商、交互打磨 | 🔜 |
| P1 外挂知识库 RAG | embedding / ranker / 文档解析全部走外部 API | ⬜ |
| **P2** 生态接入 | **A2A**：与其他 agent 工具通话并协作完成任务；**MCP**：以客户端/宿主为主，让对话接入外部工具服务器（文件、搜索等），对外暴露 MCP 服务端为后置扩展 | ⬜ |

Anthropic 及定制化厂家 API 按 P0 约定的顺序接入：一家实现不预建 trait，第二家接入时再提炼抽象。

## 开发方式

本项目以「小步、准、可验证」的增量方式推进：每个改动先对齐*目标 / 最小验收标准 / 明确不做的事*，一次交付一个可运行的垂直切片；改动一个 commit（Conventional Commits + 中文描述），版本遵循 SemVer，详见 [CHANGELOG](CHANGELOG.md) 与 [.github/instructions](.github/instructions/)。

## License

[MIT](LICENSE)
