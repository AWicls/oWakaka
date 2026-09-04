# oWakaka 项目总纲

流程细则见 [git-workflow 指令](instructions/git-workflow.instructions.md)；阶段路线图与节奏门禁见 [实施教练 agent](agents/owakaka-mentor.agent.md)；Slint 专项见 [slint 指令](instructions/slint.instructions.md)。以下仅列跨文件通用硬约定（均为用户反复纠正后沉淀，默认必须遵守）：

## 代码组织
- 模块风格：2018+ 的 `foo.rs` + 同名目录，**不用 `mod.rs`**
- crate 结构：业务逻辑在 lib 目标（`src/lib.rs`，doctest 依托）；`main.rs` 仅纯转发；UI 逻辑归 `src/ui.rs`（唯一接口 `ui::run()`），通信层归 `src/ai/`
- 抽象纪律：单家实现不预建 trait；能跑通的最小垂直切片优先

## 注释与测试
- 全模块链 `//!` 模块级 + `///` 条目/字段级文档注释；凡可离线验证的行为写成 doctest（临时资源用 `std::env::temp_dir`，禁依赖本地凭据文件）
- 凭据文件：正式 `config.toml`、测试 `config.test.toml`（均不入库，模板 `config.example.toml`）；读写一律经 `load_toml`/`store_toml`
- 含密钥结构体禁 derive Debug：手动 impl 恒脱敏，并用 doctest 断言锁行为

## 质量门禁
- 每步交付前：`cargo fmt --check` + `cargo clippy --all-targets` 零警告 + `cargo test` 全绿；真实网络测试标 `#[ignore]`
- UI 交互行为不得自报验收：交付附「待用户目测清单」
- 编辑 CHANGELOG 先读目标区块再改；发布归档一次整块替换，防旧串吞条目

## 工具约定
- 检索本地 src 代码优先用 codegraph MCP（`codegraph_explore`：一次返回源码+调用链+影响范围；详见 codegraph-usage skill）；MCP 不可用回退 CLI `codegraph explore`，再不行才 grep+read
- codegraph 只索引本仓库源码：第三方 API（Slint/reqwest 等）仍按 slint 指令走官方文档/本机源码查证
- 刚编辑过的文件可能未同步进索引（~2s debounce），拿不准时直接 Read 该文件
