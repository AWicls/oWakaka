//! 无边框窗框接线：标题栏三键与拖窗的原生动作（ui/app.slint CaptionBar 的 `win-*` 回调落点）。
//!
//! 拖窗经 winit `drag_window()`（`unstable-winit-030` feature），等效原生标题栏按住拖动，
//! 保留 Win11 贴边 snap。回调触发时窗口句柄可能尚未创建，按官方社区示例 `spawn_local`
//! 延迟到事件循环内取句柄。最大化状态由后端写回 `maximized` 属性，按钮图标随之切换。

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

use super::AppWindow;

/// 最大化/还原互换（标题栏按钮与拖拽区双击共用）。
pub fn toggle_maximize(window: &slint::Window) {
    window.set_maximized(!window.is_maximized());
}

/// 接 `win-minimize` / `win-maximize` / `win-drag` 三个窗口动作（关闭在 .slint 侧直接 `root.close()`）。
pub fn wire_frame(window: &AppWindow) {
    let weak = window.as_weak();
    window.on_win_minimize(move || {
        if let Some(app) = weak.upgrade() {
            app.window().set_minimized(true);
        }
    });

    let weak = window.as_weak();
    window.on_win_maximize(move || {
        if let Some(app) = weak.upgrade() {
            toggle_maximize(app.window());
        }
    });

    let weak = window.as_weak();
    window.on_win_drag(move || {
        let weak = weak.clone(); // 每次按下都交给新任务，闭包本身仍可复用
        let _ = slint::spawn_local(async move {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if let Ok(winit_win) = app.window().winit_window().await {
                let _ = winit_win.drag_window();
            }
        });
    });
}
