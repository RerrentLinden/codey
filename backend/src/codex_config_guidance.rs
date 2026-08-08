pub(crate) const SUBAGENT_GUIDANCE: &str = r#"## 子代理使用

子代理在我们的工作里用于探索，他是你的探子。
把子代理当成你手边最顺手的、用于「宽而重」读取的工具。工作的任何时候，只要你觉得需要就可以派。只有在它能减少主线程上下文污染、提高并行度或者提供独立核验的时候才使用。
必须遵守：你需要更激进和更频繁地调用子代理，在任何需要的情况下，而不仅仅只是在对话的开头。我们需要更频繁的子代理调用来避免上下文腐烂，你承担子代理编排者的角色。

### 何时直接处理

直接读取以及处理以下内容，不派子代理：

- 已知位置的小文件、少量代码或者单一事实；
- 即将修改的具体代码；
- 派发、等待以及复核的成本不低于自己读取的任务。
- 奠基性文档，无论多长都自己读：架构文档、设计文档、交接备忘录（在别的工作流里可能是别的名字）等用来让你建立全局视角、充当后续判断地基的文件——它们的价值全在细节与脉络，一经子代理转译即失真，长度不构成外包的理由。

### 何时适合派发

适合交给子代理的：

- 巨型大文件（奠基性文档除外，见上）、跨文件或者跨目录的检索；
- 相互独立、可以并行的探索或者核验；
- 长任务当中需要重新确认模块现状的；
- 会产生大量日志、搜索结果或者外围材料的阅读。

多个独立的任务应当并发派发。

### 委派与验证

给子代理的任务必须是自包含的，说明检索范围、具体问题以及期望的输出。精度重要的时候，要求返回 `file:line`、符号名以及必要的关键原文——这些出处就是你之后廉价复核的抓手。

子代理的结果只是线索，可能遗漏或者出错。但复核不是把它读过的东西重读一遍，那样这次派发就白费了——你买的是「压缩」，重读会把压缩当场退光。复核 = 顺着它给的 `file:line` 以及关键原文来。抽查真的需要主代理亲自阅读的那几小部分，别去重新通读整份材料；既然把「读」外包了出去，就靠它压缩之后的结论来干活，只在结论要紧或者可疑的时候回去点验出处。

唯二需要你亲自完整读原文的是：① 即将修改的确切代码，② 奠基性文档——这两类本就不外包（见「何时直接处理」）。对它们，子代理至多帮你定位，读由你亲自来：定位与阅读是分工，并非重复劳动。

子代理默认只做探索、检索以及核验。代码修改、方案取舍以及最终验证由主代理来负责。

### 派发机制

- 是否派、派几个由主代理自主决定，无需用户明确要求；较重的探索应当拆成多个独立的轻任务来并发派发。
- 我们系统允许最大并行7个会话进程。所以你最多可以并行分派 6 个子代理；子代理模型的成本较低，无需去顾虑并行派发的成本，只要任务需要就积极使用。
- 子代理一律使用默认配置：工具支持角色参数的时候显式指定 `agent_role = "default"` 或者 `agent_type = "default"`；不支持的时候省略角色、由泛型派生加载 `default.toml`。禁用 `explorer`、`worker` 或者其他角色。
- 派生的时候**必须**显式 `fork_turns = "none"`，不复制主代理的历史，让每个探子都保持干净、快、不背主代理正在腐烂的上下文（代价即上文「任务必须自包含」）。
- 需要多个子代理的时候在同一轮并发派发；派发之后主代理立即 `wait_agent`，停止其余的分析、检索、命令执行以及文件修改，直至全部返回。
- 收到某个子代理结果之后，如果提供了 `close_agent` 就必须立即关闭；每个子代理只用一轮，不复用、不追派。
- 特别注意：子代理自派生起累计运行 10 分钟仍未完成：视为异常，主代理必须介入、不得继续盲等；检查代理状态或运行记录，已有可用 MESSAGE 时采用其部分结果，然后停止这个子代理。并自行判断是否需要再派生或拆分更小任务重新分派。"#;

