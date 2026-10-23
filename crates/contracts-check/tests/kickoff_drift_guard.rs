//! 防漂移守卫：`AGENT-KICKOFF.md` §2 冻结的两张枚举值必须与代码常量**同源**。
//!
//! 文档写法可以保持人读的散文形态，但**枚举值本身**不许手抄——本文件从冻结文档里
//! 抽出反引号包裹的枚举 token，与单一来源常量做**双向**比对：
//!
//! - 事件：`contracts_check::EVENT_KINDS`（`crates/contracts-check/src/events.rs`）；
//! - 动作：`agent_gateway::V0_ACTIONS`（`crates/agent-gateway/src/actions.rs`）。
//!
//! 另有文本反向守卫，挡住「隔离键随 payload」这类与实现相反的措辞回流。
//!
//! `AGENT-KICKOFF.md` 在 crate 之外，故用 `CARGO_MANIFEST_DIR` 拼路径读文件，
//! 不能用 `include_str!`。

use std::path::Path;

fn kickoff_doc() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../AGENT-KICKOFF.md");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到冻结文档 {}: {e}", path.display()))
}

/// 找到第一条满足条件的行；找不到即 panic（文档结构被改坏时不许静默跳过）。
fn find_line<'a>(doc: &'a str, pred: impl Fn(&str) -> bool, what: &str) -> &'a str {
    doc.lines().find(|line| pred(line)).unwrap_or_else(|| {
        panic!("AGENT-KICKOFF.md 里找不到{what}——文档结构变了，先修守卫再改文档")
    })
}

