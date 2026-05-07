use super::subagents::SubAgentConfig;
use super::WorkspaceConfig;
use agentos_interfaces::orchestrator::{
    DispatchTarget, OrchestratorTemplate, RoutingRule, Stage, SubAgentSpec, TaskDomain,
};
use agentos_proto::AgentId;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct RoutingConfig {
    pub rules: Vec<RoutingRuleConfig>,
    pub fallback: Option<RoutingRuleConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct RoutingRuleConfig {
    pub domain: Arc<str>,
    pub description: Arc<str>,
    pub examples: Vec<Arc<str>>,
    pub dispatch: Arc<str>,
    pub template: Option<Arc<str>>,
    pub agent_id: Option<Arc<str>>,
    pub policy_id: Option<Arc<str>>,
}

impl Default for RoutingRuleConfig {
    fn default() -> Self {
        Self {
            domain: Arc::from("general"),
            description: Arc::from(""),
            examples: Vec::new(),
            dispatch: Arc::from("direct"),
            template: None,
            agent_id: None,
            policy_id: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct TemplateConfig {
    pub name: Arc<str>,
    pub description: Arc<str>,
    pub developer_instructions: Arc<str>,
    pub stages: Vec<StageConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct StageConfig {
    pub name: Arc<str>,
    pub agent_id: Arc<str>,
    pub policy_id: Arc<str>,
    pub depends_on: Vec<Arc<str>>,
}

impl Default for StageConfig {
    fn default() -> Self {
        Self {
            name: Arc::from("stage"),
            agent_id: Arc::from("general-subagent"),
            policy_id: Arc::from("default"),
            depends_on: Vec::new(),
        }
    }
}

impl TemplateConfig {
    pub(super) fn to_template(
        &self,
        config: &WorkspaceConfig,
    ) -> Result<OrchestratorTemplate, String> {
        Ok(OrchestratorTemplate {
            name: Arc::clone(&self.name),
            stages: self
                .stages
                .iter()
                .map(|stage| {
                    Ok(Stage {
                        name: Arc::clone(&stage.name),
                        agent: SubAgentSpec {
                            agent_id: AgentId::new(Arc::clone(&stage.agent_id)),
                            policy_id: Arc::clone(&stage.policy_id),
                            metadata: config
                                .subagent_metadata(&stage.agent_id, &stage.policy_id)?,
                        },
                        depends_on: stage.depends_on.clone(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        })
    }
}

pub(super) fn parse_domain(input: &str) -> TaskDomain {
    match input {
        "software_dev" | "software-dev" | "software" => TaskDomain::SoftwareDev,
        "content_ops" | "content-ops" | "content" => TaskDomain::ContentOps,
        "research" => TaskDomain::Research,
        "editing" | "edit" => TaskDomain::Editing,
        "general" => TaskDomain::General,
        other => TaskDomain::Custom(Arc::from(other)),
    }
}

pub(super) fn parse_dispatch(
    rule: &RoutingRuleConfig,
    templates: &BTreeMap<Arc<str>, OrchestratorTemplate>,
    config: &WorkspaceConfig,
) -> Result<DispatchTarget, String> {
    match rule.dispatch.as_ref() {
        "escalate" => {
            let template_name = rule.template.as_ref().ok_or_else(|| {
                format!(
                    "routing domain '{}' uses escalate without template",
                    rule.domain
                )
            })?;
            let template = templates.get(template_name).cloned().ok_or_else(|| {
                format!(
                    "routing domain '{}' references unknown template '{}'",
                    rule.domain, template_name
                )
            })?;
            Ok(DispatchTarget::Escalate(template))
        }
        "delegate" => {
            let agent_id = rule.agent_id.as_ref().ok_or_else(|| {
                format!(
                    "routing domain '{}' uses delegate without agent_id",
                    rule.domain
                )
            })?;
            let policy_id = rule.policy_id.as_ref().ok_or_else(|| {
                format!(
                    "routing domain '{}' uses delegate without policy_id",
                    rule.domain
                )
            })?;
            if !config.subagents.iter().any(|subagent: &SubAgentConfig| {
                subagent.id == *agent_id && subagent.policy_id == *policy_id
            }) {
                return Err(format!(
                    "routing domain '{}' delegates to unknown subagent '{}' with policy '{}'",
                    rule.domain, agent_id, policy_id
                ));
            }
            Ok(DispatchTarget::Delegate(SubAgentSpec {
                agent_id: AgentId::new(Arc::clone(agent_id)),
                policy_id: Arc::clone(policy_id),
                metadata: config.subagent_metadata(agent_id, policy_id)?,
            }))
        }
        "direct" => Ok(DispatchTarget::Direct),
        other => Err(format!(
            "routing domain '{}' has unknown dispatch '{}'",
            rule.domain, other
        )),
    }
}

pub(super) fn rule_from_config(
    rule: &RoutingRuleConfig,
    templates: &BTreeMap<Arc<str>, OrchestratorTemplate>,
    config: &WorkspaceConfig,
) -> Result<RoutingRule, String> {
    Ok(RoutingRule {
        domain: parse_domain(&rule.domain),
        description: Arc::clone(&rule.description),
        examples: rule.examples.clone(),
        dispatch: parse_dispatch(rule, templates, config)?,
    })
}