pub(crate) const DEFAULT_AGENT_CONFIG: &str = r#####"name = "default"

description = "General-purpose exploration subagent using the configured default model and reasoning effort."

developer_instructions = """
你是通用子代理，是主代理派出去的探子。你只做探索、检索、核验：不改动任何东西，不做方案取舍或者最终判断——那些是主代理的事。
不要派生、调用或者请求新的子代理；任务若是需要进一步拆分，把拆分的建议返回给主代理。

你交回给主代理的东西：
- 你的产出直接喂给主代理、是它据以行动的数据，并非给人看的。密而不水，不寒暄、不复述过程、不下客套结论。
- 给证据，不给包装：关键处附上 `file:line`、符号名、必要的逐字原文。主代理会靠这些出处来抽查你、省去重读原文，所以出处必须准、且足以让它核验。
- 把「看到的事实」以及「你的推断」分开，存疑的明确标注——别把猜测写成事实。
- 压缩体量，但承重的精确信息（确切的名字、签名、取值、路径）一字不改地留住，别在转述里磨没了。

你怎么工作：
- 你只有一轮、任务是自包含的：没有追问的机会，别反问；用这一轮把任务范围查到位、尽力答全。
- 答不全就如实交代「查到了什么、还有什么没覆盖、哪里存疑或者矛盾」。宁可显式报「没查到 / 没覆盖」，也别用含糊的话糊弄过去——你悄悄漏掉的，主代理无从复核。
- 每次工具调用都必须推进任务本身。进度、道歉、自我提醒和纠错写在回复中；发现工具用错时直接改用正确工具，不要为此额外执行诊断或播报命令。
"""

[features]
image_generation = false
"#####;

pub(crate) const CODEY_FASTCTX_GUIDANCE: &str = "Codey FastCtx context tools are enabled. Use \
`mcp__codey_fastctx__read`, `mcp__codey_fastctx__grep`, `mcp__codey_fastctx__glob`, and \
`mcp__codey_fastctx__replace` for local workspace files. Call them directly when visible. When these \
functions are available inside a code-mode program, use the same names on the `tools` object, for example \
`await tools.mcp__codey_fastctx__read({ file_path: absolutePath })`. Keep local file reading, \
content search, discovery, and deterministic replacement on these FastCtx functions; no separate \
tool discovery is needed. Set `file_path` to a plain absolute filesystem path (never a URI); on \
Windows, convert the reference to a drive-letter path such as `E:/repo/file.ts` before the call. Use \
terminal commands only for builds, tests, Git, package managers, or after a FastCtx function actually \
fails. Every tool call must advance the requested task; put progress and corrections in commentary. \
Follow every Complete or Partial continuation exactly.";

pub(crate) const PREVIOUS_CODEY_FASTCTX_GUIDANCE: &str = "Codey FastCtx context tools are enabled. \
Local files have exactly one read route: call `mcp__codey_fastctx__read` directly, including when the \
input is a URI-shaped local reference. `mcp__codey_fastctx` is a direct tool namespace, not an MCP \
Resources server name. Never call `list_mcp_resources`, `list_mcp_resource_templates`, or \
`read_mcp_resource` during local workspace work, including discovery or probe calls with placeholder \
server names; never pass `mcp__codey_fastctx` to a `resources/*` method. Never use exec or shell \
commands such as `Write-Output`, `Write-Error`, `echo`, `printf`, `exit`, or `sleep` to narrate \
progress, apologize, or record self-reminders; continue directly with the correct tool instead. Set \
`file_path` to a plain absolute filesystem path (never a URI); on Windows, convert the reference to \
a drive-letter path such as `E:/repo/file.ts` before the call. For content search and file discovery, \
always use `mcp__codey_fastctx__grep` and \
`mcp__codey_fastctx__glob` before exec or shell commands. Do not use cat, sed, rg, grep, find, or \
recursive ls when a FastCtx tool covers the operation. Use exec only for builds, tests, Git, package \
managers, or when the FastCtx tool is unavailable or fails. Use `mcp__codey_fastctx__replace` only \
for deterministic mechanical replacements, and follow every Complete or Partial continuation \
exactly.";

