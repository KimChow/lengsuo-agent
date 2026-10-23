//! 事件帧**顶层键面**断言（W2 · lane proto）：线上帧最外层那几个键。
//!
//! 真相来源三处一致：`AGENT-KICKOFF.md` 第 35 行冻结
//! `Event { seq, session_id, kind, payload, ts_ms }`；`agent-gateway/src/server.rs`
//! 的真实推帧点；`lengsuo-shell/src/gateway/events.rs` 的 `GatewayEvent` 解码器
//! （无 `rename_all` ⇒ serde 按字段名原样匹配 ⇒ 只认 snake_case）。
//!
//! 本文件只守**关系**，刻意**不**锁数量、不锁取值：
//! - 不写 `assert_eq!(EVENT_FRAME_KEYS.len(), N)`；
//! - 不写 `assert_eq!(EVENT_FRAME_KEYS, &["seq", …])`。
//!
//! 源码用 `include_str!` 在**编译期**绑定（改 `server.rs` 即触发本测试重编）；
//! 扫描失败一律报出扫描器/源文件形态，避免「扫出空集 ⇒ 断言空转通过」。

use agent_core::provider::Uuid;
use agent_core::schema::Event;
use contracts_check::{EVENT_FRAME_KEYS, RUNTIME_EVENT_PAYLOAD_KEYS, TOOL_EVENT_PAYLOAD_KEYS};
use lengsuo_shell::gateway::{EventKind, GatewayEvent};

/// 网关真实推帧点的源码（**编译期**绑定）。
const SERVER_SOURCE: &str = include_str!("../../agent-gateway/src/server.rs");

// ---- 扫描器（手写字符扫描，不用正则库） ----

/// 是不是注释行（`//` / `///` / `//!` / `*` 开头）——注释里的 `json!` **不算**实现面。
fn is_comment_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('*')
}

/// 返回字符串字面量的**结束引号**位置（不含）；`bytes[start]` 是开引号。
fn string_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// 取 `json!({` 之后的**对象体**的**顶层键名**（按出现顺序）。
///
/// `open` 指向对象体的 `{`。字符串字面量整体跳过（内部的花括号不参与配对），
/// 只有深度 1 上的 `"键":` 才算顶层键 ⇒ 嵌套对象的键不会被误当成顶层键。
fn top_level_keys_of_json_object(source: &str, open: usize) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let Some(end) = string_literal_end(bytes, i) else {
                    break;
                };
                let lit = &source[i + 1..end];
                let mut k = end + 1;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if depth == 1 && k < bytes.len() && bytes[k] == b':' {
                    keys.push(lit.to_owned());
                }
                i = end + 1;
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    keys
}

/// 找出源码里所有**顶层键同时含 `seq` 与 `kind`** 的 `json!({...})` 对象，
/// 返回每个对象的顶层键名集合。
fn frame_push_key_sets(source: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(at) = source[offset..].find("json!({") {
        let start = offset + at;
        let open = start + "json!(".len();
        // 注释行里的 `json!` 不算实现面。
        let line_start = source[..start].rfind('\n').map_or(0, |p| p + 1);
        let line = &source[line_start..];
        if !is_comment_line(line) {
            let keys = top_level_keys_of_json_object(source, open);
            let has_seq = keys.iter().any(|k| k == "seq");
            let has_kind = keys.iter().any(|k| k == "kind");
            if has_seq && has_kind {
                out.push(keys);
            }
        }
        offset = start + "json!({".len();
    }
    out
}

// ---- 断言用的关系工具 ----

/// 双向差集：`missing` = 在 `sup` 里但不在 `sub` 里；`extra` = 在 `sub` 里但不在
/// `sup` 里。
fn key_diff<'a>(sub: &[&'a str], sup: &[&'a str]) -> (Vec<&'a str>, Vec<&'a str>) {
    (
        sup.iter().copied().filter(|k| !sub.contains(k)).collect(),
        sub.iter().copied().filter(|k| !sup.contains(k)).collect(),
    )
}

