//! 压缩提示词：模板、规则与两套提示的拼装（照抄 opencode `compaction.ts:104-179`）。
//!
//! **本文件只放常量与拼串**：没有状态、没有 IO、没有时钟，纯函数 ⇒ 同输入同输出。
//!
//! 逐字对照（`packages/core/src/session/compaction.ts`）：
//! - [`SUMMARY_TEMPLATE`] ← `:104-135`（八段结构，逐字照抄，**不翻译**）；
//! - [`SUMMARY_RULES`] ← `:137-143`（六条规则，逐字照抄，**不翻译**）；
//! - [`build_prompt`] ← `:145-176`（首次 / 更新两套）；
//! - [`NUDGE`] ← `:178-179`（回复没填模板时提醒一次）。
//!
//! **偏离参考件一处**：参考件的 `buildPrompt(update, legacy)` 有 `legacy` 分支
//! （旧格式 checkpoint 重写，`:166-170`）。本仓从零开始、没有历史 checkpoint
//! 格式，**该分支刻意省略**（见 `docs/compaction-status.md`）。

/// 摘要模板（八段）。逐字照抄 opencode `compaction.ts:104-135`。
///
/// 段落：`## Objective` / `## Requirements` / `## Decisions` / `## Work State`
/// （含 `### Completed` / `### Active` / `### Blocked`）/ `## Next Move` /
/// `## Relevant Files` / `## Important Context`。
pub const SUMMARY_TEMPLATE: &str = r#"You MUST use this format for your response (you may omit sections that aren't applicable). Do not include the <template> tags in your response.
<template>
## Objective
- [one or two brief sentences describing what the user is trying to accomplish]

## Requirements
- [constraints, preferences, requirements, and scope boundaries stated by the user, or "(none)"]

## Decisions
- [decisions already made and why, or "(none)"]

## Work State
Break the objective into smaller goals and report which are completed, which are being worked on, and which are blocked.
### Completed
- [goals that have been completed; otherwise "(none)"]

### Active
- [goals currently being worked on; otherwise "(none)"]

### Blocked
- [anything blocking progress, and why; otherwise "(none)"]

## Next Move
1. [ordered list of next actions, or "(none)"]

## Relevant Files
List the files and directories, other than the current working directory, that another agent would need to open to continue this work. Include at most 15, most important first. Do not list every file that was read or changed. Include paths outside the current working directory when relevant. If none, write "(none)".
- `[file or directory path]`: [brief reason it matters]

## Important Context
- [facts the next agent cannot continue without and cannot easily find on its own; or "(none)"]
</template>"#;

/// 摘要规则（六条）。逐字照抄 opencode `compaction.ts:137-143`。
///
/// 简洁单行 bullet、优先短引用、保留精确路径/符号/命令/错误串/URL/标识符、
/// 只携带**未解决**的用户问题、保留工作流状态（未提交/已提交/已推送/审查中）、
/// 不提压缩过程。
pub const SUMMARY_RULES: &str = r#"Rules:
- Keep each section concise. Use terse, single-line bullets, not prose paragraphs or nested lists.
- Prefer short references over detailed restatement. It is fine to leave out information the next agent can recover from the code or the files listed above.
- Preserve exact file paths, symbols, commands, error strings, URLs, and identifiers.
- Carry forward only user questions or requests that remain unanswered or require further action. Do not repeat ones that newer history has answered or resolved. Preserve exact wording when carrying one forward.
- Preserve consequential workflow state, including whether changes are uncommitted, committed, pushed, under review, or merged.
- Do not mention the summary process or that context was compacted."#;

/// 回复没填模板时的一次性提醒。逐字照抄 opencode `compaction.ts:178-179`。
pub const NUDGE: &str = "The previous response did not fill in the required summary template. Do not call tools. Return the summary as text using the exact section headings from the template.";

/// 构造压缩提示词（照抄 `compaction.ts:145-176`）。
///
/// - `update = false`：**首次**压缩（没有既有 checkpoint）；
/// - `update = true`：**更新**既有 checkpoint（历史里已有一次 completed 压缩）。
///
/// 参考件的 `legacy` 分支（旧格式 checkpoint 重写）本仓**刻意省略**：本仓没有
/// 历史 checkpoint 格式，见 `docs/compaction-status.md`。
pub fn build_prompt(update: bool) -> String {
    let shared = [
        "Summarize only what the user and the assistant said and did. Leave out instructions and setup the assistant was given rather than told by the user: repository conventions, instruction files such as AGENTS.md, and environment details like the session ID. The next agent receives current versions of all of these separately.",
        SUMMARY_TEMPLATE,
        SUMMARY_RULES,
        "Do not continue the task or call tools.",
        "Return only the structured summary in the requested format. Do not include a preamble, explanation, or other commentary.",
    ];
    if !update {
        return [
            "You MUST summarize the conversation above into a structured summary that will be given to another agent to resume the work.",
            shared[0],
            shared[1],
            shared[2],
            shared[3],
            shared[4],
        ]
        .join("\n\n");
    }
    [
        "Update the existing checkpoint in the conversation above into one consolidated summary.",
        "Newer history always takes precedence over the existing checkpoint. Preserve previous information unless newer history clearly contradicts, supersedes, resolves, or makes it stale. If something is no longer relevant to continuing the work, you may remove it.",
        "Incorporate newer requirements, decisions, progress, and context. Reconcile Work State and Next Move: move completed work out of Active, remove resolved blockers and answered questions, and preserve unresolved or pending work.",
        "Return only the updated Markdown sections. Do not reproduce the `<conversation-checkpoint>`, `<summary>`, or `<recent-context>` wrapper tags from the previous checkpoint.",
        shared[0],
        shared[1],
        shared[2],
        shared[3],
        shared[4],
    ]
    .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首次提示与更新提示的头一句各自照抄参考件。
    #[test]
    fn first_and_update_prompts_open_with_reference_lines() {
        let first = build_prompt(false);
        assert!(first.starts_with(
            "You MUST summarize the conversation above into a structured summary that will be given to another agent to resume the work."
        ));
        let update = build_prompt(true);
        assert!(update.starts_with(
            "Update the existing checkpoint in the conversation above into one consolidated summary."
        ));
        assert!(
            update.contains("Newer history always takes precedence over the existing checkpoint.")
        );
        assert!(
            update.contains("Incorporate newer requirements, decisions, progress, and context.")
        );
        assert!(update.contains("Return only the updated Markdown sections."));
    }

    /// 两套提示都带完整的模板与规则（逐段可 grep）。
    #[test]
    fn both_prompts_carry_template_and_rules() {
        for prompt in [build_prompt(false), build_prompt(true)] {
            assert!(prompt.contains(SUMMARY_TEMPLATE));
            assert!(prompt.contains(SUMMARY_RULES));
            for heading in [
                "## Objective",
                "## Requirements",
                "## Decisions",
                "## Work State",
                "### Completed",
                "### Active",
                "### Blocked",
                "## Next Move",
                "## Relevant Files",
                "## Important Context",
            ] {
                assert!(prompt.contains(heading), "提示缺段落：{heading}");
            }
            assert!(prompt.contains("Do not continue the task or call tools."));
        }
    }
}
