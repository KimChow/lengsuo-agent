//! 团队运行契约（§8.4）：`TaskPacket` / `MemberHandoff` 的线格式、往返与关联。
//!
//! ⛔ 只测 schema：不 spawn、不触盘、不跑阶段封存。

use super::support::*;
use serde_json::{json, Value};

fn packet() -> TaskPacket {
    TaskPacket {
        packet_id: "pkt-02-factor".to_string(),
        team_id: "quant-research-team".to_string(),
        from_member: "qr-research-team".to_string(),
        to_member: "factor-engineer".to_string(),
        objective: "把已封存的复现公式转为至少四个可执行候选".to_string(),
        context_refs: vec!["01_source_replication/factor_formula.md".to_string()],
        acceptance: vec!["候选数 >= 4".to_string(), "未来函数检查通过".to_string()],
        created_at: T0.to_string(),
    }
}

fn handoff(status: HandoffStatus) -> MemberHandoff {
    MemberHandoff {
        packet_id: "pkt-02-factor".to_string(),
        from_member: "factor-engineer".to_string(),
        status,
        summary: "形成 4 个有实质差异的候选".to_string(),
        artifacts: vec!["02_factor_candidates/candidates.jsonl".to_string()],
        open_questions: vec!["预算尚未审批".to_string()],
        created_at: T0.to_string(),
    }
}

/// `TaskPacket` → JSON → `TaskPacket` 往返相等，且键名是 camelCase。
#[test]
fn task_packet_round_trips() {
    let original = packet();
    let text = serde_json::to_string(&original).expect("应可序列化");
    let back: TaskPacket = serde_json::from_str(&text).expect("应可反序列化");
    assert_eq!(original, back);
    let value: Value = serde_json::from_str(&text).expect("应是 JSON");
    for key in [
        "packetId",
        "teamId",
        "fromMember",
        "toMember",
        "objective",
        "contextRefs",
        "acceptance",
        "createdAt",
    ] {
        assert!(value.get(key).is_some(), "缺线格式键 {key}: {text}");
    }
    assert!(
        value.get("packet_id").is_none(),
        "线格式不应出现 snake_case 键"
    );
}

/// `MemberHandoff` 四种 `status` 的线格式逐字断言（含未知取值兜底）。
#[test]
fn handoff_status_serializes_each_state() {
    let cases = [
        (HandoffStatus::Done, "done"),
        (HandoffStatus::Blocked, "blocked"),
        (HandoffStatus::NeedsHuman, "needsHuman"),
        (HandoffStatus::Unknown, "unknown"),
    ];
    for (status, wire) in cases {
        let value = serde_json::to_value(status).expect("应可序列化");
        assert_eq!(value, json!(wire), "{status:?}");
        let back: HandoffStatus = serde_json::from_value(value).expect("应可反序列化");
        assert_eq!(back, status);
    }
    assert!(HandoffStatus::Done.is_done());
    assert!(!HandoffStatus::Blocked.is_done());
    assert!(!HandoffStatus::Unknown.is_done(), "未知取值不算完成");
}

/// 线格式里出现未知状态 ⇒ 反序列化失败（不静默降级为完成）。
#[test]
fn handoff_rejects_off_schema_status() {
    let mut value = serde_json::to_value(handoff(HandoffStatus::Done)).expect("应可序列化");
    value["status"] = json!("completed");
    let result: Result<MemberHandoff, _> = serde_json::from_value(value);
    assert!(result.is_err(), "规格外的状态值不应被接受");
}

/// `MemberHandoff` 往返相等。
#[test]
fn member_handoff_round_trips() {
    let original = handoff(HandoffStatus::NeedsHuman);
    let text = serde_json::to_string(&original).expect("应可序列化");
    let back: MemberHandoff = serde_json::from_str(&text).expect("应可反序列化");
    assert_eq!(original, back);
    let value: Value = serde_json::from_str(&text).expect("应是 JSON");
    assert_eq!(value["packetId"], json!("pkt-02-factor"));
    assert_eq!(value["openQuestions"], json!(["预算尚未审批"]));
}

/// 关联判定：`packet_id` 相等才回指成功；不匹配 / 为空都 `false`。
#[test]
fn handoff_links_only_to_matching_packet() {
    let packet = packet();
    let done: Handoff = handoff(HandoffStatus::Done);
    assert!(done.links_to(&packet));

    let mut other = packet.clone();
    other.packet_id = "pkt-03-run".to_string();
    assert!(!handoff(HandoffStatus::Done).links_to(&other));

    let mut orphan = handoff(HandoffStatus::Blocked);
    orphan.packet_id = String::new();
    assert!(!orphan.links_to(&packet), "空 packetId 不得判为关联");
}

/// `TeamInfo.workflow`：默认 `None`（样本没有该键）；声明后往返一致。
#[test]
fn team_workflow_is_optional_and_round_trips() {
    let plain = parse(TEAM);
    let team = plain.team_info.as_ref().expect("应有 teamInfo");
    assert_eq!(team.workflow, None);

    let mut manifest = plain.clone();
    manifest.team_info.as_mut().expect("应有 teamInfo").workflow = Some(TeamWorkflow {
        schema_version: 1,
        task_packet_contract: Some("references/team-contract.md".to_string()),
        handoff_contract: Some("references/team-contract.md".to_string()),
        stages: vec![
            "01_source_replication".to_string(),
            "02_factor_candidates".to_string(),
        ],
    });
    let text = manifest.to_json_string().expect("应可序列化");
    assert!(text.contains("\"workflow\""), "声明后应写出 workflow 键");
    let back = ExpertManifest::from_json_str(&text).expect("应可解析");
    assert_eq!(back, manifest);

    // 未声明时序列化不吐 null 键（与既有可选字段同口径）。
    let plain_text = plain.to_json_string().expect("应可序列化");
    assert!(!plain_text.contains("workflow"));
}
