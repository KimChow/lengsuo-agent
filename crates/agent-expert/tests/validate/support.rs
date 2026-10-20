//! 专家包校验器测试的共享夹具。
//!
//! PNG 构造（头像尺寸校验用）、真实样本成员表、包目录构造、断言助手。
//! 由 `main.rs` 统一 re-export，各主题文件 `use super::support::*;` 取用。
pub use agent_expert::{
    validate::item, ExpertManifest, ExpertType, RuleSet, Severity, ValidateError, Validator,
};
pub use std::collections::BTreeMap;
pub use std::fs;
pub use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

pub const SENIOR_PLUGIN: &str = include_str!("../fixtures/senior-developer.plugin.json");
pub const PANDAI_PLUGIN: &str = include_str!("../fixtures/pandaai-team.plugin.json");

/// 生成一张**真 PNG**（`IHDR` 宽高 + `IDAT` 全白像素 + `IEND`）。
///
/// 头像尺寸负例需要真图片头（校验器读 `IHDR` 取宽高），故在测试内现造——
/// 不落二进制夹具文件（`tests/fixtures/` 只放两份 manifest 副本）。
pub fn make_png(width: u32, height: u32) -> Vec<u8> {
    /// PNG chunk：`长度 + 类型 + 数据 + CRC32`。
    pub fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    /// CRC-32（IEEE 802.3，多项式 0xEDB88320）——PNG chunk 校验用。
    pub fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= *byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// zlib「无压缩」deflate 块（stored）——避免引入压缩依赖。
    pub fn zlib_stored(raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut offset = 0usize;
        while offset < raw.len() {
            let take = usize::min(65535, raw.len() - offset);
            let last = offset + take >= raw.len();
            out.push(u8::from(last));
            out.extend_from_slice(&(take as u16).to_le_bytes());
            out.extend_from_slice(&(!(take as u16)).to_le_bytes());
            out.extend_from_slice(&raw[offset..offset + take]);
            offset += take;
        }
        let (mut a, mut b) = (1u32, 0u32);
        for byte in raw {
            a = (a + *byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        out.extend_from_slice(&((b << 16) | a).to_be_bytes());
        out
    }

    // 每行 = 1 字节 filter type(0) + width 字节灰度值(255)；先造一行再按行拼，
    // 避免在循环里 push 同一常量（clippy `same_item_push`）。
    let mut row = Vec::with_capacity(1 + width as usize);
    row.push(0u8); // filter type 0
    row.resize(1 + width as usize, 255); // 8bit grayscale
    let mut raw = Vec::with_capacity((height as usize) * row.len());
    for _ in 0..height {
        raw.extend_from_slice(&row);
    }

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8bit grayscale
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &zlib_stored(&raw)));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

/// 512×512 的真 PNG（头像正例尺寸）。
pub fn png_512() -> Vec<u8> {
    make_png(512, 512)
}

/// 256×256 的真 PNG（头像尺寸负例）。
pub fn png_256() -> Vec<u8> {
    make_png(256, 256)
}

/// 单专家样本的人格 MD（照抄样本 frontmatter + 正文骨架）。
pub const SENIOR_AGENT_MD: &str = "---\n\
name: senior-developer\n\
description: Senior full-stack developer with 10+ years experience.\n\
---\n\
\n\
# 高级开发工程师（Senior Developer）\n\
\n\
你是一位拥有 10 年以上实战经验的高级全栈开发工程师。\n";

/// 团队型主理人人格 MD（照抄样本 frontmatter）。
pub const PANDAI_LEAD_MD: &str = "---\n\
name: pd-ai-quant-research-team\n\
description: \"Orchestrates five isolated quantitative research specialists.\"\n\
displayName:\n\
  en: \"研报复现因子挖掘回测审计\"\n\
  zh: \"研报复现因子挖掘回测审计\"\n\
profession:\n\
  en: \"PD-AI量化研究专家团\"\n\
  zh: \"PD-AI量化研究专家团\"\n\
maxTurns: 200\n\
skills:\n\
  - ai-quant-team-runtime\n\
---\n\
\n\
## 执行前置：PandaData MCP 硬闸门\n";

/// 团队型团员人格 MD（照抄样本 frontmatter）。
pub fn pandaai_member_md(id: &str) -> String {
    format!(
        "---\n\
name: {id}\n\
description: \"独立完成自己那一段任务，只处理任务包中自己负责的部分。\"\n\
maxTurns: 200\n\
skills:\n\
  - ai-quant-team-runtime\n\
---\n\
\n\
## 职责\n\
\n\
你是专家团成员，不是主理人。\n"
    )
}

/// 单专家样本的三个技能目录名。
pub const SENIOR_SKILLS: &[&str] = &["browser-use", "frontend-dev", "fullstack-dev"];

/// 团队型样本的六个技能目录名。
pub const PANDAI_SKILLS: &[&str] = &[
    "ai-quant-team-runtime",
    "skill-report-replication",
    "skill-factor-mining-pandaai",
    "skill-pandaai-factor-online",
    "skill-backtest-overfit",
    "skill-strategy-tearsheet-report",
];

/// 团队型样本的五个团员 id。
pub const PANDAI_MEMBERS: &[&str] = &[
    "source-replication-researcher",
    "factor-engineer",
    "pandaai-experimenter",
    "overfit-auditor",
    "performance-reporter",
];

/// 临时目录：测试结束自动清理。
pub struct TempDir(PathBuf);

impl TempDir {
    /// 在系统临时目录下建一个唯一目录。
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("agent-expert-validate-{tag}-{nanos}"));
        fs::create_dir_all(&path).expect("临时目录应可创建");
        Self(path)
    }

    /// 目录路径。
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 在 `dir` 下搭一个**可校验通过**的专家包。
///
/// 单专家（`senior-developer`）与团队型（`pandaai-team`）各一版；目录名
/// 与 `plugin.json.name` 一致（`pandaai-team` 的 `name` 是
/// `pandaai-ai-quant-research-team` ⇒ 目录名照样本落 `pandaai-team`，
/// 用于复现「目录名 ≠ name」的 #3b 偏离）。
pub fn build_package(dir: &Path, plugin_json: &str, team: bool) -> ExpertManifest {
    let plugin_dir = dir.join(".lengsuo-plugin");
    fs::create_dir_all(&plugin_dir).expect("插件目录应可创建");
    fs::write(plugin_dir.join("plugin.json"), plugin_json).expect("manifest 应可写");

    let manifest = ExpertManifest::from_json_str(plugin_json).expect("样本应可解析");

    // 人格文件：照 manifest 的 `agents[]` 逐项建（内容照抄样本 frontmatter）。
    // 路径逃逸的声明项**不建文件**（越界的文件本就不该在包内）。
    let agents_dir = dir.join("agents");
    fs::create_dir_all(&agents_dir).expect("agents 目录应可创建");
    for declared in &manifest.agents {
        if declared.contains("..") {
            continue;
        }
        let stem = declared
            .trim_start_matches("./agents/")
            .trim_end_matches(".md");
        let body = if team {
            if stem == manifest.agent_name {
                PANDAI_LEAD_MD.to_string()
            } else {
                pandaai_member_md(stem)
            }
        } else {
            SENIOR_AGENT_MD.to_string()
        };
        fs::write(agents_dir.join(format!("{stem}.md")), body).expect("人格文件应可写");
    }

    // 技能目录：每个建 `SKILL.md`（照样本结构）。
    let skills_dir = dir.join("skills");
    fs::create_dir_all(&skills_dir).expect("skills 目录应可创建");
    for declared in &manifest.skills {
        let stem = declared.trim_start_matches("./skills/");
        let skill_dir = skills_dir.join(stem);
        fs::create_dir_all(&skill_dir).expect("技能目录应可创建");
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {stem}\ndescription: 技能说明\n---\n\n正文\n"),
        )
        .expect("SKILL.md 应可写");
    }

    // 头像：照 manifest 的 `avatar` 建 512×512 PNG。
    if let Some(avatar) = manifest.avatar.as_ref() {
        let avatar_path = dir.join(avatar);
        if let Some(parent) = avatar_path.parent() {
            fs::create_dir_all(parent).expect("avatars 目录应可创建");
        }
        fs::write(&avatar_path, png_512()).expect("头像应可写");
    }

    manifest
}

