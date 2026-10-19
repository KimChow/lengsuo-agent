//! 上下文总预算裁剪集成测试（W1 context-budget 卡）。
//!
//! 验收对象是**真实 Runtime 回合链**：`Runtime::send_text` → `run_turn` →
//! `build_turn_context`（第一道闸：逐条收窄到 2000 字符；第二道闸：总预算裁剪）→
//! `Provider::next_steps_with_context`。断言针对 `Capturing` provider 捕获的
//! `TurnContext.history`——`runtime::context` 是私有模块，裁剪逻辑从 crate 外
//! 只能经回合链观测（与 provider-context 卡同一测试口径）。
//!
//! `HISTORY_TOTAL_CHARS` / `HISTORY_MESSAGE_CHARS` 是对 src 私有常量的**契约镜像**：
//! 源常量一旦变动，超预算用例会失配，迫使其同步更新本契约测试。

use std::sync::{Arc, Mutex};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Isolation, Message, Mode, PermissionMode, Role};
use serde_json::json;

/// 历史总预算（字符口径），镜像 `src/runtime/context.rs` 的同名常量。
const HISTORY_TOTAL_CHARS: usize = 24_000;
/// 单条收窄上限，镜像 `src/runtime/context.rs` 的 `HISTORY_MESSAGE_CHARS`。
const HISTORY_MESSAGE_CHARS: usize = 2000;
/// 单条收窄的截断尾标，镜像 `src/runtime.rs` 的 `narrow` 追加文本。
const NARROW_MARK: &str = "…（截断，完整结果见 result_ref）";

/// 捕获型 provider：记录每次回合上下文，回复固定长度的字母串（可精确控字数）。
struct Capturing {
    seen: Mutex<Vec<TurnContext>>,
    reply: String,
}

impl Capturing {
    /// `reply_chars` 为 assistant 回复的字符数（内容为 `a` 重复，可安全过脱敏器）。
    fn new(reply_chars: usize) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            reply: "a".repeat(reply_chars),
        })
    }

    /// 取出并清空已捕获上下文。
    fn take(&self) -> Vec<TurnContext> {
        std::mem::take(&mut *self.seen.lock().expect("捕获锁"))
    }
}

impl Provider for Capturing {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "上下文预算测试必须走带上下文的回合接口",
        ))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen.lock().expect("捕获锁").push(context.clone());
        Ok(vec![
            ProviderStep::Text(self.reply.clone()),
            ProviderStep::Finished,
        ])
    }
}

/// 起一个真实 Runtime：memory 关闭，provider 注入 Capturing。
fn runtime(reply_chars: usize) -> (Arc<Capturing>, Runtime) {
    let dir = std::env::temp_dir().join(format!("budget-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let capture = Capturing::new(reply_chars);
    let mut config = RuntimeConfig::default_v0();
    config.memory_enabled = false;
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(capture.clone())
        .config(config)
        .build()
        .expect("runtime");
    (capture, rt)
}

/// 新建会话（ask 模式即可：预算裁剪与模式无关）。
fn session(rt: &Runtime) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "预算裁剪卡".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode: Mode::Ask,
        permission_mode: PermissionMode::ReadOnly,
        env_binding: None,
        isolation: Isolation::local_default(),
    })
    .expect("create session")
}

/// 第 k 轮的用户输入：30 字符且各轮互不相同（便于按内容定位丢弃边界）。
fn user_text(k: usize) -> String {
    format!("u{k:02}").repeat(10)
}

/// 经宿主回填路径落一组相邻的 ToolCall + ToolResult（结果恰 400 字符，不触发 blob 预览）。
fn record_pair(rt: &Runtime, id: uuid::Uuid, tool: &str) {
    let args = json!({ "k": "v" });
    rt.record_tool_call(id, tool, &args)
        .expect("record tool call");
    rt.record_tool_result(id, tool, args, &"r".repeat(400))
        .expect("record tool result");
}

/// 历史总字符数（与裁剪口径一致：逐条 content 字符数求和）。
fn total_chars(history: &[Message]) -> usize {
    history.iter().map(|m| m.content.chars().count()).sum()
}

/// 历史投影（角色 + 内容）：比较裁剪结果时的时钟无关口径（ts 属历史数据，原样携带）。
fn project(history: &[Message]) -> Vec<(Role, String)> {
    history
        .iter()
        .map(|m| (m.role, m.content.clone()))
        .collect()
}

/// 名为 `tool` 的工具消息（调用与/或结果）。
fn tool_msgs<'a>(history: &'a [Message], tool: &str) -> Vec<&'a Message> {
    history
        .iter()
        .filter(|m| m.tool.as_ref().is_some_and(|t| t.name == tool))
        .collect()
}

