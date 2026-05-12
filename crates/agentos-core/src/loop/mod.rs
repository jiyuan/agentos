use crate::approve::Policy;
use crate::hooks::Hooks;
use crate::subagents::{SubAgentError, SubAgentRegistry};
use crate::task_workspace::{TaskWorkspace, TaskWorkspaceError};
use crate::tools::{ToolRegistry, ToolRegistryError};
use crate::trace;
use agentos_interfaces::guardrail::{
    GuardrailError, GuardrailOutcome, Input, InputGuardrail, OutputGuardrail, ToolGuardrail,
};
use agentos_interfaces::orchestrator::{Orchestrator, Plan, RunContext};
use agentos_interfaces::run_state::{InterruptionAction, RunState};
use agentos_proto::{AgentId, Message, SpanKind, ToolCall, ToolResult, ToolStatus};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;
use tracing::info;

mod approval;
mod delegate;
mod escalate;
mod items;
mod telemetry;

use approval::{approve_transition, ApproveTransition};
use delegate::execute_delegate;
use escalate::execute_escalate;
use items::{
    assistant_tool_call_item, metadata_value, subagent_result_item, suborchestrator_result_item,
    tool_result_item, tool_status_name,
};
use telemetry::{plan_assignment_fields, record_telemetry_event};

#[derive(Debug, Error)]
pub enum RunError {
    #[error("run has already finished")]
    AlreadyDone,
    #[error("paused run must be resumed through the approval path")]
    NotResumable,
    #[error("maximum turn count exceeded")]
    MaxTurnsExceeded,
    #[error("orchestrator failed: {0}")]
    Orchestrator(#[from] agentos_interfaces::orchestrator::OrchestratorError),
    #[error("guardrail backend failed: {0}")]
    Guardrail(#[from] GuardrailError),
    #[error("guardrail '{guardrail}' tripped: {reason}")]
    GuardrailTripped {
        guardrail: Arc<str>,
        reason: Arc<str>,
    },
    #[error("tool execution failed: {0}")]
    Tool(#[from] ToolRegistryError),
    #[error("sub-agent execution failed: {0}")]
    SubAgent(#[from] SubAgentError),
    #[error("task workspace failed: {0}")]
    TaskWorkspace(#[from] TaskWorkspaceError),
    #[error("approval denied: {reason}")]
    ApprovalDenied { reason: Arc<str> },
    #[error("approval cannot pause this action yet: {reason}")]
    ApprovalUnsupported { reason: Arc<str> },
}

pub struct LoopDeps<'a> {
    pub orchestrator: &'a dyn Orchestrator,
    pub max_turns: usize,
    pub hooks: Option<&'a Hooks>,
    pub tools: Option<&'a ToolRegistry>,
    pub task_workspace: Option<&'a TaskWorkspace>,
    pub policy: &'a Policy,
    pub subagents: Option<&'a SubAgentRegistry>,
    pub input_guardrails: &'a [InputGuardrailEntry<'a>],
    pub output_guardrails: &'a [OutputGuardrailEntry<'a>],
    pub tool_guardrails: &'a [ToolGuardrailEntry<'a>],
}

pub struct InputGuardrailEntry<'a> {
    pub name: Arc<str>,
    pub guardrail: &'a dyn InputGuardrail,
}

pub struct OutputGuardrailEntry<'a> {
    pub name: Arc<str>,
    pub guardrail: &'a dyn OutputGuardrail,
}

pub struct ToolGuardrailEntry<'a> {
    pub name: Arc<str>,
    pub guardrail: &'a dyn ToolGuardrail,
}

#[derive(Debug)]
pub enum RunLoopState {
    Start(StartCtx),
    Plan(PlanCtx),
    Approve(ApproveCtx),
    Act(ActCtx),
    Observe(ObserveCtx),
    Paused(RunState),
    Finish(FinalOutput),
}

