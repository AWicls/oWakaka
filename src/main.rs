//! 可执行入口：纯转发，不含任何逻辑。
//! UI 专属接口是库目标的 `o_wakaka::ui::run`（Slint 壳），
//! 通信与配置逻辑在 `o_wakaka::ai`；分层与线程模型见各模块文档。

fn main() -> Result<(), slint::PlatformError> {
    o_wakaka::ui::run()
}
