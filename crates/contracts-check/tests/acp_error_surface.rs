//! ACP **wire 错误码面**（`agent-acp` 的 JSON-RPC 负数族）穷举断言。
//!
//! 这条断言治的是「ACP 实现了码、契约层零登记」的漂移：`agent-acp` 另有一套
//! **与网关信封层完全不同的**错误码命名空间——**i64 负数**族（标准族
//! `-32700/-32601/-32600/-32602` 与私有族 `-32000/-32001`），而契约层
//! (`ENVELOPE_CODES`) 只登记了**网关信封层**的 `u16` 码。既有
//! `tests/envelope_code_surface.rs` **结构上不可能**覆盖它：那张表扫的是
//! `agent-gateway/src/{rpc,server}.rs` 里「网关**能作为响应码发出**的 `u16`
//! 字面量」，ACP 的负数码是**另一个 crate 的常量**、既不在被扫文件里、也不是
//! `u16` 字面量。
//!
//! # 为什么这条不变量重要
//!
//! 两套命名空间**会同时出现在同一个响应体里**：`crates/agent-gateway/src/rpc.rs`
//! 的 `acp_forward()` 在「转发成功但对端回的是 ACP 错误」时，把对端的
//! `{"error":{…}}` 原样塞进信封的 `data` 字段，而信封**顶层 `code` 仍是 `0`**
//! （转发本身送达并拿到应答）。既有回归锚
//! `crates/agent-gateway/tests/acp_forward_whitelist.rs` 明文断言：
//!
//! ```text
//! resp["code"] == 0                        // 信封层 u16 族
//! resp["data"]["error"]["code"] == -32601   // ACP wire i64 负数族
//! ```
//!
//! ⇒ 壳侧（`crates/lengsuo-shell/src/gateway/rpc.rs`，判据 `envelope.code == 0`）
//! **只看顶层 `code`**，`data.error.code` 里的 ACP 负数码此前**没有任何一层对它
//! 做断言** ⇒ 本测试就是那层缺失的护栏。
//!
//! 做法照抄 `tests/envelope_code_surface.rs`：**只读文件、手写字符扫描、不新增
//! 依赖、不用正则库**。全量清单见 `docs/contracts/acp-host.md` §9。
use contracts_check::{ACP_PRIVATE_ERROR_CODES, ACP_STANDARD_ERROR_CODES, ENVELOPE_CODES};
use std::collections::{BTreeMap, BTreeSet};

/// 预期**承载** ACP wire 错误码常量的源文件（相对 `agent-acp/src/`）。
///
/// - `message.rs`：标准族 `JSONRPC_ERROR_*`（`:71/:73/:75/:77`）；
/// - `router.rs`：私有族 `ACP_ERROR_*`（`:40/:42`）；
/// - `host_router.rs`：私有族**别名** `ACP_ERROR_HOST_CONNECTION_CLOSED`
///   （`:278`，写成 `= ACP_ERROR_CONNECTION_CLOSED;` 而非字面量）。
///
/// ⛔ 扫描面**不**限于这三个文件：实现在**别的**文件里新加一个负数 `i64`
/// 常量时，扫描仍会扫到并要求登记；本表只保证「已知来源清单」与实际一致。
const EXPECTED_CODE_FILES: &[&str] = &["message.rs", "router.rs", "host_router.rs"];

/// 一行是不是注释：去掉缩进后以 `//` 开头。
///
/// ⛔ **必须排除注释行**：`message.rs:70-77` 与 `router.rs:38-42` 上面全是 `///`
/// 文档注释（「标准 JSON-RPC 错误码：方法不存在」等**散文**）——不排除就会把
/// **注释里的散文**当成实现面（进而把散文里的数字当成码收进扫描结果）。
///
/// 只认**行注释**是够的：被扫描的 `agent-acp/src/` 下**没有块注释**（`/*` 只出
/// 现在 `host_baseline.rs` / `methods.rs` / `tasks.rs` 的 `session/*` 路径写法里）。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 一个 `const <NAME>: i64 = …` 声明的**原始**右值。
#[derive(Debug, Clone, PartialEq, Eq)]
enum RawValue {
    /// 字面量赋值（`-32000`）。
    Literal(i64),
    /// 别名赋值（`= ACP_ERROR_CONNECTION_CLOSED;`）——指向**同 crate 内**的另一个常量。
    Alias(String),
}

