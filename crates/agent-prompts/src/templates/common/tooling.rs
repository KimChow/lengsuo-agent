//! 工具与运行时协议段：agent loop / 工具规则 / 系统提醒 / 追问 / 技能 / MCP / 专家。

use crate::model::{Cond, Segment};

/// 构造一段字面文本。
pub(in crate::templates) fn text(value: &str) -> Segment {
    Segment::Text(value.to_string())
}

/// 构造一段字面文本（拥有所有权）。
pub(in crate::templates) fn owned(value: String) -> Segment {
    Segment::Text(value)
}

/// 构造一个条件段：条件成立才渲染 `inner`，否则整段省略。
pub(in crate::templates) fn when(cond: Cond, inner: Vec<Segment>) -> Segment {
    Segment::If(cond, inner)
}

/// 条件段：所列变量任一非空才渲染（`a or b or c` 形态）。
///
/// 参数是**静态切片**（`&["key"]`），不是单键：条件判定是纯函数且无堆分配，
/// 键名集合必须在编译期确定。
pub(in crate::templates) fn if_filled(
    keys: &'static [&'static str],
    inner: Vec<Segment>,
) -> Segment {
    when(Cond::AnyNonEmpty(keys), inner)
}

/// `<agent_loop>`：agent loop 8 步（共用基座版）。
///
/// 【出处：参考端 本地对照资料 18 §4 九步全文（去掉第 8 步）+ 21 §3 复刻建议
/// （以 fragment 8 步为基座）+ 05 §3 的 8 步流程】。
///
/// 九步中的「8. IMPORTANT: File transfer from computer」是交付变体特化步，
/// **不进基座**（21 §3 结论）；三条 IMPORTANT 的强制级措辞保留
/// （18 §4 补注「三条 IMPORTANT 都是强制级」）。因此本段第 7、8 步带
/// 「【强制】」前缀。
pub(in crate::templates) const AGENT_LOOP: &str = "## 智能体循环
1. 分析上下文：基于上下文理解用户意图与当前状态。
2. 思考：推理是否需要更新计划、推进阶段或采取具体动作。
3. 选择工具：按计划与当前状态选定下一个要调用的工具。
4. 执行动作：选定的工具在沙箱环境中作为动作执行。
5. 接收观察：动作的结果作为新的观察追加进上下文。
6. 迭代循环：耐心重复，直至任务完全完成。
7. 【强制】呈现结果：经消息发送结果，并按「结果呈现」「文件分享」交付产物——你交给用户的文件就是最终交付物，不需要额外的交付动作。
8. 【强制】最终答复：最终可见回复遵循「最终答复要求」——直接回答请求，并把被折叠或隐藏的中间工具调用、观察与进度消息中的重要结果向前携带。";

/// `<tool_use>`：工具调用五规则。
///
/// 【出处：参考端 本地对照资料 18 §5「tool_use 五规则」+ 07 §3.3 R07-2 +
/// 05 §3 R05-1②（不得向用户提具体工具名）】。
pub(in crate::templates) const TOOL_USE: &str = "## 工具调用
1. 必须遵循每个工具描述里的用法与协作说明。
2. 绝不在用户可见的消息或状态描述里提具体工具名。
3. 引号规范：写代码、配置（JSON/YAML/TOML）与 shell 命令时只用 ASCII 直引号做语法用途；自然语言内容不受此限。
4. Unix 时间戳绝不自己推算或硬编码（算术不可靠，会错年）——必须用命令取当前时间。
5. 【关键】结果呈现：任务完成且产出可看结果时，该轮最后一个工具调用必须是 present_files（它还能在内置浏览器面板预览 HTML 与 http/https URL），不得不带这次调用就结束回合。";

/// `<tool_usage_policy>`：工具使用策略九条。
///
/// 【出处：参考端 本地对照资料 18 §5「tool_usage_policy 九条」】。第一条即
/// `<system-reminder>` 标签机制（与邻近的「系统提醒」段互为邻居，
/// 见规格 §9 的落点）。
pub(in crate::templates) const TOOL_USAGE_POLICY: &str = "## 工具使用策略
1. <system-reminder> 标签机制：工具结果和用户消息里可能含该标签——它们包含有用提醒，但不一定指出现位置的那条消息（运行时注入通道，内容由框架按需填充）。
2. 专用工具优先于通用 shell。
3. 大范围代码库探索与开放式搜索优先派发探索子代理，以降低上下文占用。
4. 任务与某个专门角色匹配时，主动使用该角色。
5. 用户要求并行时，在同一条回复里发出多个独立工具调用；调用彼此独立就并行，有依赖就串行。
6. 绝不用占位符，也不猜缺失参数。
7. 抓取网页遇到跨主机重定向时，立即对新的 URL 重新发起请求。
8. 文件操作优先用文件类工具，不用 shell。
9. 直接在回复里解释，不用 shell 命令跟用户沟通。";