pub(crate) const OLDER_CODEY_FASTCTX_GUIDANCE_V2: &str = "Codey FastCtx context tools are enabled. \
Local files have exactly one read route: call `mcp__codey_fastctx__read` directly, including when the \
input is a URI-shaped local reference. Set `file_path` to a plain absolute filesystem path (never a \
URI); on Windows, convert the reference to a drive-letter path such as `E:/repo/file.ts` before the \
call. For content search and file discovery, always use `mcp__codey_fastctx__grep` and \
`mcp__codey_fastctx__glob` before exec or shell commands. Do not use cat, sed, rg, grep, find, or \
recursive ls when a FastCtx tool covers the operation. Use exec only for builds, tests, Git, package \
managers, or when the FastCtx tool is unavailable or fails. Use `mcp__codey_fastctx__replace` only \
for deterministic mechanical replacements, and follow every Complete or Partial continuation \
exactly.";

pub(crate) const OLDER_CODEY_FASTCTX_GUIDANCE: &str = "Codey FastCtx context tools are enabled. \
For local file reading, content search, and file discovery, always use \
`mcp__codey_fastctx__read`, `mcp__codey_fastctx__grep`, and `mcp__codey_fastctx__glob` before exec \
or shell commands. Do not use cat, sed, rg, grep, find, or recursive ls when a FastCtx tool covers \
the operation. Use exec only for builds, tests, Git, package managers, or when the FastCtx tool is \
unavailable or fails. Use `mcp__codey_fastctx__replace` only for deterministic mechanical \
replacements, and follow every Complete or Partial continuation exactly.";

pub(crate) const LEGACY_CODEY_FASTCTX_GUIDANCE: &str = "Codey FastCtx context tools are enabled. Prefer \
`mcp__codey_fastctx__read`, `mcp__codey_fastctx__grep`, and \
`mcp__codey_fastctx__glob` over shell commands for local file inspection. Use \
`mcp__codey_fastctx__replace` only for deterministic batch replacements, and \
follow every Complete or Partial pagination note exactly.";

pub(crate) const CODEY_FASTCTX_GUIDANCE_VERSIONS: &[&str] = &[
    CODEY_FASTCTX_GUIDANCE,
    PREVIOUS_CODEY_FASTCTX_GUIDANCE,
    OLDER_CODEY_FASTCTX_GUIDANCE_V2,
    OLDER_CODEY_FASTCTX_GUIDANCE,
    LEGACY_CODEY_FASTCTX_GUIDANCE,
];

const DEFAULT_FASTCTX_TOOL_NAMESPACE: &str = "mcp__codey_fastctx";

pub(crate) fn codey_fastctx_guidance_for_namespace(namespace: &str) -> String {
    CODEY_FASTCTX_GUIDANCE.replace(DEFAULT_FASTCTX_TOOL_NAMESPACE, namespace)
}

pub(crate) fn default_agent_config_with_fastctx_guidance(namespace: Option<&str>) -> String {
    let Some(namespace) = namespace else {
        return DEFAULT_AGENT_CONFIG.to_string();
    };
    let guidance = codey_fastctx_guidance_for_namespace(namespace);
    let marker = "\n\"\"\"\n\n[features]\n";
    let replacement = format!("\n\n{guidance}\n\"\"\"\n\n[features]\n");
    DEFAULT_AGENT_CONFIG.replacen(marker, &replacement, 1)
}

pub(crate) fn codey_fastctx_guidance_blocks(current: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    for &guidance in CODEY_FASTCTX_GUIDANCE_VERSIONS {
        if current.contains(guidance) {
            blocks.push(guidance.to_string());
        }

        let Some(prefix_end) = guidance.find(DEFAULT_FASTCTX_TOOL_NAMESPACE) else {
            continue;
        };
        let prefix = &guidance[..prefix_end];
        for (start, _) in current.match_indices(prefix) {
            let Some(dynamic_guidance) =
                dynamic_codey_fastctx_guidance_at(current, start, guidance)
            else {
                continue;
            };
            if !blocks.iter().any(|block| block == &dynamic_guidance) {
                blocks.push(dynamic_guidance);
            }
        }
    }
    blocks
}

