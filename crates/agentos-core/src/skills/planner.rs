//! Deterministic skill planner registry.
//!
//! Each enabled workspace skill can optionally ship a Rust planner that
//! short-circuits the LLM round-trip when a deterministic pattern matches
//! (e.g. "summarize the top Hacker News story" → `WebResearchSkill`).
//!
//! Skill planners are kept behind a `SkillPlanner` trait so
//! `MaxOrchestrator::plan_internal` can iterate them without an if-let
//! chain. Adding a new skill no longer requires editing the orchestrator —
//! implement the trait and register the planner in
//! [`builtin_skill_planners`].

use super::{SkillCreatorSkill, WebResearchSkill, WorkspaceSkillCatalog};
use agentos_interfaces::orchestrator::{OrchestratorError, Plan, RunContext};
use std::sync::Arc;

/// A deterministic short-circuit planner attached to a workspace skill.
/// Implementations should return `Ok(None)` whenever the current `ctx`
/// doesn't match the skill's trigger conditions so the orchestrator keeps
/// walking the registry / falls through to the LLM.
pub trait SkillPlanner: Send + Sync {
    /// Workspace skill name (e.g. `"web-research"`). Must match the
    /// directory in `workspace/skills/`.
    fn name(&self) -> &str;

    /// Inspect the run context and return an optional `Plan`.
    fn plan(&self, ctx: &RunContext<'_>) -> Result<Option<Plan>, OrchestratorError>;
}

/// Construct the set of built-in skill planners that should be active for a
/// given catalog. Each planner gates itself on `catalog.contains(name)`, so
/// returning the full list is safe even when a workspace only enables a
/// subset.
///
/// Future skills add a struct + an entry here — no orchestrator edits.
pub fn builtin_skill_planners(catalog: WorkspaceSkillCatalog) -> Vec<Arc<dyn SkillPlanner>> {
    vec![
        Arc::new(WebResearchSkill::new(catalog.clone())),
        Arc::new(SkillCreatorSkill::new(catalog)),
    ]
}
