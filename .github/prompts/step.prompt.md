---
description: "一句话目标 → 增量指令：计划门禁三件套，确认后动工并走完质量门禁与提交"
name: "增量指令"
argument-hint: "例如：实现 SQLite 会话持久化"
agent: "oWakaka 实施教练"
---

本增量目标：${input:goal}

严格按以下顺序执行，禁止跳步：

0. 工作区检查：`git status` 不干净则先停下报告，等待处理
1. 输出**计划门禁三件套**，在用户明确确认前不得改动任何代码：
   - 本步目标（一句话）
   - 最小验收标准（可执行的命令或可观察的行为）
   - 不做的事（明确排除项，防止范围蔓延）
2. 确认后动工：最小垂直切片，改动聚焦尽可能少的文件
3. 质量门禁：`cargo fmt --check` + `cargo clippy --all-targets` 零警告 + `cargo test` 全绿；UI 改动附「待用户目测清单」，不得自报验收
4. 细粒度 commit（Conventional Commits + 中文）+ 同步 CHANGELOG `[Unreleased]`
5. 汇报验证结果 + 下一步建议

细则以 [项目总纲](../copilot-instructions.md) 与 [git 工作流](../instructions/git-workflow.instructions.md) 为准，本文件不重复其内容。
