//! 记忆检索三工具的 schema、注册与模式可见性（h2-memory-search）。
//!
//! schema 层照抄 codex 的三个 tools：参数名 / 默认值 / 上限 / `deny_unknown_fields`
//! 一项不改；本仓唯一刻意偏离是 `user:` / `project:` 双根前缀。后端行为验收在
//! `crates/agent-memory/tests/memory_search.rs`（本 crate 有 serde_json，故 schema
//! 反序列化用例落在这里）。

use agent_core::mode::{mode_policy, MEMORY_READONLY_TOOLS, READONLY_TOOLS, WRITE_TOOLS};
use agent_core::schema::Mode;
use agent_core::tool::{
    default_registry, parse_memory_list_args, parse_memory_read_args, parse_memory_search_args,
    ToolKind, MEMORY_CURRENT_SESSION_NOT_SEARCHABLE, MEMORY_LIST_TOOL, MEMORY_PATH_SEMANTICS,
    MEMORY_READ_TOOL, MEMORY_SEARCH_TOOL, MEMORY_SEARCH_TOOL_SPECS,
};
use agent_memory::{
    SearchMatchMode, DEFAULT_LIST_MAX_RESULTS, DEFAULT_READ_MAX_TOKENS, DEFAULT_SEARCH_MAX_RESULTS,
    MAX_LIST_RESULTS, MAX_SEARCH_RESULTS,
};
use serde_json::json;

/// 上限常量逐项照抄 codex（`ext/memories/src/lib.rs:11-15`）。
#[test]
fn limits_match_codex_constants() {
    assert_eq!(DEFAULT_LIST_MAX_RESULTS, 2_000);
    assert_eq!(MAX_LIST_RESULTS, 2_000);
    assert_eq!(DEFAULT_SEARCH_MAX_RESULTS, 200);
    assert_eq!(MAX_SEARCH_RESULTS, 200);
    assert_eq!(DEFAULT_READ_MAX_TOKENS, 20_000);
}

/// 多传未声明字段 ⇒ 反序列化失败（三工具都启用 `deny_unknown_fields`）。
#[test]
fn unknown_fields_are_rejected_for_all_three_tools() {
    let list = parse_memory_list_args(&json!({"path": "user:", "bogus": 1}));
    assert!(list.is_err(), "memory.list 未知字段必须报错：{list:?}");
    assert!(format!("{:?}", list.err()).contains("bogus"));

    let search = parse_memory_search_args(&json!({"queries": ["x"], "bogus": 1}));
    assert!(
        search.is_err(),
        "memory.search 未知字段必须报错：{search:?}"
    );
    assert!(format!("{:?}", search.err()).contains("bogus"));

    let read = parse_memory_read_args(&json!({"path": "MEMORY.md", "bogus": 1}));
    assert!(read.is_err(), "memory.read 未知字段必须报错：{read:?}");
    assert!(format!("{:?}", read.err()).contains("bogus"));
}

/// 默认值逐项对齐 codex：search 的 `case_sensitive` 默认 **true**。
#[test]
fn defaults_match_codex() {
    let list = parse_memory_list_args(&json!({})).expect("list 可省略全部参数");
    assert_eq!(list.path, None);
    assert_eq!(list.cursor, None);
    assert_eq!(list.max_results, DEFAULT_LIST_MAX_RESULTS);

    let search = parse_memory_search_args(&json!({"queries": ["x"]})).expect("search 最小参数");
    assert_eq!(search.match_mode, SearchMatchMode::Any);
    assert_eq!(search.path, None);
    assert_eq!(search.cursor, None);
    assert_eq!(search.context_lines, 0);
    assert!(search.case_sensitive, "case_sensitive 默认必须是 true");
    assert!(!search.normalized, "normalized 默认必须是 false");
    assert_eq!(search.max_results, DEFAULT_SEARCH_MAX_RESULTS);

    let read = parse_memory_read_args(&json!({"path": "MEMORY.md"})).expect("read 最小参数");
    assert_eq!(read.line_offset, 1);
    assert_eq!(read.max_lines, None);
    assert_eq!(
        read.max_tokens, DEFAULT_READ_MAX_TOKENS,
        "max_tokens 不由模型控制"
    );
}

/// 上限夹取在参数层完成：`99999 → MAX`；`0` 参数校验失败。
#[test]
fn max_results_is_clamped_and_zero_is_rejected() {
    let list = parse_memory_list_args(&json!({"max_results": 99_999})).expect("list");
    assert_eq!(list.max_results, MAX_LIST_RESULTS);
    assert!(parse_memory_list_args(&json!({"max_results": 0})).is_err());

    let search = parse_memory_search_args(&json!({"queries": ["x"], "max_results": 99_999}))
        .expect("search");
    assert_eq!(search.max_results, MAX_SEARCH_RESULTS);
    assert!(
        parse_memory_search_args(&json!({"queries": ["x"], "max_results": 0})).is_err(),
        "max_results=0 必须参数校验失败"
    );
}

/// `queries` 空数组/空白项/缺失 ⇒ 校验失败（不是「返回空结果」）。
#[test]
fn empty_queries_fail_validation() {
    for args in [
        json!({"queries": []}),
        json!({"queries": [""]}),
        json!({"queries": ["ok", "  "]}),
        json!({}),
    ] {
        let result = parse_memory_search_args(&args);
        assert!(result.is_err(), "queries={args} 必须校验失败：{result:?}");
        assert!(format!("{:?}", result.err()).contains("queries"));
    }
}

