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

## 布局语义坑（「大下巴」多轮返工实证）
- `visible: false` 只免渲染不免布局占位：布局内条件子项必须用 `if cond :` 块，否则收起项仍吃高度+spacing
- 布局中裸 Rectangle/嵌套 Layout 默认 `stretch: 1`，会吃掉富余高把定高内容摊开下沉 → 逐层显式 `stretch: 0` + 末尾惰性 filler `Rectangle { vertical-stretch: 1 }`
- 非布局父级（Rectangle）里的定高 Layout 被求解器垂直居中（非 y=0）→ 显式 `x: 0; y: 0; width: parent.width` 钉住
- Rectangle 的 layout preferred 不从子级 layout 反推：`min-height` 框装 N 行定高内容恒按 min-height 计 → 多出行被 clip 且不可滚；终态 = 框 `min-height` + `stretch: 1` 吃富余高，内嵌 ScrollView 自滚（viewport-height 按行数绑定、viewport-width 钉死）
- 回调内对模型行赋值触发整行组件重建，编辑中 TextInput 的焦点/滚动状态丢失（每字符失焦、输入框位移）→ 编辑态用独立缓冲，提交时才写回模型

## 交付纪律
- UI 交互与视觉 agent 无法自测：每次交互改动交付必附「用户待目测清单」，禁止自报验收
- 行为语义不确定（滚动方向、粘底、焦点）时，先做最小单点验证再组合进主界面
- **熔断规则**：同一视觉/布局 bug 修 2 轮未愈，停止调参与凭记忆猜测，必须回退到本机源码/官方文档查证 + 最小复现，再改主界面
