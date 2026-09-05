---
name: git-workflow
description: "Use when: 即将做任何代码改动、提交代码、打 tag、发布版本、更新 CHANGELOG。触发词：commit、提交、tag、版本、发布、release、changelog、git、验收、改动前检查。"
---

# oWakaka Git 工作流规范

## 硬规则（每次改动必须遵守）

1. **改动前确认工作区干净**：开始任何改动前先 `git status`；有未提交内容时先提交或与用户确认，绝不在脏工作区上叠加新改动。
2. **每个改动一个 commit**：提交粒度要细，一个逻辑独立的改动 = 一个 commit；不混合多个改动，不留批量大提交。
3. **提交信息格式**：Conventional Commits + 中文描述，即 `type: 简述`，必要时附正文说明动机。
   - type：`feat` 新功能 / `fix` 修复 / `docs` 文档 / `refactor` 重构 / `test` 测试 / `perf` 性能 / `chore` 杂务
4. **Changelog 随改动记**：每个用户可感知的改动都追加到根目录 `CHANGELOG.md` 的 `[Unreleased]` 小节；打 tag 时把 Unreleased 归档为对应版本号并写日期。格式遵循 [Keep a Changelog 1.1.0 中文版](https://keepachangelog.com/zh-CN/1.1.0/)。
5. **SemVer tag**：tag 格式 `vX.Y.Z`。`feat` → minor，`fix`/补丁 → patch，破坏性变更 → major；0.x 阶段不承诺向后兼容，破坏性变更可放在 minor。只在**用户验收节点**打 tag，agent 只建议版本号，不擅自打。
   - **打 tag 前 checklist**（缺一项不得打）：① CHANGELOG `[Unreleased]` 归档为版本号+日期，一次整块替换 ② README 版本与功能描述同步 ③ `cargo fmt --check` + `cargo clippy --all-targets` 零警告 + `cargo test` 全绿。
6. **main 单分支**：直接在 main 提交；禁止 `--force`、`--no-verify` 和未经确认的 `reset --hard`。

## 改动开工顺序

`git status`（必须干净）→ 给出本步计划 → 用户确认 → 实施 → `cargo check` 验证 → commit（+ 更新 CHANGELOG）→ 汇报。