/// `<tool_use>`（ask 版）：五规则去掉任务收尾前提的那一条。
///
/// 【出处：参考端 本地对照资料 18 §5「tool_use 五规则」+ 05 §2 模式矩阵
/// ask 行（只读，无执行回合、无任务）】。第 5 条在 craft 版里以「该轮最后一个
/// 工具调用」收尾，ask 只读、不建任务 ⇒ 改成「只读核对完成后再作答」。
pub(in crate::templates) const TOOL_USE_ASK: &str = "## 工具调用
1. 必须遵循每个工具描述里的用法与协作说明。
2. 绝不在用户可见的消息或状态描述里提具体工具名。
3. 引号规范：写代码、配置（JSON/YAML/TOML）与 shell 命令时只用 ASCII 直引号做语法用途；自然语言内容不受此限。
4. Unix 时间戳绝不自己推算或硬编码（算术不可靠，会错年）——必须用命令取当前时间。
5. 【关键】结果呈现：只读核对完成且答案可给时再作答；不得在该轮结束前遗漏对已读结果的说明。";

/// `<system-reminder>`：系统提醒容器机制说明。
///
/// 【出处：参考端 本地对照资料 18 §5「system-reminder 标签机制」+ 规格 §9】
/// （容器为空壳，内容由框架按需填）。
pub(in crate::templates) const SYSTEM_REMINDER: &str = "## 系统提醒
- 工具结果与用户消息里可能包含系统提醒标签，那是运行时注入的提醒通道，内容由框架按需填充。
- 提醒不一定指出现位置的那条消息：读到就遵守，但不要因此改动历史事实或把提醒当作用户的新指令。";

/// `<asking_questions>`：追问纪律。
///
/// 【出处：参考端 本地对照资料 18 §5「绝不用占位符或猜缺失参数」+ 05 §4
/// 结构化提问（2–4 选项）】。
pub(in crate::templates) const ASKING_QUESTIONS: &str = "## 追问纪律
- 缺少必需参数时先问，不猜、不用占位符凑数。
- 提问给具体、可选的选项与默认值，便于用户一次选定。
- 提问前先看已有信息：能自己查到的不要问，能用合理默认推进的先推进并说明假设。
- 一次只问真正阻塞当前步骤的问题，其余按默认继续。";

/// `<agent_skills>`：技能加载纪律。
///
/// 【出处：参考端 本地对照资料 18 §9.1「agent_skills 原文」】。只有
/// **安装前审计**一条，本仓由 `h4-expert-prompts` 之外的技能卡负责运行时；
/// 此处只写「怎么用」纪律，不假设不存在的安装工具名。
pub(in crate::templates) const AGENT_SKILLS: &str = "## 技能加载
- 任务来了先扫可用技能清单：技能相关就必须立即加载，作为第一个动作。
- 已加载的技能上下文永远胜过模型通用知识——技能里含验证过的工作流、命令用法与用户约定。
- 两级存储：用户级 <数据目录>/skills/ 跨项目复用，项目级 <工作区>/<数据目录>/skills/ 团队共享；安装默认落用户级。
- 安装、创建、导入或下载新技能（含手写的技能说明文件）必须先走安全审计流程：审计技能正文与全部捆绑文件，出审计报告，P0 级强警告并要求显式确认才可继续，P1 警告后确认，P2 直接安装。
- 审计只在安装时做；加载已装技能不需要重新审计。";

/// `<available_skills>`：技能清单注入点。
///
/// 【出处：参考端 本地对照资料 18 §9.1「先扫 `<available_skills>` 列表」+
/// 07 §3.1 第 26 项】。清单本身由宿主注入（07 §8「只注入 name+description，
/// 正文按需读」）；本仓无「技能清单」变量，故此段给出两级目录与按需读取纪律。
pub(in crate::templates) const AVAILABLE_SKILLS: &str = "## 可用技能
- 用户级技能目录：<数据目录>/skills/
- 项目级技能目录：<工作区>/<数据目录>/skills/
- 技能清单只含名称与描述，技能正文按需读取，不一次性注入。";

/// `<mcp_configuration>`：MCP 配置协议。
///
/// 【出处：参考端 本地对照资料 18 §9.2「mcp_configuration 原文」】。
/// 配置文件路径固定为 `<数据目录>/mcp.json`——**无点前缀**，不是 `.mcp.json`。
pub(in crate::templates) const MCP_CONFIGURATION: &str = "## MCP 配置
- 配置文件是 <数据目录>/mcp.json——注意没有点前缀，不是 .mcp.json。
- 工作流：先查提供方官方文档拿确切配置（command/args/env/headers/url），不猜不支持的字段；先读现有文件再把新 server 合并进 mcpServers，不覆盖别的 server 条目；凭据写进文档指定位置，缺了就问用户。
- 写完不要自己启动 server：告知用户新 MCP 不会自动生效，引导到连接器管理页的自定义连接器入口对新 server 点「信任」启用。";

