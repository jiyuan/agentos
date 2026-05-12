use super::{RunnerDeps, RunnerError};
use crate::task_workspace::TaskWorkspace;
use agentos_interfaces::session::Item;
use agentos_interfaces::RunState;
use agentos_proto::{Envelope, TaskId};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(super) struct ActiveTaskSession<'a> {
    workspace: &'a TaskWorkspace,
    task_id: TaskId,
    session_id: Arc<str>,
}

pub(super) fn activate_for_run<'a>(
    state: &mut RunState,
    input: &Envelope,
    deps: &'a RunnerDeps<'_>,
) -> Result<Option<ActiveTaskSession<'a>>, RunnerError> {
    let Some(workspace) = deps.task_workspace else {
        return Ok(None);
    };
    let task_id = task_id_for_run(state, input);
    let session_id = Arc::from(sanitize_file_stem(state.run_id.as_str()));
    workspace.init_task(&task_id)?;
    state.task_id = Some(task_id.clone());
    state.task_session_id = Some(Arc::clone(&session_id));
    let active = ActiveTaskSession {
        workspace,
        task_id,
        session_id,
    };
    append_event(
        &active,
        json!({
            "event": "session_started",
            "phase": "run",
            "run_id": state.run_id.as_str(),
            "active_agent": state.active_agent.as_str(),
            "conversation_id": input.conversation_id.as_str(),
            "channel_id": input.channel_id.as_str(),
        }),
    )?;
    append_event(
        &active,
        json!({
            "event": "input",
            "message": input.message,
            "metadata": input.metadata,
        }),
    )?;
    Ok(Some(active))
}

pub(super) fn activate_for_resume<'a>(
    state: &mut RunState,
    deps: &'a RunnerDeps<'_>,
) -> Result<Option<ActiveTaskSession<'a>>, RunnerError> {
    let Some(workspace) = deps.task_workspace else {
        return Ok(None);
    };
    let task_id = state
        .task_id
        .clone()
        .unwrap_or_else(|| task_id_for_state(state));
    let session_id = Arc::from(format!(
        "{}-resume-{}",
        sanitize_file_stem(state.run_id.as_str()),
        state.trace_events.len()
    ));
    workspace.init_task(&task_id)?;
    state.task_id = Some(task_id.clone());
    state.task_session_id = Some(Arc::clone(&session_id));
    let active = ActiveTaskSession {
        workspace,
        task_id,
        session_id,
    };
    append_event(
        &active,
        json!({
            "event": "session_started",
            "phase": "resume",
            "run_id": state.run_id.as_str(),
            "active_agent": state.active_agent.as_str(),
        }),
    )?;
    Ok(Some(active))
}

pub(super) fn active<'a>(
    state: &RunState,
    deps: &'a RunnerDeps<'_>,
) -> Option<ActiveTaskSession<'a>> {
    let workspace = deps.task_workspace?;
    Some(ActiveTaskSession {
        workspace,
        task_id: state
            .task_id
            .clone()
            .unwrap_or_else(|| task_id_for_state(state)),
        session_id: state
            .task_session_id
            .clone()
            .unwrap_or_else(|| Arc::from(sanitize_file_stem(state.run_id.as_str()))),
    })
}

pub(super) fn persist_items(
    active: Option<&ActiveTaskSession<'_>>,
    phase: &'static str,
    items: &[Item],
) -> Result<(), RunnerError> {
    let Some(active) = active else {
        return Ok(());
    };
    for (index, item) in items.iter().enumerate() {
        append_event(
            active,
            json!({
                "event": "transcript_item",
                "phase": phase,
                "index": index,
                "message": item.message,
                "metadata": item.metadata,
            }),
        )?;
    }
    append_event(
        active,
        json!({
            "event": "session_finished",
            "phase": phase,
        }),
    )
}

fn append_event(active: &ActiveTaskSession<'_>, event: Value) -> Result<(), RunnerError> {
    active
        .workspace
        .append_session_event(&active.task_id, &active.session_id, &event)?;
    Ok(())
}

fn task_id_for_run(state: &RunState, input: &Envelope) -> TaskId {
    input
        .metadata
        .get("task_id")
        .and_then(Value::as_str)
        .map(TaskId::new)
        .unwrap_or_else(|| task_id_for_state(state))
}

pub(super) fn task_id_for_state(state: &RunState) -> TaskId {
    if state.active_agent.as_str() == "general-subagent" {
        TaskId::new("general")
    } else {
        TaskId::new(state.run_id.as_str())
    }
}

pub(super) fn sanitize_file_stem(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' | ':' => ch,
            _ => '_',
        })
        .collect()
}