fn dynamic_codey_fastctx_guidance_at(
    current: &str,
    start: usize,
    guidance_template: &str,
) -> Option<String> {
    let prefix_end = guidance_template.find(DEFAULT_FASTCTX_TOOL_NAMESPACE)?;
    let after_prefix = current.get(start + prefix_end..)?;
    let namespace_end = after_prefix.find("__read`")?;
    let namespace = &after_prefix[..namespace_end];
    if namespace.is_empty()
        || namespace.contains('`')
        || namespace.contains('\n')
        || namespace.contains('\r')
        || !namespace.starts_with("mcp__")
    {
        return None;
    }
    let guidance = guidance_template.replace(DEFAULT_FASTCTX_TOOL_NAMESPACE, namespace);
    current[start..].starts_with(&guidance).then_some(guidance)
}

pub(crate) fn append_subagent_guidance(existing: &str) -> String {
    if existing.contains(SUBAGENT_GUIDANCE) {
        return existing.to_string();
    }
    let mut updated = existing.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(SUBAGENT_GUIDANCE);
    updated.push('\n');
    updated
}

pub(crate) fn remove_subagent_guidance(current: &str) -> Option<String> {
    let guidance_start = current.find(SUBAGENT_GUIDANCE)?;
    let mut owned_start = guidance_start;
    if current[..owned_start].ends_with("\n\n") {
        owned_start -= 2;
    }
    let mut owned_end = guidance_start + SUBAGENT_GUIDANCE.len();
    if current[owned_end..].starts_with('\n') {
        owned_end += 1;
    }
    let mut restored = current[..owned_start].to_string();
    restored.push_str(&current[owned_end..]);
    Some(restored)
}

pub(crate) fn remove_codey_fastctx_guidance(current: &str) -> Option<String> {
    let mut restored = current.to_string();
    let mut changed = false;
    for guidance in codey_fastctx_guidance_blocks(current) {
        while let Some(without_guidance) = remove_guidance_paragraph(&restored, &guidance) {
            restored = without_guidance;
            changed = true;
        }
    }
    changed.then_some(restored)
}

pub(crate) fn remove_owned_guidance_block(current: &str, guidance: &str) -> Option<String> {
    let guidance_start = current.find(guidance)?;
    Some(remove_guidance_at(current, guidance_start, guidance.len()))
}

fn remove_guidance_paragraph(current: &str, guidance: &str) -> Option<String> {
    let guidance_start = current.match_indices(guidance).find_map(|(start, _)| {
        let end = start + guidance.len();
        let starts_paragraph = start == 0 || current[..start].ends_with("\n\n");
        let ends_paragraph = end == current.len() || current[end..].starts_with("\n\n");
        (starts_paragraph && ends_paragraph).then_some(start)
    })?;
    Some(remove_guidance_at(current, guidance_start, guidance.len()))
}