impl RunLoopState {
    pub async fn step(self, deps: &LoopDeps<'_>) -> Result<Self, RunError> {
        match self {
            Self::Start(ctx) => start(ctx, deps).await,
            Self::Plan(ctx) => plan(ctx, deps).await,
            Self::Approve(ctx) => approve(ctx, deps).await,
            Self::Act(ctx) => act(ctx, deps).await,
            Self::Observe(ctx) => observe(ctx).await,
            Self::Paused(_) => Err(RunError::NotResumable),
            Self::Finish(_) => Err(RunError::AlreadyDone),
        }
    }
}

pub fn resume_approved(state: RunState) -> Result<RunLoopState, RunError> {
    let mut state = state;
    if let Some(reason) = state.take_rejected_reason() {
        return Err(RunError::ApprovalDenied { reason });
    }

    let turns = resume_turns(&state);
    let Some(action) = state.take_approved_action() else {
        return Err(RunError::NotResumable);
    };
    let plan = match action {
        InterruptionAction::ToolCall(call) => Plan::CallTool(call),
        InterruptionAction::Delegate(spec) => Plan::Delegate(spec),
        InterruptionAction::Escalate(spec) => Plan::Escalate(spec),
        InterruptionAction::Handoff { agent_id, payload } => Plan::Handoff(agent_id, payload),
    };
    Ok(RunLoopState::Act(ActCtx { state, plan, turns }))
}

#[derive(Debug)]
pub struct StartCtx {
    pub state: RunState,
}

#[derive(Debug)]
pub struct PlanCtx {
    pub state: RunState,
    pub turns: usize,
}

#[derive(Debug)]
pub struct ApproveCtx {
    pub state: RunState,
    pub plan: Plan,
    pub turns: usize,
}

#[derive(Debug)]
pub struct ActCtx {
    pub state: RunState,
    pub plan: Plan,
    pub turns: usize,
}

#[derive(Debug)]
pub struct ObserveCtx {
    pub state: RunState,
    pub turns: usize,
}

#[derive(Debug)]
pub struct FinalOutput {
    pub state: RunState,
    pub message: Message,
}

async fn start(ctx: StartCtx, deps: &LoopDeps<'_>) -> Result<RunLoopState, RunError> {
    info!(
        run_id = ctx.state.run_id.as_str(),
        active_agent = ctx.state.active_agent.as_str(),
        "run_loop_start"
    );
    if let Some(item) = ctx.state.transcript.items.last() {
        let run_ctx = RunContext::from_state(&ctx.state);
        let input = Input {
            message: item.message.clone(),
        };
        for entry in deps.input_guardrails {
            let outcome = entry.guardrail.check(&input, &run_ctx).await?;
            ensure_guardrail_passed(&entry.name, outcome)?;
        }
    }
    Ok(RunLoopState::Plan(PlanCtx {
        state: ctx.state,
        turns: 0,
    }))
}

