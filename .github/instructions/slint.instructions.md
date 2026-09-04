---
description: "Use when: 编写/修改 Slint UI（.slint、src/ui.rs）或涉及 Slint 组件属性、布局尺寸、滚动交互。触发词：Slint、app.slint、气泡、布局、绑定环、viewport、滚动、TextEdit。"
applyTo: "ui/**/*.slint,src/ui.rs"
---

# Slint 开发规约（本仓库实测教训，禁止凭记忆写 API）

## API 查证先行
写任何不确定的属性/元素/回调/枚举前，二选一举证，不猜：
- 与 Cargo.lock 同版本的**本机源码**：`~/.cargo/registry/src/*/i-slint-compiler-<ver>/builtins.slint`（内置元素与属性）；std-widgets 属性查 docs.slint.dev 对应版本文档
- 查证结论若与既有代码注释冲突，以源码为准并更新本文件的陷阱清单

## 已实测的坑（勿再犯）
- 圆角 = `border-radius`；对齐 = `horizontal-alignment`；宽度测量 = `preferred-width`（**无** `implicit-width`）
- 组件根内不可用 `parent`（宽/位由使用方绑定）；同组件内兄弟 id 直接写 `id.prop`（`root.<id>` 会被当根属性查找报错）；组件定义内 `root` = 当前组件根
- 非父布局内的 layout 默认 height = parent.height → 用 layout 撑 Rectangle 高度必成绑定环；改用定宽 Text 的 `height` 反测
- `viewport-y` 负值系：0=顶部，`height - viewport-height`=底部；粘底状态机必须用 `scrolled()`（仅用户滚动触发），勿用 `changed viewport-y`
- 嵌套 ScrollView 的滚轮被外层截获：面板级滚动自绘 `TouchArea.scroll-event`（字段 `delta-x`/`delta-y`，处理器 `accept` 消费事件）
- TextEdit 的 `viewport-height` 与其 `height` 内置互绑，外部读取成环 → 内容自适应高度用同宽同字号的隐藏镜像 `Text` 测量
- 无 `%` 运算符（`Math.mod`）；无虚线描边（半透明实线近似）；组件/struct 用新语法声明（`:=` 已废弃，build.rs 视为错误）

## 交付纪律
- UI 交互与视觉 agent 无法自测：每次交互改动交付必附「用户待目测清单」，禁止自报验收
- 行为语义不确定（滚动方向、粘底、焦点）时，先做最小单点验证再组合进主界面