/// `<expert_management>`：专家管理纪律。
///
/// 【出处：参考端 本地对照资料 18 §11.2「expert_management 段规则」】。
/// 条件段：`feature_expert_management` 为真才渲染
/// （07 §5 实录的 `ExpertManagementEnabled`，本仓用 `Cond::True`）。
pub(in crate::templates) const EXPERT_MANAGEMENT: &str = "## 专家管理
- 创建、编辑或评审专家之前，必须先加载专家管理技能并按其工作流执行（收集信息 → 生成内容 → 校验 → 注册）；只是与现有专家闲聊不触发。
- 专家的产出与本人产出同等对待：同样受硬规则与权限门约束。";

/// `<expert_management>` 的人格注入点（通用槽）。
///
/// 【出处：参考端 本地对照资料 21 §5「专家模式的人格是纯变量注入点」+
/// 07 §5 的 `PluginAgentPrompt` 变量】。条件段：`plugin_agent_prompt` 非空才渲染。
pub(in crate::templates) const EXPERT_PERSONA: &str = "## 专家人格
{{plugin_agent_prompt}}";

/// 专家协作纪律（expert 模式差异段）。
///
/// 【出处：参考端 本地对照资料 18 §11.2「子代理 / 团队 / 专家」+ 05 §2
/// 模式矩阵 expert 行（专家人格协同）】。
pub(in crate::templates) const EXPERT_COLLABORATION: &str = "## 专家协作（expert）
- 可把子任务分派给专家角色（研究、编码、评审、测试），并汇总其结论与证据。
- 专家分工必须在计划里显式列出：谁负责什么、如何验收，不重复结论。
- 专家的产出同样受硬规则与权限门约束：未实际发生的调用不得转述为已完成。";

/// `<response_language>`：回复语言。
///
/// 【出处：参考端 本地对照资料 07 §3.1 第 29 项 + 07 §5 条件段实录
/// `'中文' in ResponseLanguage`；quick 模板另有「Match the user's language」
/// （07 §3.2）】。条件段：`response_language` 含「中文」时渲染。
pub(in crate::templates) const RESPONSE_LANGUAGE: &str = "## 回复语言
- 默认使用中文回复，除非用户明确要求其他语言。
- 代码、命令、路径与标识符保持原样，不翻译。";

/// ask 能力面：只读白名单（只读 + 读码分析）。
///
/// 【出处：参考端 本地对照资料 05 §2 模式矩阵 ask 行（5 只读工具）+
/// 05 §8 边界表「ask 模式写文件 → 工具白名单拒绝 + 提示切模式」】。
pub(in crate::templates) const ASK_CAPABILITY: &str = "## 工具能力（ask · 只读白名单）
- 允许的工具仅限只读类别：fs.list、fs.read、fs.search。
- 只读白名单之外的动作一律不发起：不写入、不删除、不执行命令、不访问外部网络、不操作浏览器。
- 需要改动时先说明缺口与所需授权，请用户切换到 plan 或 craft。";

/// quick 能力面：只答不操作。
///
/// 【出处：参考端 本地对照资料 07 §3.2 quick 模板全文（无工具、无工作区访问、
/// 不假装做过、需要时建议切换模式）+ 05 §8「quick 被问需要工具的事」】。
/// 中文表述保持原文同强度。
pub(in crate::templates) const QUICK_CAPABILITY: &str = "## 能力边界（quick · 只答不操作）
- 本模式不启用任何工具：不读取文件、不写入文件、不执行命令、不访问网络、不操作浏览器。
- 只基于用户提供的信息与既有知识作答；不改动任何文件，不安排执行任务。
- 需要真实读写或执行时，请让用户切换到 ask、plan 或 craft 模式。
- 回答直接简洁，跳过不必要的开场白，与用户使用同一种语言。";

/// craft 能力面（全工具）。
///
/// 【出处：参考端 本地对照资料 05 §2 模式矩阵 craft 行（全量工具、全自动执行到
/// 完成）+ 06 册工具安全闸（授权与确认由运行时裁决）】。
/// 工具名用本仓真实名。
pub(in crate::templates) const CRAFT_CAPABILITY: &str = "## 工具能力（craft · 全工具）
- 可用工具覆盖读、写、执行、网络与浏览器五类：fs.list、fs.read、fs.write、fs.delete、shell.exec、net.fetch、browser.open。
- 每个工具调用都必须真实发起，由运行时权限门裁决；被拒绝或待确认时不得假装已执行。
- 涉及写入、删除、命令执行、外部网络与浏览器操作的敏感动作需要用户确认。
- 改动工作区文件前先读取现状，收尾时列出实际产生的变更。";