/// 登记面（帧顶层键名）的键集合。
fn registered_keys() -> Vec<&'static str> {
    EVENT_FRAME_KEYS.to_vec()
}

/// 取一个 JSON 对象的顶层键名。
fn object_keys(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_object()
        .expect("帧应是 JSON 对象")
        .keys()
        .map(String::as_str)
        .collect()
}

// ---- 断言 1：网关推帧点的键面 == 登记面（双向） ----

#[test]
fn gateway_push_point_keys_equal_registered_frame_keys() {
    let sets = frame_push_key_sets(SERVER_SOURCE);
    assert!(
        !sets.is_empty(),
        "server.rs 里没扫到含 `seq` + `kind` 顶层键的 `json!({{...}})` 推帧点——扫描器或源文件形态变了"
    );
    let registered = registered_keys();
    for keys in &sets {
        let scanned: Vec<&str> = keys.iter().map(String::as_str).collect();
        let (missing, extra) = key_diff(&scanned, &registered);
        assert!(
            missing.is_empty() && extra.is_empty(),
            "网关推帧点的顶层键面与登记面不一致：登记有而源码缺 {missing:?}；源码有而登记缺 {extra:?}"
        );
    }
}

// ---- 断言 2：登记面 ⊆ 壳侧解码器的线上键面（子集，不锁相等） ----

#[test]
fn registered_frame_keys_are_decodable_by_shell_gateway_event() {
    let decoded = serde_json::to_value(GatewayEvent {
        seq: 1,
        session_id: "s".to_owned(),
        kind: EventKind::Text,
        payload: serde_json::Value::Null,
        ts_ms: 2,
    })
    .expect("GatewayEvent 序列化失败");
    let decoder_keys = object_keys(&decoded);
    for key in EVENT_FRAME_KEYS {
        assert!(
            decoder_keys.contains(key),
            "登记的帧顶层键 {key} 不在壳侧解码器可解键面 {decoder_keys:?} 里"
        );
    }
}

// ---- 断言 3：`Event::to_envelope()` 的键面 == 登记面（双向） ----

#[test]
fn event_to_envelope_keys_equal_registered_frame_keys() {
    let event = Event {
        seq: 7,
        session_id: Uuid::nil(),
        kind: "text".to_owned(),
        payload: serde_json::json!({"note": "x"}),
        ts_ms: 1234,
    };
    let envelope = event.to_envelope();
    let keys = object_keys(&envelope);
    let (missing, extra) = key_diff(&keys, EVENT_FRAME_KEYS);
    assert!(
        missing.is_empty() && extra.is_empty(),
        "`Event::to_envelope()` 的键面与登记面不一致：登记有而实现缺 {missing:?}；实现有而登记缺 {extra:?}"
    );
}

// ---- 断言 4：帧顶层键面与事件载荷键面不相交（防顶层与载荷串键） ----

// ⚠️ 具名例外（2026-10-24）：`kind` 在帧顶层与载荷层都真实存在，不是实现缺陷——
//   帧顶层 `kind` = 事件类型判别符（`text` / `warning` / `session_update` …）；
//   载荷层 `kind` = 该事件的**子类**（`warning` 的 `memory_compose_failed` /
//   `skill_injection_rejected` / `expert_persona_unavailable` 等）。
//   生产源码 6 处 emit 点（`agent-core/src/runtime/sessions.rs:63,166`、
//   `agent-core/src/runtime/context.rs:118,135,206,318`）均以 `json!({ "kind": … })`
//   发出载荷层 `kind`。⇒「宽面登记 ⊇ 源文件全部载荷键」是实质不变量（防实现静默
//   加键），「宽面 ∩ 帧键 == ∅」是风格性防线；二者在事实上不可兼得 ⇒ 后者让位。
//   ⇒ 仅对 `kind` 开有据例外；其余任何重名照旧为红。
const FRAME_KEY_PAYLOAD_EXEMPT: &[&str] = &["kind"];

