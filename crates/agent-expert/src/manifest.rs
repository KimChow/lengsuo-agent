//! 专家包 manifest 数据结构（`plugin.json` 的逐字段映射）。
//!
//! 规格：调研 `harness-expert.md` §3.1「plugin.json 逐字段判定表」——本卡最
//! 重要交付物，字段名/类型/必填三列均照该表落，**不发明字段名**。
//!
//! 两条硬约定：
//! - **wire 格式原样保留**：JSON 键名与样本逐字一致（camelCase），
//!   唯一改名是插件目录 `.codebuddy-plugin/` → `.lengsuo-plugin/`（§3.3）。
//! - **可选字段不吐 null**：所有 `Option`/空集合都 `skip_serializing`，
//!   保证「反序列化 → 序列化 → 语义相等」对样本逐字成立。
//!
//! 规范外事实（§1.3）：`dependencies.connectors` **只解析不强制**。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

use crate::error::ExpertError;

/// 必填字段（§3.1「必填」列；`plugin` 例外见 `ExpertManifest` 文档）。
const REQUIRED_FIELDS: &[&str] = &[
    "name",
    "version",
    "description",
    "expertType",
    "agentName",
    "agents",
    "displayName",
    "profession",
    "displayDescription",
    "categoryId",
    "defaultInitPrompt",
    "tags",
    "quickPrompts",
];

/// 必填且 wire 类型为字符串的字段（其余必填字段类型由结构体自身决定）。
const REQUIRED_STRING_FIELDS: &[&str] = &["name", "version", "description", "agentName"];

/// 双语文本（`{en, zh}`）：样本里 `displayName`/`profession`/`tags` 等一律成对出现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalizedText {
    /// 中文。
    pub zh: String,
    /// 英文。
    pub en: String,
}

/// 作者（`{name, email}`，样本 pandaai 带 email）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    /// 作者名。
    pub name: String,
    /// 邮箱（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// 依赖声明（**规范外**，§1.3：只解析不强制；规范未给出其它键，不虚构）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependencies {
    /// 连接器名（如 `pandadata`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connectors: Vec<String>,
}

/// 团队成员角色（`role ∈ {lead, member}`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberRole {
    /// 主理人（在 `members` 中，`teamInfo.memberAgents` **不含**它）。
    Lead,
    /// 普通团员。
    Member,
}

/// 团队成员（`{id, name, profession, avatar, role}`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpertMember {
    /// 成员 id（= `agents/<id>.md` 的 stem）。
    pub id: String,
    /// 花名（双语）。
    pub name: LocalizedText,
    /// 职业头衔（双语）。
    pub profession: LocalizedText,
    /// 头像相对路径（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// 角色。
    pub role: MemberRole,
}

/// 团队信息（`{leadAgent, memberAgents}`；`memberAgents` **不含**主理人）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamInfo {
    /// 主理人 agent 名（= `agentName`）。
    pub lead_agent: String,
    /// 团员 agent 名列表（不含主理人）。
    #[serde(default)]
    pub member_agents: Vec<String>,
}

/// 专家类型（wire 值 `agent` / `team`；§3.1 与样本一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpertType {
    /// 单专家（样本 `senior-developer`）。
    Agent,
    /// 专家团（样本 `pandaai-team`）。
    Team,
}