/// 断言历史中不存在孤儿：ToolCall 必后随同名 ToolResult，ToolResult 必前有同名 ToolCall。
fn assert_no_orphan_tools(history: &[Message]) {
    for (i, m) in history.iter().enumerate() {
        match m.role {
            Role::ToolCall => {
                let next = history
                    .get(i + 1)
                    .expect("ToolCall 之后必须还有 ToolResult，不得截在配对中间");
                assert_eq!(next.role, Role::ToolResult, "ToolCall 孤儿: 位置 {i}");
                let (a, b) = (m.tool.as_ref(), next.tool.as_ref());
                assert_eq!(
                    a.map(|t| t.name.as_str()),
                    b.map(|t| t.name.as_str()),
                    "ToolCall 与其后 ToolResult 必须同工具名: 位置 {i}"
                );
            }
            Role::ToolResult => {
                assert!(i > 0, "ToolResult 不得出现在输出头部（孤儿结果）");
                let prev = &history[i - 1];
                assert_eq!(prev.role, Role::ToolCall, "ToolResult 孤儿: 位置 {i}");
                let (a, b) = (prev.tool.as_ref(), m.tool.as_ref());
                assert_eq!(
                    a.map(|t| t.name.as_str()),
                    b.map(|t| t.name.as_str()),
                    "ToolResult 与其前 ToolCall 必须同工具名: 位置 {i}"
                );
            }
            _ => {}
        }
    }
}

/// 1) 未超预算 ⇒ 输出与输入逐条相等（顺序、角色、内容、时间戳都不变）。
#[test]
fn under_budget_history_passes_through_verbatim() {
    let (capture, rt) = runtime(60);
    let s = session(&rt);
    rt.send_text(s.id, "第一轮输入").expect("turn 1");
    rt.send_text(s.id, "第二轮输入").expect("turn 2");
    capture.take();

    let snapshot = rt.get_session(s.id).expect("snapshot").messages;
    assert!(
        total_chars(&snapshot) <= HISTORY_TOTAL_CHARS,
        "前置条件：快照未超总预算"
    );
    rt.send_text(s.id, "第三轮输入").expect("turn 3");
    let ctx = capture.take().remove(0);
    assert_eq!(
        ctx.history, snapshot,
        "未超预算：历史必须逐条原样通过（不重排、不改写、不丢）"
    );
}

/// 2) 超预算 ⇒ 只丢最旧整段，最近消息保留，总数下降且不超预算。
#[test]
fn over_budget_drops_only_oldest_and_stays_within_budget() {
    // 每轮 = user 30 字符 + assistant 1900 字符 = 1930；14 轮 = 27020 > 24000。
    // 从最新端累加：12 轮 = 23160 ≤ 24000，13 轮 = 25090 > 24000
    // ⇒ 保留最新 12 轮（24 条），丢弃最旧 2 轮（4 条）。
    let (capture, rt) = runtime(1900);
    let s = session(&rt);
    for k in 1..=14 {
        rt.send_text(s.id, &user_text(k)).expect("turn");
    }
    capture.take();

    let snapshot = rt.get_session(s.id).expect("snapshot").messages;
    let before = total_chars(&snapshot);
    assert!(
        before > HISTORY_TOTAL_CHARS,
        "前置条件：快照必须超总预算（实际 {before}）"
    );
    rt.send_text(s.id, "超预算探针").expect("probe");
    let ctx = capture.take().remove(0);

    const EXPECTED_DROP: usize = 4;
    assert_eq!(
        ctx.history,
        snapshot[EXPECTED_DROP..].to_vec(),
        "超预算：必须恰好丢弃最旧 {EXPECTED_DROP} 条，其余逐条原样保留"
    );
    assert!(
        ctx.history.len() < snapshot.len(),
        "总数必须单调下降（丢过才停）"
    );
    let after = total_chars(&ctx.history);
    assert!(
        after <= HISTORY_TOTAL_CHARS,
        "裁剪后必须不超总预算（实际 {after}）"
    );
    assert_eq!(ctx.history.last(), snapshot.last(), "最近一条消息永不丢弃");
}

