//! 可执行入口：纯转发，不含任何逻辑。
//! UI 专属接口是库目标的 `o_wakaka::ui::run`（Slint 壳），
//! 通信与配置逻辑在 `o_wakaka::ai`；分层与线程模型见各模块文档。

// debug 保留控制台看启动诊断，release 切 GUI 子系统免弹 cmd 窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), slint::PlatformError> {
    o_wakaka::ui::run()
}
