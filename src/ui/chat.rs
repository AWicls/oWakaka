//! 对话域入口：发送（贴气泡 → 快照历史 → 后台轮）、停止、思考开关，以及后台事件
//! 回流 Timer（按 (sid, gen) 路由写回发起会话）。[`super`] 的 `run()` 建好 [`Ctx`] 后
//! 调 [`wire_chat`] / [`wire_timer`] 完成接线；共享句柄束与回传消息类型（[`UiMsg`]、
//! [`ClientCache`]）定义在 `ui.rs`，本页只消费。
//!
//! 域内子模块：[`host`]（多会话簿记）、[`session`]（会话栏/回收站接线）、
//! [`bubbles`]（气泡模型操作）、[`models`]（模型下拉状态机）。

mod bubbles;
pub(super) mod host;
pub(super) mod models;
pub(super) mod session;

pub(super) use host::{Host, RunRef, StreamMsg};
pub(super) use models::ModelsState;

use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use slint::{ComponentHandle, SharedString, Timer, TimerMode};

use super::{AppWindow, ChatMessage, ClientCache, Ctx, UiMsg};
use crate::{
    ai::client::{Client, TurnEvent, TurnOptions, TurnPersona},
    db::Db,
    ui::settings::{on_fetch_result, on_test_result},
};

use self::bubbles::{STATE_PARTIAL, append_part, fold_tail, snapshot_history};

/// 取（必要时懒建）客户端；`Err` 携带可直接展示的文案。
/// 请求组装/取消编排已下沉至 [`Client::spawn_turn`](crate::ai::client::Client::spawn_turn)。
pub(super) fn ensure_client(cache: &ClientCache, db: &Db) -> Result<Arc<Client>, String> {
    let mut guard = cache.lock().unwrap();
    if guard.is_none() {
        let client = Client::load(db).map_err(|e| format!("读取配置失败: {e}"))?;
        *guard = Some(Arc::new(client));
    }
    Ok(guard.as_ref().unwrap().clone())
}

/// 每次发送现读 DB 组装轮次人设（毫秒级，读失败按无角色处理不阻断发送）：
/// 助手段 = `persona_id` 绑定的会话助手（未绑定/已删/悬空回落默认助手），
/// 用户人设仍全局一份。两段提示词与温度全空 → `None`（请求与旧行为逐位一致）。
fn persona_for_turn(db: &Db, persona_id: i64) -> Option<TurnPersona> {
    let a = db
        .assistant(persona_id)
        .ok()
        .flatten()
        .filter(|a| !a.deleted)
        .or_else(|| db.default_assistant().ok());
    let u = db.active_persona("user").ok().flatten();
    let p = TurnPersona {
        system_prompt: a
            .as_ref()
            .map_or_else(String::new, |x| x.system_prompt.clone()),
        user_persona: u.map_or_else(String::new, |x| x.system_prompt),
        temperature: a.as_ref().and_then(|x| x.temperature),
    };
    (!p.is_empty()).then_some(p)
}