/// 从**单行**里抽出 `const <NAME>: i64 = <字面量|别名>;` 声明；不是这个形态返回 `None`。
///
/// 口径（手写字符扫描，不用正则）：①行内找 `const ` 关键字；②常量名取到 `:`
/// 为止，必须**全为大写字母 / 数字 / 下划线**（排除 `let`、结构体字段等其它
/// `const` 字样）；③类型标注必须**恰好**是 `i64`（排除 `&str` / `u16`）；④`=`
/// 之后取右值：`可选 -` + 数字 ⇒ 字面量；大写标识符 ⇒ 别名；⑤右值必须以 `;`
/// 结束（排除 `= -32000 + 1` 这类表达式）。
fn parse_i64_const(line: &str) -> Option<(String, RawValue)> {
    let const_at = line.find("const ")?;
    let after_const = &line[const_at + "const ".len()..];
    let colon_at = after_const.find(':')?;
    let name = after_const[..colon_at].trim();
    if !is_const_name(name) {
        return None;
    }
    let eq_at = after_const[colon_at + 1..].find('=')?;
    let ty = after_const[colon_at + 1..colon_at + 1 + eq_at].trim();
    if ty != "i64" {
        return None;
    }
    let rhs = after_const[colon_at + 1 + eq_at + 1..].trim();
    let rhs = rhs.strip_suffix(';')?.trim();
    if let Some(digits) = rhs.strip_prefix('-') {
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let magnitude: i64 = digits.parse().ok()?;
        return Some((name.to_owned(), RawValue::Literal(-magnitude)));
    }
    if is_const_name(rhs) {
        return Some((name.to_owned(), RawValue::Alias(rhs.to_owned())));
    }
    None
}