async fn plan(ctx: PlanCtx, deps: &LoopDeps<'_>) -> Result<RunLoopState, RunError> {
    if ctx.turns >= deps.max_turns {
        return Err(RunError::MaxTurnsExceeded);
    }

    let mut state = ctx.state;
    let mut fields = BTreeMap::new();
    fields.insert(Arc::from("turn"), Value::from(ctx.turns));
    let parent_id = trace::run_span_id(&state);
    let plan_span_id = trace::record_span(&mut state, parent_id, SpanKind::State, "plan", fields);
    trace::record_event(
        &mut state,
        deps.hooks,
        plan_span_id.clone(),
        "plan_started",
        BTreeMap::new(),
    );

    let hydrate_span_id = trace::record_span(
        &mut state,
        Some(plan_span_id.clone()),
        SpanKind::State,
        "orchestrator.hydrate",
        BTreeMap::new(),
    );
    trace::record_event(
        &mut state,
        deps.hooks,
        hydrate_span_id.clone(),
        "hydrate_started",
        BTreeMap::new(),
    );
    let mut run_ctx = RunContext::from_state(&state);
    deps.orchestrator.hydrate(&mut run_ctx).await?;
    let mut hydrate_fields = BTreeMap::new();
    hydrate_fields.insert(
        Arc::from("memory_fragments"),
        Value::from(run_ctx.memory_fragments.len()),
    );
    hydrate_fields.insert(
        Arc::from("resources"),
        Value::from(run_ctx.resource_index.entries.len()),
    );
    for key in [
        "memory_hydration_candidate_count",
        "memory_hydration_selected_count",
        "memory_hydration_namespace_count",
    ] {
        if let Some(value) = run_ctx.system.metadata.get(key) {
            hydrate_fields.insert(Arc::from(key), value.clone());
        }
    }
    let plan = deps.orchestrator.plan(&run_ctx).await?;
    drop(run_ctx);
    trace::record_event(
        &mut state,
        deps.hooks,
        hydrate_span_id,
        "hydrate_finished",
        hydrate_fields,
    );
    trace::record_span(
        &mut state,
        Some(plan_span_id.clone()),
        SpanKind::Llm,
        "orchestrator.plan",
        BTreeMap::new(),
    );
    let assignment_fields = plan_assignment_fields(&state, &plan);
    record_telemetry_event(
        &mut state,
        deps.hooks,
        plan_span_id.clone(),
        "orchestrator_task_assigned",
        assignment_fields,
    );
    trace::record_event(
        &mut state,
        deps.hooks,
        plan_span_id,
        "plan_finished",
        BTreeMap::new(),
    );
    info!(
        run_id = state.run_id.as_str(),
        active_agent = state.active_agent.as_str(),
        turn = ctx.turns,
        "plan_finished"
    );

    match plan {
        Plan::Reply(message) => {
            let run_ctx = RunContext::from_state(&state);
            for entry in deps.output_guardrails {
                let outcome = entry.guardrail.check(&message, &run_ctx).await?;
                ensure_guardrail_passed(&entry.name, outcome)?;
            }
            Ok(RunLoopState::Finish(FinalOutput { state, message }))
        }
        plan => Ok(RunLoopState::Approve(ApproveCtx {
            state,
            plan,
            turns: ctx.turns,
        })),
    }
}

async fn approve(ctx: ApproveCtx, deps: &LoopDeps<'_>) -> Result<RunLoopState, RunError> {
    match approve_transition(ctx, deps.policy) {
        ApproveTransition::Allow { state, plan, turns } => {
            Ok(RunLoopState::Act(ActCtx { state, plan, turns }))
        }
        ApproveTransition::Deny { reason } => Err(RunError::ApprovalDenied { reason }),
        ApproveTransition::Pause { state } => Ok(RunLoopState::Paused(state)),
        ApproveTransition::Unsupported { reason } => Err(RunError::ApprovalUnsupported { reason }),
    }
}

async fn act(ctx: ActCtx, deps: &LoopDeps<'_>) -> Result<RunLoopState, RunError> {
    let mut state = ctx.state;
    match ctx.plan {
        Plan::CallTool(call) => {
            // Record the assistant turn that requested the tool *before*
            // executing it. OpenAI/Anthropic/DeepSeek all 400 if a tool result
            // arrives without a preceding assistant turn carrying that
            // tool_call's id.
            state.transcript.items.push(assistant_tool_call_item(&call));
            let result = execute_tool(&mut state, deps, call).await?;
            state.transcript.items.push(tool_result_item(result));
        }
        Plan::Delegate(spec) => {
            let result = execute_delegate(&mut state, deps, &spec).await?;
            state.transcript.items.push(subagent_result_item(result));
        }
        Plan::Escalate(spec) => {
            let result = execute_escalate(&mut state, deps, &spec).await?;
            state
                .transcript
                .items
                .push(suborchestrator_result_item(&spec, result));
        }
        Plan::Handoff(agent_id, payload) => {
            execute_handoff(&mut state, deps, agent_id, payload);
        }
        Plan::Reply(_) => {}
    }

    Ok(RunLoopState::Observe(ObserveCtx {
        state,
        turns: ctx.turns + 1,
    }))
}