/// `line_offset` / `max_lines` / `all_within_lines.line_count` 的 min 约束。
#[test]
fn zero_minima_fail_validation() {
    assert!(parse_memory_read_args(&json!({"path": "x", "line_offset": 0})).is_err());
    assert!(parse_memory_read_args(&json!({"path": "x", "max_lines": 0})).is_err());
    assert!(parse_memory_search_args(&json!({
        "queries": ["x"],
        "match_mode": {"type": "all_within_lines", "line_count": 0}
    }))
    .is_err());
    // 0 在允许范围内的参数不受影响：context_lines 的 min 是 0。
    assert!(parse_memory_search_args(&json!({
        "queries": ["x"],
        "context_lines": 0
    }))
    .is_ok());
}

/// `SearchMatchMode` 的 serde 形状与 codex 一致：`type` 标签 + snake_case。
#[test]
fn search_match_mode_serde_shape_matches_codex() {
    assert_eq!(
        serde_json::from_value::<SearchMatchMode>(json!({"type": "any"})).unwrap(),
        SearchMatchMode::Any
    );
    assert_eq!(
        serde_json::from_value::<SearchMatchMode>(json!({"type": "all_on_same_line"})).unwrap(),
        SearchMatchMode::AllOnSameLine
    );
    assert_eq!(
        serde_json::from_value::<SearchMatchMode>(json!({
            "type": "all_within_lines",
            "line_count": 3
        }))
        .unwrap(),
        SearchMatchMode::AllWithinLines { line_count: 3 }
    );
    assert_eq!(
        serde_json::to_value(SearchMatchMode::Any).unwrap(),
        json!({"type": "any"})
    );
}

/// 三个工具注册进默认注册表，`kind = Read`。
#[test]
fn default_registry_registers_three_read_tools() {
    let registry = default_registry();
    for (name, description) in MEMORY_SEARCH_TOOL_SPECS {
        let spec = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} 必须注册进默认注册表"));
        assert_eq!(spec.kind, ToolKind::Read, "{name} 应为只读类");
        assert!(
            !spec.sensitive_arguments,
            "{name} 只读检索不应触发敏感动作扫描"
        );
        assert_eq!(spec.description, description);
    }
    // 只守关系，不锁数量：注册表规模会随工具接入增长（默认注册表现含
    // 4 个记忆写工具 + 3 个记忆检索工具 + 其他域工具）。
    for (name, _) in MEMORY_SEARCH_TOOL_SPECS.iter() {
        assert!(registry.get(name).is_some(), "{name} 必须仍在默认注册表里");
    }
}

/// 工具描述必须写明参数名、双根前缀语义与「当前会话不可查」纪律。
#[test]
fn tool_descriptions_carry_params_path_semantics_and_discipline() {
    let registry = default_registry();

    let list = registry.get(MEMORY_LIST_TOOL).expect("memory.list");
    for param in ["path", "cursor", "max_results"] {
        assert!(list.description.contains(param), "list 描述缺参数 {param}");
    }
    assert!(list.description.contains(MEMORY_PATH_SEMANTICS));

    let search = registry.get(MEMORY_SEARCH_TOOL).expect("memory.search");
    for param in [
        "queries",
        "match_mode",
        "path",
        "cursor",
        "context_lines",
        "case_sensitive",
        "normalized",
        "max_results",
    ] {
        assert!(
            search.description.contains(param),
            "search 描述缺参数 {param}"
        );
    }
    assert!(search.description.contains(MEMORY_PATH_SEMANTICS));
    assert!(
        search
            .description
            .contains(MEMORY_CURRENT_SESSION_NOT_SEARCHABLE),
        "search 描述必须含「当前会话不可查」纪律原文"
    );
    assert!(search.description.contains("默认 true"));

    let read = registry.get(MEMORY_READ_TOOL).expect("memory.read");
    for param in ["path", "line_offset", "max_lines"] {
        assert!(read.description.contains(param), "read 描述缺参数 {param}");
    }
    assert!(read.description.contains(MEMORY_PATH_SEMANTICS));
    assert!(
        read.description
            .contains(MEMORY_CURRENT_SESSION_NOT_SEARCHABLE),
        "read 描述必须含「当前会话不可查」纪律原文"
    );
}

/// 模式可见性：Craft / Expert 可见；Ask / Plan / Quick 不可见；不进写集。
#[test]
fn memory_search_tools_are_visible_only_in_craft_and_expert() {
    for name in MEMORY_READONLY_TOOLS {
        assert!(
            !READONLY_TOOLS.contains(&name),
            "{name} 不得进 Ask 只读白名单"
        );
        assert!(!WRITE_TOOLS.contains(&name), "{name} 不得进写集");
        assert!(mode_policy(Mode::Craft).allows(name), "craft 应可见 {name}");
        assert!(
            mode_policy(Mode::Expert).allows(name),
            "expert 应可见 {name}"
        );
        assert!(
            !mode_policy(Mode::Plan).allows(name),
            "plan 不应可见 {name}"
        );
        assert!(!mode_policy(Mode::Ask).allows(name), "ask 不应可见 {name}");
        assert!(
            !mode_policy(Mode::Quick).allows(name),
            "quick 不应可见 {name}"
        );
    }
    assert_eq!(
        MEMORY_READONLY_TOOLS,
        [MEMORY_LIST_TOOL, MEMORY_SEARCH_TOOL, MEMORY_READ_TOOL]
    );
}
