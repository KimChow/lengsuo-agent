//! 输出脱敏过滤器：密码/token/私钥正则族（开工说明 §4、§6；凭据不得进事件流）。

use agent_core::redact::Redactor;

#[test]
fn redacts_common_credential_shapes() {
    let r = Redactor::default_v0();
    let cases = [
        ("password=hunter2please", "hunter2please"),
        ("PASSWORD : s3cr3t-word", "s3cr3t-word"),
        (
            r#"{"api_key":"AKIAEXAMPLEKEY12" , "x":1}"#,
            "AKIAEXAMPLEKEY12",
        ),
        (
            "token: ghp_AbCdEf1234567890AbCd",
            "ghp_AbCdEf1234567890AbCd",
        ),
        (
            "Authorization: Bearer mj-val_9d8c7b6a",
            "Bearer mj-val_9d8c7b6a",
        ),
        ("secret = \"dont-leak-me\"", "dont-leak-me"),
        ("client_secret=zz_1234567890abcdef", "zz_1234567890abcdef"),
    ];
    for (input, leak) in cases {
        let out = r.redact_str(input);
        assert!(!out.contains(leak), "未脱敏：{input:?} -> {out:?}");
        assert!(out.contains("***"), "应有脱敏标记：{out:?}");
        // 键名保留，便于排障
        let lowered = out.to_ascii_lowercase();
        assert!(
            lowered.contains("password")
                || lowered.contains("token")
                || lowered.contains("key")
                || lowered.contains("secret")
                || lowered.contains("authorization"),
            "键名应保留：{out:?}"
        );
    }
}