/// 是不是「常量名」的形态：非空且全为大写字母 / 数字 / 下划线。
fn is_const_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// 读 `agent-acp/src/` 下全部 `.rs` 源文件，返回 `(文件名, 源码)` 列表（按文件名排序）。
///
/// 路径写法照抄 `tests/method_ledger.rs`（读 `agent-acp/src/methods.rs` 的先例）。
fn acp_src_files() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-acp")
        .join("src");
    let mut out: Vec<(String, String)> = Vec::new();
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("读不到 {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("读目录项失败").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("非 UTF-8")
            .to_owned();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
        out.push((name, source));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// 扫出给定源文件列表里所有 `const <NAME>: i64 = …` 声明（**原始**右值，未解析）。
fn scan_i64_const_decls_in(files: &[(String, String)]) -> BTreeMap<String, (String, RawValue)> {
    let mut out: BTreeMap<String, (String, RawValue)> = BTreeMap::new();
    for (file, source) in files {
        for line in source.lines() {
            if is_comment_line(line) {
                continue;
            }
            let Some((name, value)) = parse_i64_const(line) else {
                continue;
            };
            out.insert(name.clone(), (file.clone(), value));
        }
    }
    out
}

/// 扫出 `agent-acp/src/` 下所有 `const <NAME>: i64 = …` 声明（**原始**右值，未解析）。
fn scan_i64_const_decls() -> BTreeMap<String, (String, RawValue)> {
    scan_i64_const_decls_in(&acp_src_files())
}

/// 解析一个别名的值（带环检测）。
fn resolve_alias(
    name: &str,
    decls: &BTreeMap<String, (String, RawValue)>,
    visiting: &mut Vec<String>,
) -> Option<i64> {
    let (_, raw) = decls.get(name)?;
    match raw {
        RawValue::Literal(v) => Some(*v),
        RawValue::Alias(target) => {
            if visiting.iter().any(|s| s == name) {
                return None;
            }
            visiting.push(name.to_owned());
            let resolved = resolve_alias(target, decls, visiting);
            visiting.pop();
            resolved
        }
    }
}

/// 扫出全部 `i64` 常量的**解析后**码值（含别名的解析），返回 `(文件名, 常量名, 码值)`。
///
/// ⛔ 别名必须能解析：解析不了（指向未知常量 / 成环）⇒ **panic**——那是扫描器与
/// 源文件形态脱节了，必须让人看见，而不是静默少扫一圈。
fn scanned_acp_error_codes() -> Vec<(String, String, i64)> {
    let decls = scan_i64_const_decls();
    let mut out: Vec<(String, String, i64)> = Vec::new();
    for name in decls.keys() {
        let mut visiting = Vec::new();
        let value = resolve_alias(name, &decls, &mut visiting).unwrap_or_else(|| {
            panic!("常量 {name} 的值解析不出来（别名指向未知常量或成环）——扫描器与源文件形态脱节了")
        });
        out.push((decls[name].0.clone(), name.clone(), value));
    }
    out
}

/// 扫出的码值集合（**按码值去重**——别名的同值不重复计入）。
fn scanned_code_values() -> BTreeSet<i64> {
    scanned_acp_error_codes()
        .into_iter()
        .map(|(_, _, v)| v)
        .collect()
}

/// 扫出的码值**所在文件**集合。
fn scanned_code_files() -> BTreeSet<String> {
    scanned_acp_error_codes()
        .into_iter()
        .map(|(file, _, _)| file)
        .collect()
}

/// 扫描器**排除注释行**这条规则的自我证明。
///
/// ⛔ 若不排除注释，被注释掉的 `const` 声明（`/// pub const …: i64 = -99999;`
/// 这种「注释里藏着声明」的形态）就会被当成实现面 ⇒ 注释里的码值会被误收进
/// 扫描结果。本断言用**合成样本**把这条规则钉死。
///
/// ⚠️ 样本里的注释行是**完整可解析的声明**——否则这条自检就只是「碰巧绿」
/// （注释行本来也解析不出声明，去掉排除步骤照样绿），起不到守规则的作用。
#[test]
fn scanner_excludes_comment_lines() {
    // 样本：两条**完整可解析**的声明被注释掉（`///` 与 `//` 两种形态）+ 一条真的。
    let sample = "\
/// pub const JSONRPC_ERROR_COMMENTED_OUT: i64 = -99999;
/// 标准 JSON-RPC 错误码：方法不存在。
pub const JSONRPC_ERROR_METHOD_NOT_FOUND: i64 = -32601;
// pub const JSONRPC_ERROR_LINE_COMMENTED: i64 = -88888;
";
    let decls = scan_i64_const_decls_in(&[("sample.rs".to_owned(), sample.to_owned())]);
    let values: BTreeSet<i64> = decls
        .values()
        .map(|(_, raw)| match raw {
            RawValue::Literal(v) => *v,
            RawValue::Alias(_) => unreachable!("样本里没有别名"),
        })
        .collect();
    assert_eq!(
        values,
        BTreeSet::from([-32601]),
        "扫描器把注释里的声明当成了实现——注释排除规则失效"
    );
    assert!(
        !values.contains(&-99999) && !values.contains(&-88888),
        "注释行里的码值被扫进来了——注释排除规则失效"
    );
}

/// 契约登记的全部 ACP wire 错误码（标准族 ∪ 私有族）。
fn registered_acp_error_codes() -> BTreeSet<i64> {
    let mut out: BTreeSet<i64> = ACP_STANDARD_ERROR_CODES.iter().copied().collect();
    out.extend(ACP_PRIVATE_ERROR_CODES.iter().copied());
    out
}

/// 核心关系不变量：**实现源码里扫出的码 ⊆ 契约登记的码**。
///
/// 红了说明：`agent-acp` 发了新码，但 `contracts_check` 的
/// `ACP_STANDARD_ERROR_CODES` / `ACP_PRIVATE_ERROR_CODES` 没登记——先补登记
/// （并同步 `docs/contracts/acp-host.md` §9），再合并。
///
/// ⚠️ 刻意**只守关系、不锁数量/取值**：实现正当新增错误码时，把新码登记进对应
/// 常量后本断言自然变绿。任何「扫出恰好 N 个码」「集合等于某个定值」的断言
/// 都会在正当扩充时必红（上一张动作面卡就踩过这个坑）。
#[test]
fn implementation_acp_error_codes_are_subset_of_registered() {
    let registered = registered_acp_error_codes();
    let unregistered: Vec<(String, String, i64)> = scanned_acp_error_codes()
        .into_iter()
        .filter(|(_, _, v)| !registered.contains(v))
        .collect();
    assert!(
        unregistered.is_empty(),
        "实现发出但契约未登记的 ACP 错误码（差集）{unregistered:?}——\
         契约落后于实现，先补登记再合并"
    );
}

/// 反向关系不变量：**契约登记的每个码值都必须在实现里出现**。
///
/// 红了说明：契约登记了实现里已删 / 已改名的码 ⇒ 登记面腐化了，要把它撤下
/// （或把实现改回来）。
#[test]
fn registered_acp_error_codes_are_subset_of_implementation() {
    let scanned = scanned_code_values();
    let missing: Vec<i64> = registered_acp_error_codes()
        .difference(&scanned)
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "契约登记但实现里扫不到的 ACP 错误码（差集）{missing:?}——\
         登记面腐化了，撤下登记或把实现改回来"
    );
}