/// 3) 工具配对：连续 ToolCall+ToolResult 组被丢时两者一起消失，无孤儿调用/孤儿结果。
#[test]
fn tool_call_and_result_groups_drop_together() {
    // 布局：[6 组早段工具对][16 轮对话][1 组晚段工具对][2 轮对话]。
    // 总量 6×409 + 18×1930 + 409 = 37603 > 24000 ⇒ 需从最旧丢约 13.6k 字符：
    // 早段 6 组（2454）+ 最旧 6 轮（11580）整组消失，保留自第 7 轮起的后缀
    // （含晚段工具对，总量 23569 ≤ 24000）。
    let (capture, rt) = runtime(1900);
    let s = session(&rt);
    for k in 1..=6 {
        record_pair(&rt, s.id, &format!("early{k}"));
    }
    for k in 1..=16 {
        rt.send_text(s.id, &user_text(k)).expect("turn");
    }
    record_pair(&rt, s.id, "late7");
    for k in 17..=18 {
        rt.send_text(s.id, &user_text(k)).expect("turn");
    }
    capture.take();

    let snapshot = rt.get_session(s.id).expect("snapshot").messages;
    rt.send_text(s.id, "配对探针").expect("probe");
    let ctx = capture.take().remove(0);

    const EXPECTED_DROP: usize = 24;
    assert_eq!(
        ctx.history,
        snapshot[EXPECTED_DROP..].to_vec(),
        "丢弃必须以整组为单位：早段 6 组工具对 + 最旧 6 轮恰好构成丢弃前缀"
    );
    for k in 1..=6 {
        let name = format!("early{k}");
        assert!(
            tool_msgs(&ctx.history, &name).is_empty(),
            "被丢的工具组必须调用与结果一起消失：{name} 仍有残留"
        );
    }
    assert_eq!(
        tool_msgs(&ctx.history, "late7").len(),
        2,
        "保留区的工具组必须调用与结果成对在列"
    );
    assert_no_orphan_tools(&ctx.history);
    assert!(
        total_chars(&ctx.history) <= HISTORY_TOTAL_CHARS,
        "裁剪后必须不超总预算（实际 {}）",
        total_chars(&ctx.history)
    );
}

/// 4) 单条超大消息仍被 HISTORY_MESSAGE_CHARS 收窄（第一道闸没被总预算绕过）。
#[test]
fn oversized_single_message_still_narrowed_by_first_gate() {
    let (capture, rt) = runtime(5000);
    let s = session(&rt);
    rt.send_text(s.id, "短短的输入").expect("turn 1");
    capture.take();

    rt.send_text(s.id, "第二输入").expect("turn 2");
    let ctx = capture.take().remove(0);
    assert_eq!(
        ctx.history.len(),
        2,
        "总字符未超预算 ⇒ 不得因预算丢消息: {:?}",
        ctx.history
    );
    assert_eq!(ctx.history[0].role, Role::User);
    assert_eq!(ctx.history[0].content, "短短的输入");
    let narrowed = &ctx.history[1];
    assert_eq!(narrowed.role, Role::Assistant);
    let expected = format!("{}{}", "a".repeat(HISTORY_MESSAGE_CHARS), NARROW_MARK);
    assert_eq!(
        narrowed.content, expected,
        "超大单条必须先过第一道闸（2000 字符 + 截断尾标）"
    );
    assert_eq!(
        narrowed.content.chars().count(),
        HISTORY_MESSAGE_CHARS + NARROW_MARK.chars().count()
    );
}

/// 5) 空历史 ⇒ 空输出，不 panic，不伪造占位消息。
#[test]
fn empty_history_yields_empty_trimmed_history() {
    let (capture, rt) = runtime(30);
    let s = session(&rt);
    rt.send_text(s.id, "首轮输入").expect("turn");
    let ctx = capture.take().remove(0);
    assert!(ctx.history.is_empty(), "空历史必须裁出空历史");
    assert_eq!(ctx.user_text, "首轮输入");
}

/// 6) 确定性：同一输入连续调用两次结果相等；结果只依赖消息序列（与落表时刻无关）。
#[test]
fn same_input_produces_identical_trimmed_output() {
    let (capture, rt) = runtime(1900);
    let a = session(&rt);
    for k in 1..=14 {
        rt.send_text(a.id, &user_text(k)).expect("turn A");
    }
    // 会话 B 的消息序列与 A 完全相同，但落表时刻不同（不同会话、不同时间戳）。
    let b = session(&rt);
    for k in 1..=14 {
        rt.send_text(b.id, &user_text(k)).expect("turn B");
    }
    capture.take();
    // 快照取自探针之前（探针自身不入被裁历史）。
    let snap_a = rt.get_session(a.id).expect("snapshot A").messages;
    let snap_b = rt.get_session(b.id).expect("snapshot B").messages;
    assert_eq!(project(&snap_a), project(&snap_b));

    // 连续两次探针：两次 build_turn_context 的输入相同 ⇒ 输出必须相同。
    rt.send_text(a.id, "确定性探针").expect("probe A");
    let ctx_a = capture.take().remove(0);
    rt.send_text(b.id, "确定性探针").expect("probe B");
    let ctx_b = capture.take().remove(0);

    assert_eq!(
        project(&ctx_a.history),
        project(&ctx_b.history),
        "同输入必须同输出（裁剪不读时钟/环境/会话状态）"
    );
    // 与输入顺序无关的稳定性：输出都是各自快照按同一规则的同一形态后缀。
    assert_eq!(project(&ctx_a.history), project(&snap_a[4..]));
    assert_eq!(project(&ctx_b.history), project(&snap_b[4..]));
}