fn remove_guidance_at(current: &str, guidance_start: usize, guidance_len: usize) -> String {
    let guidance_end = guidance_start + guidance_len;
    let (owned_start, owned_end) = if current[..guidance_start].ends_with("\n\n") {
        (guidance_start - 2, guidance_end)
    } else if current[guidance_end..].starts_with("\n\n") {
        (guidance_start, guidance_end + 2)
    } else if current[..guidance_start].ends_with('\n') {
        (guidance_start - 1, guidance_end)
    } else if current[guidance_end..].starts_with('\n') {
        (guidance_start, guidance_end + 1)
    } else {
        (guidance_start, guidance_end)
    };
    format!("{}{}", &current[..owned_start], &current[owned_end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fastctx_guidance_routes_uri_shaped_local_files_through_the_read_tool() {
        assert_eq!(
            codey_fastctx_guidance_for_namespace("mcp__codey_fastctx"),
            CODEY_FASTCTX_GUIDANCE
        );
        assert!(CODEY_FASTCTX_GUIDANCE.contains("Call them directly when visible"));
        assert!(CODEY_FASTCTX_GUIDANCE.contains("tools.mcp__codey_fastctx__read"));
        assert!(CODEY_FASTCTX_GUIDANCE.contains("available inside a code-mode program"));
        assert!(
            CODEY_FASTCTX_GUIDANCE
                .contains("`file_path` to a plain absolute filesystem path (never a URI)")
        );
        assert!(CODEY_FASTCTX_GUIDANCE.contains("drive-letter path such as `E:/repo/file.ts`"));
        assert!(CODEY_FASTCTX_GUIDANCE.contains("no separate tool discovery is needed"));
        assert!(CODEY_FASTCTX_GUIDANCE.contains("Every tool call must advance the requested task"));
        assert!(!CODEY_FASTCTX_GUIDANCE.contains("list_mcp_resources"));
        assert!(!CODEY_FASTCTX_GUIDANCE.contains("read_mcp_resource"));
        assert!(!CODEY_FASTCTX_GUIDANCE.contains("Write-Output"));
        assert!(!CODEY_FASTCTX_GUIDANCE.contains("file:///"));
    }

    #[test]
    fn default_agent_config_can_include_the_fastctx_namespace_guidance() {
        let config = default_agent_config_with_fastctx_guidance(Some("mcp__fastctx"));

        assert!(config.contains("tools.mcp__fastctx__read"));
        assert!(config.contains("Call them directly when visible"));
        assert!(config.contains("no separate tool discovery is needed"));
        assert!(config.contains("put progress and corrections in commentary"));
        assert!(!config.contains("list_mcp_resources"));
        assert!(!config.contains("read_mcp_resource"));
        assert!(!config.contains("Write-Output"));
        assert!(!config.contains("mcp__codey_fastctx"));
        assert!(config.contains("[features]"));
        assert!(config.ends_with("image_generation = false\n"));
    }

    #[test]
    fn default_agent_never_uses_terminal_commands_as_narration() {
        assert!(DEFAULT_AGENT_CONFIG.contains("每次工具调用都必须推进任务本身"));
        assert!(DEFAULT_AGENT_CONFIG.contains("进度、道歉、自我提醒和纠错写在回复中"));
        assert!(DEFAULT_AGENT_CONFIG.contains("直接改用正确工具"));
        assert!(!DEFAULT_AGENT_CONFIG.contains("Write-Output"));
        assert!(!DEFAULT_AGENT_CONFIG.contains("Write-Error"));
    }

    #[test]
    fn fastctx_guidance_cleanup_removes_every_codey_owned_version() {
        let user_server_guidance = CODEY_FASTCTX_GUIDANCE_VERSIONS
            .iter()
            .map(|guidance| guidance.replace(DEFAULT_FASTCTX_TOOL_NAMESPACE, "mcp__fastctx"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let configured = format!(
            "User guidance.\n\n{}\n\n{user_server_guidance}\n\nConcurrent guidance.",
            CODEY_FASTCTX_GUIDANCE_VERSIONS.join("\n\n"),
        );

        assert_eq!(
            remove_codey_fastctx_guidance(&configured).as_deref(),
            Some("User guidance.\n\nConcurrent guidance.")
        );
    }

    #[test]
    fn fastctx_guidance_blocks_detect_user_fastctx_namespaces() {
        let user_server_guidance = CODEY_FASTCTX_GUIDANCE_VERSIONS
            .iter()
            .map(|guidance| guidance.replace(DEFAULT_FASTCTX_TOOL_NAMESPACE, "mcp__context_tools"))
            .collect::<Vec<_>>();
        let configured = format!("Prefix\n\n{}\n\nSuffix", user_server_guidance.join("\n\n"));

        assert_eq!(
            codey_fastctx_guidance_blocks(&configured),
            user_server_guidance
        );
    }
}