#[test]
fn frame_keys_do_not_collide_with_event_payload_keys() {
    // 防豁免腐烂：例外项必须真的在帧顶层键面里；`kind` 一旦不再是帧键即应移除例外。
    for k in FRAME_KEY_PAYLOAD_EXEMPT {
        assert!(
            EVENT_FRAME_KEYS.contains(k),
            "豁免项 {k:?} 已不在帧顶层键面，请从 FRAME_KEY_PAYLOAD_EXEMPT 移除"
        );
    }
    for (name, payload_keys) in [
        ("TOOL_EVENT_PAYLOAD_KEYS", TOOL_EVENT_PAYLOAD_KEYS),
        ("RUNTIME_EVENT_PAYLOAD_KEYS", RUNTIME_EVENT_PAYLOAD_KEYS),
    ] {
        let overlap: Vec<&str> = EVENT_FRAME_KEYS
            .iter()
            .copied()
            .filter(|k| payload_keys.contains(k))
            .filter(|k| !FRAME_KEY_PAYLOAD_EXEMPT.contains(k))
            .collect();
        assert!(
            overlap.is_empty(),
            "帧顶层键面与 {name} 的载荷键面相交 {overlap:?}——顶层与载荷同名会让「键在哪个面」失效"
        );
    }
}

// ---- 断言 5：线上帧形态与落盘形态**故意不同**（两个面，不是同一个面） ----

#[test]
fn envelope_form_and_disk_form_are_different_surfaces() {
    let event = Event {
        seq: 7,
        session_id: Uuid::nil(),
        kind: "text".to_owned(),
        payload: serde_json::json!({"note": "x"}),
        ts_ms: 1234,
    };
    // 落盘形态：`Event` 的 serde 形态（camelCase，供事件日志 JSONL 用）。
    let disk = serde_json::to_value(&event).expect("Event 序列化失败");
    let disk_keys = object_keys(&disk);
    // 线上形态：`to_envelope()` 的手工构造形态（snake_case，供订阅者用）。
    let envelope = event.to_envelope();
    let envelope_keys = object_keys(&envelope);
    let (missing, extra) = key_diff(&envelope_keys, &disk_keys);
    assert!(
        !(missing.is_empty() && extra.is_empty()),
        "`to_envelope()` 的键面与落盘 serde 键面相同 {envelope_keys:?}——两者本应是不同的面"
    );
    // 落盘面**不是**线上帧面。
    let (missing, extra) = key_diff(&disk_keys, EVENT_FRAME_KEYS);
    assert!(
        !(missing.is_empty() && extra.is_empty()),
        "落盘 serde 键面 {disk_keys:?} 与线上帧面相同——两者本应是不同的面"
    );
}

// ---- 扫描器自测（喂假源码 ⇒ 断言扫描器只取该取的） ----

#[test]
fn scanner_finds_only_frame_like_json_objects() {
    // 自测用固定夹具 ⇒ 这里的数量/取值断言是**扫描器**的规格，不是契约面。
    let src = r#"
let a = json!({"code":404,"msg":"unknown route","data":null});
let b = json!({"seq":event.seq,"session_id":event.session_id,"kind":event.kind,"payload":event.payload,"ts_ms":event.ts_ms});
let c = json!({"outer":{"seq":1,"kind":"x"},"seq":2,"kind":"y"});
// let d = json!({"seq":3,"kind":"z"});
"#;
    let sets = frame_push_key_sets(src);
    assert_eq!(
        sets.len(),
        2,
        "只应认出两个含 `seq` + `kind` 顶层键的对象：{sets:?}"
    );
    assert_eq!(
        sets[0],
        vec!["seq", "session_id", "kind", "payload", "ts_ms"],
        "扁平帧的顶层键取错"
    );
    assert_eq!(
        sets[1],
        vec!["outer", "seq", "kind"],
        "嵌套对象的键不得被当成顶层键"
    );
}