/// 抽出一条行内所有反引号包裹的片段（`a `b` c `d`` → ["b", "d"]）。
fn backtick_spans(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

/// snake_case 单词：小写字母/数字/下划线，不以数字开头。
fn is_snake_word(word: &str) -> bool {
    let mut chars = word.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// 把一段反引号片段按非标识符字符切词；空片段丢弃。
fn words_of(span: &str) -> Vec<&str> {
    span.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// 抽取事件行里的枚举 token。
///
/// 只认「整段片段全部由 snake_case 单词组成」的反引号片段：枚举句
/// `` `thinking / text / ...` `` 命中；`GET /events`（含大写方法名）、
/// 源码路径 `.../events.rs::EVENT_KINDS`（含大写 / 点号切出的 path 词）这类
/// 路由/引用片段整段跳过。允许整段跳过而非逐词判断，是为了避免把 `events`
/// 这类路由组成部分误当成事件 kind。
fn event_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for span in backtick_spans(line) {
        let words = words_of(span);
        if words.is_empty() {
            continue;
        }
        if words.iter().all(|w| is_snake_word(w)) {
            out.extend(words.into_iter().map(str::to_owned));
        }
    }
    out
}

/// 抽取动作行里的 `域:动作` token，兼容紧凑写法（`session:list/create` 展开为两项）。
fn action_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for span in backtick_spans(line) {
        for piece in span.split(|c: char| c.is_whitespace() || "、，,".contains(c)) {
            let Some((domain, rest)) = piece.trim().split_once(':') else {
                continue;
            };
            if !is_snake_word(domain) {
                continue;
            }
            for action in rest.split('/') {
                if is_snake_word(action) {
                    out.push(format!("{domain}:{action}"));
                }
            }
        }
    }
    out
}

fn unique_sorted(mut tokens: Vec<String>) -> Vec<String> {
    tokens.sort();
    tokens.dedup();
    tokens
}

/// §2 事件行：文档 ⊆ `EVENT_KINDS` 且 `EVENT_KINDS` ⊆ 文档（双向，防漏写/私增）。
#[test]
fn kickoff_event_list_matches_event_kinds() {
    let doc = kickoff_doc();
    let line = find_line(
        &doc,
        |l| l.contains("GET /events") && l.contains("事件名"),
        "§2 事件行（同时含 `GET /events` 与「事件名」）",
    );

    let documented = unique_sorted(event_tokens(line));
    let frozen: Vec<String> = contracts_check::EVENT_KINDS
        .iter()
        .map(|k| (*k).to_owned())
        .collect();

    for kind in &documented {
        assert!(
            frozen.contains(kind),
            "§2 事件行写了 `{kind}`，但不在 EVENT_KINDS 里（新 kind 须先登记 \
             crates/contracts-check/src/events.rs::EVENT_KINDS 再改文档）。行文：{line}"
        );
    }
    for kind in &frozen {
        assert!(
            documented.contains(kind),
            "EVENT_KINDS 里的 `{kind}` 没出现在 §2 事件行（文档漏写）。行文：{line}"
        );
    }
}

/// §2 动作行：文档 ⊆ `V0_ACTIONS` 且 `V0_ACTIONS` ⊆ 文档（双向）。
#[test]
fn kickoff_action_list_matches_v0_actions() {
    let doc = kickoff_doc();
    let line = find_line(
        &doc,
        |l| l.contains("v0 动作面"),
        "§2 动作行（含「v0 动作面」）",
    );

    let documented = unique_sorted(action_tokens(line));
    let frozen: Vec<String> = agent_gateway::V0_ACTIONS
        .iter()
        .map(|a| (*a).to_owned())
        .collect();

    for action in &documented {
        assert!(
            frozen.contains(action),
            "§2 动作行写了 `{action}`，但不在 V0_ACTIONS 里（新动作须先按「只增不改」\
             补登记进 crates/agent-gateway/src/actions.rs::V0_ACTIONS 与台账）。行文：{line}"
        );
    }
    for action in &frozen {
        assert!(
            documented.contains(action),
            "V0_ACTIONS 里的 `{action}` 没出现在 §2 动作行（文档漏写）。行文：{line}"
        );
    }
}

/// 文本反向守卫：实现是**宿主绑定身份**（`rpc.rs::run` 的 `session:create` 对 payload
/// 携带 `isolation`/`envBinding`/`env_binding` 回 403 `server-bound identity`），
/// 文档不许再出现「隔离键随 payload」这类相反措辞。
#[test]
fn kickoff_does_not_claim_isolation_keys_ride_the_payload() {
    let doc = kickoff_doc();
    for forbidden in ["隔离键随 payload", "隔离键随payload"] {
        assert!(
            !doc.contains(forbidden),
            "AGENT-KICKOFF.md 出现与实现相反的措辞「{forbidden}」。应改为：身份/租户/环境由\
             宿主绑定（GatewayConfig.isolation）；payload 携带 isolation/envBinding 一律 403；\
             读/写类动作按 own() 做归属校验（跨隔离 → 403）"
        );
    }
    assert!(
        doc.contains("宿主绑定"),
        "鉴权行应写明「身份/租户/环境由宿主绑定」——这句被删了同样算漂移"
    );
}

/// 扫描器自检：规则在合成样本上的行为必须可预期，防止抽取逻辑坏掉后「空扫描恒绿」。
#[test]
fn token_scanners_behave_on_synthetic_samples() {
    // 路由片段（含大写方法名）整段跳过；枚举片段与单 token 片段被抽出。
    let event_line = "- 事件：`GET /events`；事件名 `alpha / beta_2 / gamma`；\
                      来源 `crates/foo/src/events.rs::EVENT_KINDS`";
    assert_eq!(
        event_tokens(event_line),
        vec!["alpha", "beta_2", "gamma"],
        "事件 token 抽取规则变了或失效"
    );

    // 紧凑与展开两种写法都要认；无冒号的引用片段跳过。
    let action_line = "- v0 动作面：`svc:one/two`、`svc:three`；台账 `docs/x.md` §1";
    assert_eq!(
        action_tokens(action_line),
        vec!["svc:one", "svc:two", "svc:three"],
        "动作 token 抽取规则变了或失效"
    );

    // 空输入必须抽不出东西——否则真实断言会变成空对空假绿。
    assert!(event_tokens("").is_empty());
    assert!(action_tokens("").is_empty());
}