/// 12 个分类（§5，**原样照抄**，`NN-Name` 是跨端契约）。
macro_rules! categories {
    ($(($variant:ident, $wire:literal, $zh:literal)),* $(,)?) => {
        /// 专家分类枚举（12 个）。
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum CategoryId {
            $($variant),*
        }

        impl CategoryId {
            /// 全部 12 个分类（按编号升序）。
            pub const ALL: [CategoryId; 12] = [$(CategoryId::$variant),*];

            /// wire 值（`NN-Name`）。
            pub fn as_str(&self) -> &'static str {
                match self { $(CategoryId::$variant => $wire),* }
            }

            /// 中文标签。
            pub fn zh_label(&self) -> &'static str {
                match self { $(CategoryId::$variant => $zh),* }
            }
        }

        impl std::str::FromStr for CategoryId {
            type Err = ExpertError;

            /// 由 wire 值解析；未知分类报 `UnknownCategory`。
            fn from_str(value: &str) -> Result<Self, ExpertError> {
                match value {
                    $($wire => Ok(CategoryId::$variant),)*
                    other => Err(ExpertError::UnknownCategory { value: other.to_string() }),
                }
            }
        }

        impl Serialize for CategoryId {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for CategoryId {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                CategoryId::from_str(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

categories![
    (ProductDesign, "01-ProductDesign", "产品设计"),
    (Engineering, "02-Engineering", "技术工程"),
    (GameSpatial, "03-GameSpatial", "游戏空间"),
    (DataAI, "04-DataAI", "数据智能"),
    (MarketingGrowth, "05-MarketingGrowth", "营销增长"),
    (ContentCreative, "06-ContentCreative", "内容创作"),
    (SalesCommerce, "07-SalesCommerce", "销售商务"),
    (FinanceInvestment, "08-FinanceInvestment", "金融投资"),
    (OperationsHR, "09-OperationsHR", "运营人力"),
    (ProjectQuality, "10-ProjectQuality", "项目质量"),
    (SecurityCompliance, "11-SecurityCompliance", "法务安全"),
    (IndustryConsultant, "12-IndustryConsultant", "行业顾问"),
];

/// 主理人 / 团员两套人格提示（§8.2）。
///
/// **派生数据**：由 `agents[]` + `teamInfo.memberAgents` 推出，不参与序列化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPair {
    /// 主理人的人格文件路径。
    pub lead: Option<String>,
    /// 团员的人格文件路径（顺序 = `teamInfo.memberAgents`）。
    #[serde(default)]
    pub members: Vec<String>,
}

/// 专家包 manifest：`.lengsuo-plugin/plugin.json` 的结构化映射。
///
/// 字段与 §3.1 判定表一一对应；Rust 侧用 snake_case，wire 侧仍是样本里的
/// camelCase 键名（`#[serde(rename_all = "camelCase")]`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertManifest {
    /// 唯一键 + 技能命名空间前缀（§3.1 必填；kebab-case 校验属卡 2）。
    pub name: String,
    /// 语义化版本（必填）。
    pub version: String,
    /// 英文一句话描述（必填；与 `displayDescription` 分工）。
    pub description: String,
    /// 作者（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Author>,
    /// 主页（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// 许可证（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// 检索关键词（可选）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// 专家类型（`agent` / `team`）。
    pub expert_type: ExpertType,
    /// 主 Agent 名 = `agents/<agentName>.md` 的 stem（禁通用名）。
    pub agent_name: String,
    /// 团队信息（`team` 型必填，`agent` 型无）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_info: Option<TeamInfo>,
    /// 人格文件路径数组（每项 `./agents/*.md`）。
    pub agents: Vec<String>,
    /// 技能包路径数组（每项 `./skills/<name>`，可选）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// 展示名（花名）。
    pub display_name: LocalizedText,
    /// 职业头衔。
    pub profession: LocalizedText,
    /// 展示描述（中文 40–50 字，长度校验属卡 2）。
    pub display_description: LocalizedText,
    /// 头像相对路径（可选；本仓不做头像生成）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// 分类（12 个之一）。
    pub category_id: CategoryId,
    /// 默认开场提示（须与 `quick_prompts[0]` 一致）。
    pub default_init_prompt: LocalizedText,
    /// 标签（固定 3 个）。
    pub tags: Vec<LocalizedText>,
    /// 快捷提示（固定 3 个）。
    pub quick_prompts: Vec<LocalizedText>,
    /// 插件标识（须等于 `name`）。
    ///
    /// §3.1 列为必填，但实测单专家样本 `senior-developer` **无此键**，
    /// 故结构层放宽为可选；「若声明则须等于 name」由卡 2 判定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// 成员列表（`team` 型含主理人，`agent` 型为空）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<ExpertMember>,
    /// 依赖声明（**规范外**：只解析不强制，§1.3）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Dependencies>,
    /// 人格文件路径（**派生**：由 `agents` + `agent_name` 推出）。
    ///
    /// `serde(skip)`：不参与 wire 格式——样本 `plugin.json` 没有这个键，
    /// 派生值若被写回会破坏「反序列化 → 序列化 → 语义相等」。
    #[serde(skip)]
    pub persona: Option<String>,
    /// 主理人 / 团员两套人格提示（**派生**，§8.2；同样不参与 wire 格式）。
    #[serde(skip)]
    pub prompt_pair: Option<PromptPair>,
}

impl ExpertManifest {
    /// 解析 `plugin.json` 文本（本 crate 的**规范入口**）。
    ///
    /// 派生字段（`persona` / `prompt_pair`）在此填充；直接用
    /// `serde_json::from_str` 解析则拿不到它们。
    pub fn from_json_str(text: &str) -> Result<Self, ExpertError> {
        let value: Value = serde_json::from_str(text).map_err(|e| ExpertError::ManifestParse {
            reason: e.to_string(),
        })?;
        if !value.is_object() {
            return Err(ExpertError::ManifestParse {
                reason: "manifest 根必须是 JSON 对象".to_string(),
            });
        }
        for field in REQUIRED_FIELDS {
            if value.get(*field).is_none() {
                return Err(ExpertError::MissingField {
                    field: (*field).to_string(),
                });
            }
        }
        for field in REQUIRED_STRING_FIELDS {
            let raw = &value[*field];
            if !raw.is_string() {
                return Err(ExpertError::TypeMismatch {
                    field: (*field).to_string(),
                    expected: "string",
                    actual: json_type_name(raw),
                });
            }
        }
        // 未知分类单独报 UNKNOWN_CATEGORY（不是笼统的解析失败）。
        let raw_category = value.get("categoryId").and_then(Value::as_str).ok_or({
            ExpertError::TypeMismatch {
                field: "categoryId".to_string(),
                expected: "string",
                actual: json_type_name(value.get("categoryId").unwrap_or(&Value::Null)),
            }
        })?;
        CategoryId::from_str(raw_category)?;

        let mut manifest: Self =
            serde_json::from_value(value).map_err(|e| ExpertError::ManifestParse {
                reason: e.to_string(),
            })?;
        manifest.persona = manifest.derive_persona();
        manifest.prompt_pair = manifest.derive_prompt_pair();
        Ok(manifest)
    }

    /// 序列化为 JSON 文本（键序按结构体声明；语义与样本等价）。
    pub fn to_json_string(&self) -> Result<String, ExpertError> {
        serde_json::to_string(self).map_err(|e| ExpertError::ManifestParse {
            reason: e.to_string(),
        })
    }

    /// 是否专家团。
    pub fn is_team(&self) -> bool {
        self.expert_type == ExpertType::Team
    }

    /// 主理人成员（`team` 型恰一个 `role == lead`）。
    pub fn lead_member(&self) -> Option<&ExpertMember> {
        self.members.iter().find(|m| m.role == MemberRole::Lead)
    }

    /// 人格文件路径：`agents` 中 stem == `agent_name` 的那一项。
    pub fn derive_persona(&self) -> Option<String> {
        self.agents
            .iter()
            .find(|p| file_stem(p).as_deref() == Some(self.agent_name.as_str()))
            .cloned()
    }

    /// 人格文件路径（只读访问器）。
    pub fn persona(&self) -> Option<&str> {
        self.persona.as_deref()
    }

    /// 主理人 / 团员两套人格提示（只读访问器）。
    pub fn prompt_pair(&self) -> Option<&PromptPair> {
        self.prompt_pair.as_ref()
    }

    /// 两套人格提示（`agent` 型无 ⇒ `None`；§8.2）。
    pub fn derive_prompt_pair(&self) -> Option<PromptPair> {
        if !self.is_team() {
            return None;
        }
        let team = self.team_info.as_ref()?;
        let lead = self.derive_persona();
        let members = team
            .member_agents
            .iter()
            .filter_map(|id| {
                self.agents
                    .iter()
                    .find(|p| file_stem(p).as_deref() == Some(id.as_str()))
                    .cloned()
            })
            .collect();
        Some(PromptPair { lead, members })
    }
}

/// 路径末段的文件名（去目录、去 `.md` 后缀）。
fn file_stem(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let stem = name.strip_suffix(".md").unwrap_or(name);
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

/// JSON 值的类型名（错误信息用）。
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
