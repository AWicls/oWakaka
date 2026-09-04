# 更新日志

本项目所有重要变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Added
- ai 模块链完整文档注释（`//!` 模块级 + `///` 条目/字段级），含 4 个可离线运行的文档测试：配置加载、请求 extra 平铺序列化、响应未知字段收集、客户端构造
- OpenAI chat 兼容非流式客户端（`src/ai/client.rs`）：通用 `Client::from_config` + `chat()`，任意提供 `/chat/completions` 的网关可用；非 2xx 返回带状态码与原始响应体的 `ChatError`
- TOML 配置加载（`src/ai/config.rs`）：`base_url`/`api_key`/`model` 从 `config.local.toml` 读取（不入库），提供 `config.example.toml` 模板
- chat 响应 DTO 补全 `Choice`；接线 ai 模块链进入编译；真实请求测试 `#[ignore]`，用 `cargo test -- --ignored` 验证
- 依赖：reqwest、tokio、toml

## [0.1.0] - 2026-09-04

### Added
- 项目基线：Rust 二进制骨架（edition 2024）、OpenAI chat/response DTO 模块骨架
- Git 工作流规范（.github/instructions/git-workflow.instructions.md）：干净工作区门禁、细粒度提交、Conventional Commits + 中文、Keep a Changelog、SemVer tag
- 实施教练 agent（.github/agents/owakaka-mentor.agent.md）：阶段门禁 + 计划门禁 + 成本控制
- CHANGELOG.md 与 .gitignore（含密钥、数据库文件忽略）