#[test]
fn redacts_private_key_blocks() {
    let r = Redactor::default_v0();
    let pem = "-----BEGIN PRIVATE KEY-----\nTU9DS0VZTU9DS0VZ\n-----END PRIVATE KEY-----\n";
    let out = r.redact_str(pem);
    assert!(!out.contains("TU9DS0VZTU9DS0VZ"), "私钥体必须移除：{out}");
    assert!(!out.contains("PRIVATE KEY"), "私钥块整体替换：{out}");
    assert!(out.contains(r#"{"redacted""#), "占位为脱敏对象：{out}");
}

#[test]
fn leaves_ordinary_text_untouched() {
    let r = Redactor::default_v0();
    for input in [
        "compiled 3 crates in 1.2s",
        "GET /rpc 200 OK in 4ms",
        "the token count is low today",      // 无值，不脱敏
        "password policy requires 12 chars", // 政策描述，无凭据值
    ] {
        assert_eq!(r.redact_str(input), input, "普通文本不得改动");
    }
    assert!(!r.needs_redaction("plain line\nsecond line"));
}

#[test]
fn handles_long_input_by_chunks_and_redacts_every_hit() {
    let r = Redactor::default_v0();
    let noise = "a".repeat(5_000);
    let input = format!("{noise}password=leak-one {noise}token: leak-two {noise}");
    let out = r.redact_str(&input);
    assert!(!out.contains("leak-one"), "第一处未脱敏");
    assert!(!out.contains("leak-two"), "第二处未脱敏");
    assert!(out.contains(&noise), "非敏感片段保留");
}

#[test]
fn redacts_json_payload_in_place() {
    let r = Redactor::default_v0();
    let value = serde_json::json!({
        "stdout": "ok",
        "headers": {"authorization": "Bearer leaked-value-123"},
        "nested": {"items": ["line with password=inline-leak", "clean line"]},
    });
    let out = r.redact_json(&value);
    let text = serde_json::to_string(&out).expect("serialize");
    assert!(!text.contains("leaked-value-123"), "{text}");
    assert!(!text.contains("inline-leak"), "{text}");
    assert_eq!(out["stdout"], "ok", "非敏感字段原样保留");
    assert_eq!(out["nested"]["items"][1], "clean line");
}

#[test]
fn redaction_is_idempotent_and_marks_output() {
    let r = Redactor::default_v0();
    let once = r.redact_str("password=abcdef123456");
    let twice = r.redact_str(&once);
    assert_eq!(once, twice, "二次脱敏不得改变结果");
    assert!(r.needs_redaction("password=abcdef123456"));
    assert!(!r.needs_redaction(&once), "已脱敏文本无需再标红");
}

// ---- 形态规则（对照 codex-rs/secrets/src/sanitizer.rs） ----

#[test]
fn redacts_bare_openai_and_aws_tokens() {
    let r = Redactor::default_v0();
    let cases = [
        // 键名 X-Api 不在键名族内，只能靠 sk- 形态规则。
        (
            r#"curl -H "X-Api: sk-abcdefghijklmnopqrstuvwxyz""#,
            "sk-abcdefghijklmnopqrstuvwxyz",
        ),
        (
            "aws_access_key_id = AKIAIOSFODNN7EXAMPLE",
            "AKIAIOSFODNN7EXAMPLE",
        ),
    ];
    for (input, leak) in cases {
        let out = r.redact_str(input);
        assert!(!out.contains(leak), "裸 token 未脱敏：{input:?} -> {out:?}");
        assert!(out.contains("***"), "应有脱敏标记：{out:?}");
    }
    assert_eq!(r.redact_str("sk-abcdefghijklmnopqrstuvwxyz"), "***");
    assert_eq!(r.redact_str("AKIAIOSFODNN7EXAMPLE"), "***");
}

#[test]
fn redacts_bearer_token_shapes_like_codex() {
    let r = Redactor::default_v0();
    // 样例取自 codex sanitizer.rs 的 redacts_supported_bearer_tokens。
    let cases = [
        ("Bearer abcde+fghijklmnopqrstuvwxyz012345", "Bearer ***"),
        ("Bearer abcdefghijklmnop+secret_suffix", "Bearer ***"),
        ("Bearer sk-abcdefghijklmnopqrst+secret_suffix", "Bearer ***"),
        ("Bearer AKIAABCDEFGHIJKLMNOP/~secret_suffix", "Bearer ***"),
        (
            "Bearer AbcdefghijklMN09._~+/-==; echo done",
            "Bearer ***; echo done",
        ),
        (
            "authorization: bEaReR\tabcdefghijklmnop",
            "authorization: Bearer ***",
        ),
        ("Bearer   abcdefghijklmnop", "Bearer ***"),
    ];
    for (input, expected) in cases {
        assert_eq!(r.redact_str(input), expected, "输入：{input:?}");
    }
}

#[test]
fn avoids_shape_false_positives() {
    let r = Redactor::default_v0();
    // sk- 需 >=20 位；AKIA 需词边界 + 恰好 16 位；Bearer 需 >=16 位。
    let cases = [
        "sk-learn",
        "sk-short",
        "AKIAABCDEFGHIJ",
        "Bearer abcde",
        // codex avoids_bearer_false_positives 全部样例。
        "Bearer of good news",
        "Bearer abcdefghijklmno",
        "NotABearer abcdefghijklmnop",
        "Bearerabcdefghijklmnop",
        "Bearer\nabcdefghijklmnop",
        "Bearer\u{a0}abcdefghijklmnop",
        "Bearer abcdefghijklmno\u{212a}",
    ];
    for input in cases {
        assert_eq!(r.redact_str(input), input, "不得误伤：{input:?}");
    }
}

#[test]
fn shape_redaction_is_idempotent() {
    let r = Redactor::default_v0();
    let samples = [
        r#"curl -H "X-Api: sk-abcdefghijklmnopqrstuvwxyz""#,
        "aws_access_key_id = AKIAIOSFODNN7EXAMPLE",
        "authorization: bEaReR\tabcdefghijklmnop",
        "Bearer sk-abcdefghijklmnopqrst+secret_suffix",
    ];
    for input in samples {
        let once = r.redact_str(input);
        let twice = r.redact_str(&once);
        assert_eq!(once, twice, "二次脱敏不得改变结果：{input:?}");
    }
}