/// 对话核心接线：发送（贴气泡 → 快照历史 → 后台请求）、停止、思考开关。
pub(super) fn wire_chat(window: &AppWindow, ctx: &Ctx) {
    let db = ctx.db.clone();
    let host = ctx.host.clone();
    let models = ctx.models.clone();
    let runtime = ctx.runtime.clone();
    let tx = ctx.tx.clone();
    let cache = ctx.cache.clone();

    // —— 发送路径：贴用户气泡 → 快照历史 → 后台请求（方式按配置分发） ——
    {
        let window_weak = window.as_weak();
        let runtime = runtime.clone();
        let tx = tx.clone();
        let cache = cache.clone();
        let host = host.clone();
        let models = models.clone();
        let db = db.clone();
        window.on_send(move |text| {
            let text: String = text.to_string();
            if text.trim().is_empty() {
                return;
            }
            let sid = host.current.get();
            let current = host.active_model();
            host.retitle_active(&text);
            current.push(ChatMessage {
                role: "user".into(),
                text: text.clone().into(),
                thinking: SharedString::default(),
                tstate: STATE_PARTIAL,
                tauto: true,
            });
            host.persist_user_message(sid, &text);
            let history = snapshot_history(&current);

            // 配置缺失/损坏只影响本轮发送，以内联气泡反馈，不退出也不卡 UI
            let client = match ensure_client(&cache, &db) {
                Ok(c) => c,
                Err(msg) => {
                    append_part(&current, false, &format!("[配置错误] {msg}"));
                    return;
                }
            };
            // 登记轮次（gen_id 递增、记录本轮模型）后投任务；回流事件打 (sid, gen_id)
            // 标路由回原会话
            let active_model = models.active.borrow().clone();
            let gen_id = {
                let mut runs = host.runs.borrow_mut();
                let run = &mut runs[sid];
                run.gen_id += 1;
                run.model = active_model.clone();
                run.gen_id
            };
            window_weak.upgrade().unwrap().set_generating(true);
            let run = RunRef { sid, gen_id };
            let handle = client.spawn_turn(
                &runtime,
                history,
                TurnOptions {
                    // Step C：当前模型声明不支持思考 → 即使开关开着也强制关
                    thinking: host.thinking_on.get() && models.supports_thinking(&active_model),
                    // 模型下拉的激活项；config 缺失时为空串→回落客户端默认模型
                    model: Some(active_model),
                    // 会话绑定助手（-1/悬空回落默认）；user 人设全局注入
                    persona: persona_for_turn(
                        &db,
                        host.persona_ids.borrow().get(sid).copied().unwrap_or(-1),
                    ),
                },
                {
                    let tx = tx.clone();
                    move |event| {
                        let _ = tx.send(UiMsg::Turn(StreamMsg { run, event }));
                    }
                },
            );
            host.runs.borrow_mut()[sid].cancel = Some(handle);
        });
    }

    // —— 停止当前会话 / 思考模式开关 ——
    {
        let window_weak = window.as_weak();
        window.on_stop({
            let host = host.clone();
            move || {
                let sid = host.current.get();
                if let Some(handle) = host.runs.borrow_mut()[sid].cancel.take() {
                    handle.cancel(); // 任务内请求 future 被丢弃 → 连接即断
                }
                if let Some(w) = window_weak.upgrade() {
                    w.set_generating(false);
                }
            }
        });
        let window_weak = window.as_weak();
        window.on_toggle_thinking({
            let host = host.clone();
            move || {
                host.thinking_on.set(!host.thinking_on.get());
                if let Some(w) = window_weak.upgrade() {
                    let on = host.thinking_on.get();
                    w.set_thinking_on(on);
                }
            }
        });
    }
}

/// 回流接线：Timer 排空事件队列，按 (sid, gen) 路由写回发起会话。
pub(super) fn wire_timer(window: &AppWindow, ctx: &Ctx, rx: mpsc::Receiver<UiMsg>) {
    let window_weak = window.as_weak();
    let timer_host = ctx.host.clone();
    let timer_models = ctx.models.clone();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(30), move || {
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        for msg in rx.try_iter() {
            let StreamMsg { run, event } = match msg {
                UiMsg::Turn(s) => s,
                UiMsg::Models(Ok(ids)) => {
                    // 远端清单并入下拉（去重保序），失败重试留待下回打开
                    timer_models.rebuild(Some(&ids));
                    timer_models.apply(&window);
                    continue;
                }
                UiMsg::Models(Err(e)) => {
                    eprintln!("拉取远端模型清单失败: {e}");
                    timer_models.fetched.set(false);
                    continue;
                }
                UiMsg::ProvTest(msg) => {
                    on_test_result(&window, msg);
                    continue;
                }
                UiMsg::ProvFetch(result) => {
                    on_fetch_result(&window, result);
                    continue;
                }
            };
            // 该会话已停止/重发（gen_id 前进过）：旧轮迟到事件一律丢弃
            {
                let runs = timer_host.runs.borrow();
                let Some(state) = runs.get(run.sid) else {
                    continue;
                };
                if state.gen_id != run.gen_id {
                    continue;
                }
            }
            let model = timer_host.models.borrow()[run.sid].clone();
            match event {
                TurnEvent::Reasoning(t) => append_part(&model, true, &t),
                TurnEvent::Content(t) => append_part(&model, false, &t),
                TurnEvent::Done => {
                    fold_tail(&model); // 兜底：纯思考回复也要收起
                    timer_host.runs.borrow_mut()[run.sid].cancel = None;
                    timer_host.persist_turn(run.sid);
                    if run.sid == timer_host.current.get() {
                        window.set_generating(false);
                    }
                }
                TurnEvent::Error(e) => {
                    // 错误详情截断，防止超长网关响应体撑爆气泡
                    let brief: String = e.chars().take(300).collect();
                    append_part(&model, false, &format!("\n[出错] {brief}"));
                    timer_host.runs.borrow_mut()[run.sid].cancel = None;
                    timer_host.persist_turn(run.sid);
                    if run.sid == timer_host.current.get() {
                        window.set_generating(false);
                    }
                }
            }
        }
    });
    // 定时器与应用同生命周期，故意泄漏（进程退出即回收）
    std::mem::forget(timer);
}
