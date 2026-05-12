use super::common::{default_skills_dir, elapsed_ms, result_metadata, skills_root_for_tests};
use crate::skills::{create_skill, SkillCreation, SkillResourceKind};
use agentos_interfaces::tool::{Tool, ToolError, ToolSpec};
use agentos_proto::{ToolCall, ToolResult, ToolStatus};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, value::RawValue};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

/// Tool wrapping `crate::skills::create_skill` so model-driven sub-agents can
/// scaffold new workspace skills end-to-end (folder, frontmatter SKILL.md,
/// optional resource subdirectories, and validation pass) inside the normal
/// run-loop approval and guardrail flow.
#[derive(Default)]
pub struct SkillCreatorTool;

/// Deserialised tool input. `root` is intentionally *not* exposed on the
/// LLM-visible schema and not accepted from callers — the runtime resolves
/// the skills directory itself (matching the cron-tool fix). Tests redirect
/// writes via `TEST_SKILLS_DIR` in `super::common`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillCreateArgs {
    name: String,
    description: String,
    #[serde(default)]
    resources: Vec<String>,
}

#[async_trait]
impl Tool for SkillCreatorTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("skill_create"),
            description: Arc::from(
                "Scaffold a new workspace skill following the Anthropic \
                 SKILL.md folder format. Creates workspace/skills/<name>/SKILL.md \
                 with YAML frontmatter and an optional set of bundled resource \
                 directories (scripts, references, assets). Validates the \
                 result before returning.",
            ),
            input_schema: json!({
                "type": "object",
                "required": ["name", "description"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name in lowercase hyphen-case (e.g. rss-digest)."
                    },
                    "description": {
                        "type": "string",
                        "description": "One-line description used as the catalog summary."
                    },
                    "resources": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["scripts", "references", "assets"] },
                        "description": "Optional bundled resource subdirectories to create."
                    }
                }
            }),
            requires_isolation: false,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let parsed: SkillCreateArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let start = Instant::now();

        let mut resources = BTreeSet::new();
        for entry in &parsed.resources {
            let kind = match entry.as_str() {
                "scripts" => SkillResourceKind::Scripts,
                "references" => SkillResourceKind::References,
                "assets" => SkillResourceKind::Assets,
                other => {
                    return Err(ToolError::Failed(Arc::from(format!(
                        "unknown resource kind '{other}'; expected scripts, references, or assets"
                    ))));
                }
            };
            resources.insert(kind);
        }

        let creation = SkillCreation {
            name: Arc::from(parsed.name.as_str()),
            description: Arc::from(parsed.description.as_str()),
            resources,
        };
        let root = skills_root_for_tests().unwrap_or_else(default_skills_dir);
        let skill = create_skill(&root, &creation)
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;

        let message = format!("created skill '{}' at {}", skill.name, skill.path.display());
        let bytes_out = message.len() as u64;
        Ok(ToolResult {
            call_id: call.id.clone(),
            status: ToolStatus::Succeeded,
            content: Arc::from(message),
            metadata: result_metadata(elapsed_ms(start), bytes_out),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::common::test_support::{tool_call, SkillsDirGuard};
    use super::*;

    #[tokio::test]
    async fn skill_creator_tool_scaffolds_skill_dir_and_markdown() {
        let guard = SkillsDirGuard::new("skill-creator-tool");
        let args = json!({
            "name": "test-skill",
            "description": "Smoke test for SkillCreatorTool.",
            "resources": ["scripts"],
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let result = SkillCreatorTool
            .call(&tool_call("skill_create", "call_1"), &raw)
            .await
            .unwrap();
        assert_eq!(result.status, ToolStatus::Succeeded);
        assert!(result.content.contains("test-skill"));
        let skill_md = guard.dir.join("test-skill").join("SKILL.md");
        let body = std::fs::read_to_string(&skill_md).unwrap();
        assert!(body.contains("name: test-skill"));
        assert!(body.contains("Smoke test for SkillCreatorTool."));
        assert!(guard.dir.join("test-skill").join("scripts").is_dir());
    }

    #[tokio::test]
    async fn skill_creator_tool_rejects_root_override_from_caller() {
        // Regression: the model could previously pass `root="workspace"`
        // and have SKILL.md land outside `workspace/skills/`. Schema no
        // longer advertises `root`, and deny_unknown_fields makes silent
        // misuse impossible.
        let _guard = SkillsDirGuard::new("skill-creator-rooted");
        let args = json!({
            "name": "rooted-skill",
            "description": "Attempts directory escape.",
            "root": "workspace",
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = SkillCreatorTool
            .call(&tool_call("skill_create", "call_root"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("unknown field") && msg.contains("root"));
    }

    #[tokio::test]
    async fn skill_creator_tool_rejects_unknown_resource() {
        let _guard = SkillsDirGuard::new("skill-creator-bad");
        let args = json!({
            "name": "another-skill",
            "description": "n/a",
            "resources": ["nope"],
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = SkillCreatorTool
            .call(&tool_call("skill_create", "call_2"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("unknown resource kind"));
    }
}
