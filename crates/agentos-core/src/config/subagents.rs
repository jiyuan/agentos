use super::normalize::{normalize_config_token, normalize_domain};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct SubAgentConfig {
    pub name: Arc<str>,
    pub id: Arc<str>,
    pub description: Arc<str>,
    pub developer_instructions: Arc<str>,
    pub policy_id: Arc<str>,
    pub orchestrator: Arc<str>,
    pub model_tier: Arc<str>,
    pub tools: Vec<Arc<str>>,
    pub memory_view: Arc<str>,
    pub memory_domains: Vec<Arc<str>>,
    pub memory_tools: Vec<Arc<str>>,
    pub max_turns: usize,
    pub inherit_guardrails: bool,
    /// Character cap for `MaxOutputLength` when `inherit_guardrails = true`.
    /// Tripped output aborts the run, so this needs to comfortably exceed any
    /// reply you expect from the model. Defaults are tuned for chat: long
    /// enough to fit a thorough multi-paragraph answer, short enough to catch
    /// runaway generation.
    pub max_output_chars: usize,
}

impl Default for SubAgentConfig {
    fn default() -> Self {
        Self {
            name: Arc::from("research-subagent"),
            id: Arc::from("research-subagent"),
            description: Arc::from(""),
            developer_instructions: Arc::from(""),
            policy_id: Arc::from("readonly-web"),
            orchestrator: Arc::from("builtin.max"),
            model_tier: Arc::from("medium"),
            tools: vec![Arc::from("http")],
            memory_view: Arc::from("none"),
            memory_domains: Vec::new(),
            memory_tools: Vec::new(),
            max_turns: 4,
            inherit_guardrails: true,
            max_output_chars: 64_000,
        }
    }
}

pub(super) fn normalize_memory_view(input: &str) -> Result<String, String> {
    match normalize_config_token(input).as_str() {
        "none" | "" => Ok("none".to_owned()),
        "shared_readonly" | "shared_read_only" => Ok("shared_readonly".to_owned()),
        "shared_readwrite" | "shared_read_write" => Ok("shared_readwrite".to_owned()),
        other => Err(format!(
            "unknown subagent memory_view '{other}'; expected none, shared_readonly, or shared_readwrite"
        )),
    }
}

pub(super) fn normalize_memory_tool(input: &str) -> Result<String, String> {
    match normalize_config_token(input).as_str() {
        "read" | "write" | "forget" => Ok(normalize_config_token(input)),
        other => Err(format!(
            "unknown subagent memory_tools entry '{other}'; expected read, write, or forget"
        )),
    }
}

pub(super) fn subagent_memory_metadata(
    subagent: &SubAgentConfig,
) -> Result<BTreeMap<Arc<str>, Value>, String> {
    let memory_view = normalize_memory_view(&subagent.memory_view)?;
    let mut metadata = BTreeMap::new();
    if memory_view == "none" {
        if !subagent.memory_domains.is_empty() {
            return Err(format!(
                "subagent '{}' sets memory_domains without enabling memory_view",
                subagent.id
            ));
        }
        return Ok(metadata);
    }

    let memory_domains = subagent
        .memory_domains
        .iter()
        .map(|domain| normalize_domain(domain, "subagents.memory_domains"))
        .collect::<Result<Vec<_>, _>>()?;
    let memory_tools = subagent
        .memory_tools
        .iter()
        .map(|tool| normalize_memory_tool(tool))
        .collect::<Result<Vec<_>, _>>()?;

    metadata.insert(Arc::from("memory_view"), Value::String(memory_view));
    metadata.insert(
        Arc::from("memory_default_owner"),
        Value::String("agent".to_owned()),
    );
    if !memory_domains.is_empty() {
        metadata.insert(
            Arc::from("memory_domains"),
            Value::Array(
                memory_domains
                    .iter()
                    .map(|domain| Value::String(domain.to_string()))
                    .collect(),
            ),
        );
    }
    if !memory_tools.is_empty() {
        metadata.insert(
            Arc::from("memory_tools"),
            Value::Array(memory_tools.into_iter().map(Value::String).collect()),
        );
    }
    Ok(metadata)
}
