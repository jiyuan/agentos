//! Deterministic planner for the `skill-creator` workspace skill.
//!
//! Detects compact "create skill: NAME, DESCRIPTION [, resources=...]" prefixes
//! in the latest user message and emits a `Plan::CallTool` against the
//! `skill_create` built-in tool. Natural-language requests are left to the
//! LLM, which sees `skill_create` in its tools schema and can call it
//! directly.

use super::WorkspaceSkillCatalog;
use agentos_interfaces::orchestrator::{OrchestratorError, Plan, RunContext};
use agentos_proto::{Message, MessageRole, ToolCall, ToolCallId};
use serde_json::{json, value::RawValue, Value};
use std::sync::Arc;

pub struct SkillCreatorSkill<'a> {
    catalog: &'a WorkspaceSkillCatalog,
}

impl<'a> SkillCreatorSkill<'a> {
    pub fn new(catalog: &'a WorkspaceSkillCatalog) -> Self {
        Self { catalog }
    }

    pub fn plan(&self, ctx: &RunContext<'_>) -> Result<Option<Plan>, OrchestratorError> {
        if !self.catalog.contains("skill-creator") {
            return Ok(None);
        }
        let Some(item) = ctx.state.transcript.items.last() else {
            return Ok(None);
        };
        match item.message.role {
            MessageRole::User => self.plan_create(&item.message.content),
            MessageRole::Tool => self.plan_acknowledgement(ctx),
            MessageRole::Assistant | MessageRole::System => Ok(None),
        }
    }

    fn plan_create(&self, input: &str) -> Result<Option<Plan>, OrchestratorError> {
        let Some(parsed) = parse_create_prefix(input) else {
            return Ok(None);
        };
        let mut args = json!({
            "name": parsed.name,
            "description": parsed.description,
        });
        if !parsed.resources.is_empty() {
            args["resources"] = Value::from(parsed.resources.clone());
        }
        let raw_args = RawValue::from_string(args.to_string())
            .map_err(|err| OrchestratorError::Backend(err.to_string().into()))?;
        Ok(Some(Plan::CallTool(ToolCall {
            id: ToolCallId::new(format!("skill-creator-{}", parsed.name)),
            name: Arc::from("skill_create"),
            args: raw_args,
        })))
    }

    fn plan_acknowledgement(
        &self,
        ctx: &RunContext<'_>,
    ) -> Result<Option<Plan>, OrchestratorError> {
        if !previous_user_requested_skill_create(ctx) {
            return Ok(None);
        }
        let Some(item) = ctx.state.transcript.items.last() else {
            return Ok(None);
        };
        // Echo the tool's success message back as the assistant reply so the
        // run can complete without a second LLM round-trip when the prefix
        // shortcut is used. Natural-language requests skip this branch
        // because `previous_user_requested_skill_create` only matches the
        // explicit `create skill:` prefix.
        Ok(Some(Plan::Reply(Message::text(
            MessageRole::Assistant,
            Arc::clone(&item.message.content),
        ))))
    }
}

#[derive(Debug, PartialEq)]
struct ParsedCreate {
    name: String,
    description: String,
    resources: Vec<String>,
}

fn parse_create_prefix(input: &str) -> Option<ParsedCreate> {
    let body = input
        .strip_prefix("create skill:")
        .or_else(|| input.strip_prefix("skill create:"))?
        .trim();
    // Split on the first comma between name and description, then process any
    // trailing `resources=a|b|c` field tolerantly.
    let mut parts = body.splitn(3, ',').map(str::trim).collect::<Vec<_>>();
    let name = parts.first()?.to_string();
    if name.is_empty() {
        return None;
    }
    let description = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
    if description.is_empty() {
        return None;
    }
    let resources = parts
        .pop()
        .filter(|_| parts.len() >= 2)
        .and_then(parse_resources_tail)
        .unwrap_or_default();
    Some(ParsedCreate {
        name,
        description,
        resources,
    })
}

fn parse_resources_tail(tail: &str) -> Option<Vec<String>> {
    let rest = tail.strip_prefix("resources=")?;
    Some(
        rest.split(['|', ','])
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| matches!(s.as_str(), "scripts" | "references" | "assets"))
            .collect(),
    )
}

fn previous_user_requested_skill_create(ctx: &RunContext<'_>) -> bool {
    ctx.state.transcript.items.iter().rev().skip(1).any(|item| {
        if item.message.role != MessageRole::User {
            return false;
        }
        let lower = item.message.content.to_ascii_lowercase();
        lower.starts_with("create skill:") || lower.starts_with("skill create:")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_description() {
        let parsed = parse_create_prefix("create skill: rss-digest, fetch and summarize feeds")
            .expect("should parse");
        assert_eq!(parsed.name, "rss-digest");
        assert_eq!(parsed.description, "fetch and summarize feeds");
        assert!(parsed.resources.is_empty());
    }

    #[test]
    fn accepts_skill_create_synonym() {
        let parsed =
            parse_create_prefix("skill create: foo, bar baz").expect("synonym should parse");
        assert_eq!(parsed.name, "foo");
    }

    #[test]
    fn parses_optional_resources() {
        let parsed = parse_create_prefix(
            "create skill: rss-digest, fetch feeds, resources=scripts|references",
        )
        .expect("should parse");
        assert_eq!(parsed.resources, vec!["scripts", "references"]);
    }

    #[test]
    fn rejects_missing_description() {
        assert!(parse_create_prefix("create skill: rss-digest").is_none());
    }

    #[test]
    fn rejects_unrelated_prompts() {
        assert!(parse_create_prefix("write me a haiku").is_none());
        assert!(parse_create_prefix("please install a skill").is_none());
    }
}