/// 防空扫描：扫描器本身坏了（路径写错 / 口径改坏）时，断言会「因为什么都没扫到」
/// 而假绿 ⇒ 单独守一条「扫得到东西，且两族各自都扫得到」。
#[test]
fn scanner_actually_finds_acp_error_codes() {
    let scanned = scanned_code_values();
    assert!(
        !scanned.is_empty(),
        "一个 ACP 错误码都没扫到——扫描口径或文件路径坏了，断言会假绿"
    );
    // 标准族在 `message.rs`、私有族在 `router.rs` ⇒ 两个族各自都应扫得到东西。
    let files = scanned_code_files();
    assert!(
        files.contains("message.rs"),
        "message.rs 扫不到标准族错误码（实扫文件：{files:?}）——扫描口径坏了"
    );
    assert!(
        files.contains("router.rs"),
        "router.rs 扫不到私有族错误码（实扫文件：{files:?}）——扫描口径坏了"
    );
}

/// 扫描面覆盖守则：**实际承载错误码的文件**必须都在 [`EXPECTED_CODE_FILES`] 里
/// ⇒ 文档化的来源清单不许悄悄过期。
///
/// 红了说明：实现把错误码常量挪到了（或新增到了）另一个文件 ⇒ 更新
/// `EXPECTED_CODE_FILES` 与 `docs/contracts/acp-host.md` §9.1 的来源行号。
#[test]
fn negative_i64_consts_live_in_the_expected_files() {
    let files = scanned_code_files();
    let expected: BTreeSet<&str> = EXPECTED_CODE_FILES.iter().copied().collect();
    let unexpected: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .filter(|f| !expected.contains(*f))
        .collect();
    assert!(
        unexpected.is_empty(),
        "承载 ACP 错误码常量的文件不在 EXPECTED_CODE_FILES 里：{unexpected:?}——\
         同步更新来源清单与文档"
    );
    for file in expected {
        assert!(
            files.contains(file),
            "EXPECTED_CODE_FILES 列了 {file}，但扫描里它没承载任何错误码常量——\
             来源清单与实现脱节了"
        );
    }
}

/// **两族不相交**：标准族（JSON-RPC 标准区）与私有族（`-3260x` 之外的自定义区）
/// 是**两层**语义（`crates/agent-acp/src/message.rs:61`：「标准族见
/// `JSONRPC_ERROR_*` 常量，**私有码原样保留**」）⇒ 混进一起即红。
#[test]
fn standard_and_private_acp_error_code_families_are_disjoint() {
    let standard: BTreeSet<i64> = ACP_STANDARD_ERROR_CODES.iter().copied().collect();
    let private: BTreeSet<i64> = ACP_PRIVATE_ERROR_CODES.iter().copied().collect();
    let overlap: Vec<i64> = standard.intersection(&private).copied().collect();
    assert!(
        overlap.is_empty(),
        "标准族与私有族相交于 {overlap:?}——两族是两层语义，不得混为一族"
    );
}

/// **族内无重复**：每个常量内部不得出现重复码值（重复登记会让「差集相等」的
/// 断言静默少跑一圈）。
#[test]
fn each_acp_error_code_family_has_no_duplicates() {
    for (name, family) in [
        ("ACP_STANDARD_ERROR_CODES", ACP_STANDARD_ERROR_CODES),
        ("ACP_PRIVATE_ERROR_CODES", ACP_PRIVATE_ERROR_CODES),
    ] {
        let unique: BTreeSet<i64> = family.iter().copied().collect();
        assert_eq!(
            unique.len(),
            family.len(),
            "{name} 内部有重复码值：{family:?}（去重后 {unique:?}）"
        );
    }
}

/// **两套命名空间不串味**：信封层码（`ENVELOPE_CODES`，非负 `u16`）与 ACP wire
/// 码（`i64` 负数）**必须不可能相等**。
///
/// 守的是这条不变量：ACP 的负数码**只**出现在 `data.error.code`，而**不是**信封
/// 顶层 `code`；两者混表会让客户端无法判断某个 `code` 属于哪一族。刻意**不锁
/// 取值**——只守「任一 ACP wire 码 ≠ 任一信封码」这条关系。
#[test]
fn envelope_codes_and_acp_wire_codes_never_collide() {
    let envelope: BTreeSet<i64> = ENVELOPE_CODES.iter().map(|c| i64::from(*c)).collect();
    for code in registered_acp_error_codes() {
        assert!(
            !envelope.contains(&code),
            "ACP wire 错误码 {code} 与信封层码面撞上了——两套命名空间不得串味"
        );
    }
    // 反向：信封层码**全是非负**，ACP wire 码**全是负数** ⇒ 两族值域不相交。
    for code in &envelope {
        assert!(
            *code >= 0,
            "信封层码 {code} 是负数——信封层码是非负 u16 族，不得混入负数"
        );
    }
    for code in registered_acp_error_codes() {
        assert!(
            code < 0,
            "ACP wire 错误码 {code} 不是负数——ACP wire 码族是 i64 负数族"
        );
    }
}