/// 搭建单专家包（`senior-developer`）。
pub fn build_single(dir: &Path) -> ExpertManifest {
    build_package(dir, SENIOR_PLUGIN, false)
}

/// 搭建团队型包（`pandaai-team`）。
pub fn build_team(dir: &Path) -> ExpertManifest {
    build_package(dir, PANDAI_PLUGIN, true)
}

/// 改写 manifest JSON 的某个顶层键（测试用；不重排其它键）。
pub fn patch(plugin_json: &str, key: &str, value: serde_json::Value) -> String {
    let mut value_obj: serde_json::Value =
        serde_json::from_str(plugin_json).expect("样本应是合法 JSON");
    value_obj[key] = value;
    serde_json::to_string_pretty(&value_obj).expect("应可序列化")
}

/// 把改写后的 manifest 写回包目录并重新解析。
pub fn rewrite_manifest(dir: &Path, patched: &str) -> ExpertManifest {
    fs::write(dir.join(".lengsuo-plugin/plugin.json"), patched).expect("应可写");
    ExpertManifest::from_json_str(patched).expect("应可解析")
}

/// 读取报告里某编号的发现。
pub fn finding_for(report: &agent_expert::ValidationReport, number: u8) -> Option<&ValidateError> {
    report.findings.iter().find(|f| f.item() == number)
}

/// 断言某编号的发现存在，且错误码与级别符合预期。
pub fn assert_finding(
    report: &agent_expert::ValidationReport,
    number: u8,
    code: &str,
    severity: Severity,
) {
    let finding = finding_for(report, number).unwrap_or_else(|| {
        panic!(
            "应报 §17 #{number} 的发现，实际报告：{:#?}",
            report.findings
        )
    });
    assert_eq!(finding.code(), code, "§17 #{number} 错误码不符");
    assert_eq!(finding.severity(), severity, "§17 #{number} 级别不符");
}
