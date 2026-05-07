use crate::skills::WorkspaceSkillMetadata;
use agentos_interfaces::orchestrator::{
    DispatchPriority, OrchestratorTemplate, ResourceEntry, ResourceIndex, ResourceKind,
    RoutingRule, RoutingTable, TaskDomain,
};
use agentos_interfaces::tool::ToolSpec;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod memory;
mod normalize;
mod orchestrator;
mod subagents;

pub use memory::{
    MemoryConfig, MemoryPolicyConfig, MemoryQdrantConfig, MemoryRetentionConfig,
    MemorySharedDomainConfig, MemorySqliteVecConfig,
};
pub use orchestrator::{RoutingConfig, RoutingRuleConfig, StageConfig, TemplateConfig};
pub use subagents::SubAgentConfig;

use normalize::normalize_domain;
use orchestrator::rule_from_config;
use subagents::{normalize_memory_tool, normalize_memory_view, subagent_memory_metadata};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub agent: AgentConfig,
    pub memory: MemoryConfig,
    pub isolation: IsolationConfig,
    pub subagents: Vec<SubAgentConfig>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub mcp_tools: Vec<McpToolConfig>,
    pub resources: ResourceConfig,
    pub routing: RoutingConfig,
    pub orchestrator_templates: Vec<TemplateConfig>,
    pub task_workspace: TaskWorkspaceConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct AgentConfig {
    pub id: Arc<str>,
    pub orchestrator: Arc<str>,
    pub memory: Arc<str>,
    pub max_turns: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            id: Arc::from("default"),
            orchestrator: Arc::from("builtin.max"),
            memory: Arc::from("memory.in_memory"),
            max_turns: 16,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct IsolationConfig {
    pub worker_path: Option<PathBuf>,
    pub worker_path_env: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct McpServerConfig {
    pub id: Arc<str>,
    pub endpoint: Arc<str>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct McpToolConfig {
    pub server_id: Arc<str>,
    pub name: Arc<str>,
    pub description: Arc<str>,
    pub response: Arc<str>,
    pub requires_isolation: bool,
}

impl Default for McpToolConfig {
    fn default() -> Self {
        Self {
            server_id: Arc::from("static-mcp"),
            name: Arc::from("remote_echo"),
            description: Arc::from("Static MCP-backed tool"),
            response: Arc::from("static MCP response"),
            requires_isolation: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct ResourceConfig {
    pub priority: Vec<Arc<str>>,
    pub skills: ResourceSection,
    pub tools: ResourceSection,
    pub mcp: ResourceSection,
    pub llm: ResourceSection,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            priority: vec![
                Arc::from("skills"),
                Arc::from("tools"),
                Arc::from("mcp"),
                Arc::from("llm"),
            ],
            skills: ResourceSection::default(),
            tools: ResourceSection::default(),
            mcp: ResourceSection::default(),
            llm: ResourceSection::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct ResourceSection {
    pub enabled: Vec<Arc<str>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct TaskWorkspaceConfig {
    pub root: PathBuf,
}

impl Default for TaskWorkspaceConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("tasks"),
        }
    }
}

impl WorkspaceConfig {
    pub fn load(path: &Path) -> Result<Self, std::io::Error> {
        let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let mut config = match std::fs::read_to_string(path) {
            Ok(input) => toml::from_str(&input).map_err(std::io::Error::other)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => return Err(err),
        };
        config.resolve_paths(config_dir);
        config.validate_memory().map_err(std::io::Error::other)?;
        config.validate_subagents().map_err(std::io::Error::other)?;
        config.routing_table().map_err(std::io::Error::other)?;
        Ok(config)
    }

    fn resolve_paths(&mut self, config_dir: &Path) {
        if let Some(path) = &self.memory.path {
            if path.is_relative() {
                self.memory.path = Some(config_dir.join(path));
            }
        }
        if self.task_workspace.root.is_relative() {
            self.task_workspace.root = config_dir.join(&self.task_workspace.root);
        }
        if let Some(worker_path) = &self.isolation.worker_path {
            if worker_path.is_relative() {
                self.isolation.worker_path = Some(config_dir.join(worker_path));
            }
        }
    }

    pub fn validate_memory(&mut self) -> Result<(), String> {
        self.memory.validate()
    }

    pub fn validate_subagents(&mut self) -> Result<(), String> {
        for subagent in &mut self.subagents {
            subagent.memory_view = Arc::from(normalize_memory_view(&subagent.memory_view)?);
            for domain in &mut subagent.memory_domains {
                *domain = normalize_domain(domain, "subagents.memory_domains")?;
            }
            for tool in &mut subagent.memory_tools {
                *tool = Arc::from(normalize_memory_tool(tool)?);
            }
            if subagent.memory_view.as_ref() == "none" && !subagent.memory_domains.is_empty() {
                return Err(format!(
                    "subagent '{}' sets memory_domains without enabling memory_view",
                    subagent.id
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn subagent_metadata(
        &self,
        agent_id: &Arc<str>,
        policy_id: &Arc<str>,
    ) -> Result<BTreeMap<Arc<str>, serde_json::Value>, String> {
        let subagent = self
            .subagents
            .iter()
            .find(|subagent| subagent.id == *agent_id && subagent.policy_id == *policy_id)
            .ok_or_else(|| {
                format!(
                    "unknown subagent '{}' with policy '{}'",
                    agent_id, policy_id
                )
            })?;
        subagent_memory_metadata(subagent)
    }

    pub fn resource_index(
        &self,
        tool_specs: &[ToolSpec],
        mcp_specs: &[ToolSpec],
        skill_specs: &[WorkspaceSkillMetadata],
    ) -> ResourceIndex {
        let mcp_names = mcp_specs
            .iter()
            .map(|spec| Arc::clone(&spec.name))
            .collect::<BTreeSet<_>>();
        let tools = tool_specs
            .iter()
            .filter(|spec| !mcp_names.contains(&spec.name))
            .collect::<Vec<_>>();
        let mcp = mcp_specs.iter().collect::<Vec<_>>();
        let mut entries = Vec::new();

        for priority in &self.resources.priority {
            match priority.as_ref() {
                "skills" => {
                    for skill in skill_specs {
                        entries.push(ResourceEntry {
                            name: Arc::clone(&skill.name),
                            kind: ResourceKind::Skill,
                            summary: Arc::clone(&skill.description),
                            priority: DispatchPriority::Skill,
                        });
                    }
                }
                "tools" => {
                    for tool in &tools {
                        entries.push(ResourceEntry {
                            name: Arc::clone(&tool.name),
                            kind: ResourceKind::Tool,
                            summary: Arc::clone(&tool.description),
                            priority: DispatchPriority::ToolOrMcp,
                        });
                    }
                }
                "mcp" => {
                    for tool in &mcp {
                        entries.push(ResourceEntry {
                            name: Arc::clone(&tool.name),
                            kind: ResourceKind::Mcp,
                            summary: Arc::clone(&tool.description),
                            priority: DispatchPriority::ToolOrMcp,
                        });
                    }
                }
                "llm" => {
                    if self.resources.llm.enabled.is_empty() {
                        entries.push(ResourceEntry {
                            name: Arc::from("llm"),
                            kind: ResourceKind::Llm,
                            summary: Arc::from("Fallback language model reasoning"),
                            priority: DispatchPriority::LlmFallback,
                        });
                    } else {
                        for llm in &self.resources.llm.enabled {
                            entries.push(ResourceEntry {
                                name: Arc::clone(llm),
                                kind: ResourceKind::Llm,
                                summary: Arc::from("Configured language model fallback"),
                                priority: DispatchPriority::LlmFallback,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        ResourceIndex { entries }.sorted()
    }

    pub fn routing_table(&self) -> Result<RoutingTable, String> {
        self.validate_template_references()?;
        let templates = self.template_map()?;
        let rules = self
            .routing
            .rules
            .iter()
            .map(|rule| rule_from_config(rule, &templates, self))
            .collect::<Result<Vec<_>, _>>()?;
        let fallback = self.routing.fallback.as_ref().map_or_else(
            || -> Result<RoutingRule, String> {
                Ok(RoutingRule {
                    domain: TaskDomain::General,
                    description: Arc::from("General-purpose fallback for unclassified prompts."),
                    examples: Vec::new(),
                    dispatch: agentos_interfaces::orchestrator::DispatchTarget::Direct,
                })
            },
            |rule| rule_from_config(rule, &templates, self),
        )?;
        Ok(RoutingTable { rules, fallback })
    }

    fn template_map(&self) -> Result<BTreeMap<Arc<str>, OrchestratorTemplate>, String> {
        self.orchestrator_templates
            .iter()
            .map(|template| {
                template
                    .to_template(self)
                    .map(|value| (Arc::clone(&template.name), value))
            })
            .collect()
    }

    fn validate_template_references(&self) -> Result<(), String> {
        for template in &self.orchestrator_templates {
            for stage in &template.stages {
                if !self.subagents.iter().any(|subagent| {
                    subagent.id == stage.agent_id && subagent.policy_id == stage.policy_id
                }) {
                    return Err(format!(
                        "template '{}' stage '{}' references unknown subagent '{}' with policy '{}'",
                        template.name, stage.name, stage.agent_id, stage.policy_id
                    ));
                }
            }
        }
        Ok(())
    }
}