fn execute_handoff(
    state: &mut RunState,
    deps: &LoopDeps<'_>,
    agent_id: AgentId,
    payload: Option<Value>,
) {
    let from_agent = state.active_agent.clone();
    let parent_id = trace::run_span_id(state);
    let mut fields = BTreeMap::new();
    fields.insert(Arc::from("from_agent"), metadata_value(from_agent.as_str()));
    fields.insert(Arc::from("to_agent"), metadata_value(agent_id.as_str()));
    if let Some(payload) = payload {
        fields.insert(Arc::from("payload"), payload);
    }
    let span_id = trace::record_span(
        state,
        parent_id,
        SpanKind::Handoff,
        format!("handoff.{}", agent_id.as_str()),
        fields,
    );
    trace::record_event(
        state,
        deps.hooks,
        span_id.clone(),
        "handoff_started",
        BTreeMap::new(),
    );

    state.active_agent = agent_id;

    let mut fields = BTreeMap::new();
    fields.insert(
        Arc::from("active_agent"),
        metadata_value(state.active_agent.as_str()),
    );
    trace::record_event(state, deps.hooks, span_id, "handoff_finished", fields);
}

async fn observe(ctx: ObserveCtx) -> Result<RunLoopState, RunError> {
    Ok(RunLoopState::Plan(PlanCtx {
        state: ctx.state,
        turns: ctx.turns,
    }))
}

async fn execute_tool(
    state: &mut RunState,
    deps: &LoopDeps<'_>,
    call: ToolCall,
) -> Result<ToolResult, RunError> {
    let parent_id = trace::run_span_id(state);
    let mut fields = BTreeMap::new();
    fields.insert(Arc::from("tool_name"), metadata_value(call.name.as_ref()));
    fields.insert(Arc::from("tool_call_id"), metadata_value(call.id.as_str()));
    let tool_span_id = trace::record_span(
        state,
        parent_id,
        SpanKind::Tool,
        format!("tool.{}", call.name),
        fields,
    );
    trace::record_event(
        state,
        deps.hooks,
        tool_span_id.clone(),
        "tool_started",
        BTreeMap::new(),
    );

    {
        let run_ctx = RunContext::from_state(state);
        for entry in deps.tool_guardrails {
            let outcome = entry.guardrail.check_call(&call, &run_ctx).await?;
            ensure_guardrail_passed(&entry.name, outcome)?;
        }
    }

    let tools = deps
        .tools
        .ok_or_else(|| ToolRegistryError::UnknownTool(Arc::clone(&call.name)))?;
    // Tool failures (bad path, missing file, malformed args) become a Failed
    // `ToolResult` rather than aborting the run, so the model can read the
    // error in the next turn and self-correct (e.g. create the missing dir
    // and retry). Unknown-tool / isolation errors still bubble up — those
    // indicate a misconfigured runtime, not a recoverable model mistake.
    let result = {
        let run_ctx = RunContext::from_state(state);
        match tools.call_with_context(&call, &run_ctx).await {
            Ok(result) => result,
            Err(ToolRegistryError::Tool(tool_err)) => ToolResult {
                call_id: call.id.clone(),
                status: ToolStatus::Failed,
                content: Arc::from(tool_err.to_string()),
                metadata: BTreeMap::new(),
            },
            Err(other) => return Err(other.into()),
        }
    };

    {
        let run_ctx = RunContext::from_state(state);
        for entry in deps.tool_guardrails {
            let outcome = entry.guardrail.check_result(&result, &run_ctx).await?;
            ensure_guardrail_passed(&entry.name, outcome)?;
        }
    }

    let mut fields = BTreeMap::new();
    fields.insert(
        Arc::from("status"),
        metadata_value(tool_status_name(&result.status)),
    );
    trace::record_event(state, deps.hooks, tool_span_id, "tool_finished", fields);
    Ok(result)
}

fn ensure_guardrail_passed(name: &Arc<str>, outcome: GuardrailOutcome) -> Result<(), RunError> {
    match outcome {
        GuardrailOutcome::Passed => Ok(()),
        GuardrailOutcome::Tripped(reason) => Err(RunError::GuardrailTripped {
            guardrail: Arc::clone(name),
            reason,
        }),
    }
}

fn resume_turns(state: &RunState) -> usize {
    state
        .trace_spans
        .iter()
        .rev()
        .find(|span| span.kind == SpanKind::State && span.name.as_ref() == "plan")
        .and_then(|span| span.fields.get("turn"))
        .and_then(Value::as_u64)
        .map_or(0, |turn| turn as usize)
}
