use crate::subagents::SubAgentRunOutput;
use agentos_interfaces::orchestrator::SubOrchSpec;
use agentos_interfaces::session::Item;
use agentos_proto::{Message, MessageRole, ToolCall, ToolResult, ToolStatus};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) fn tool_result_item(result: ToolResult) -> Item {
    let mut metadata = result.metadata;
    metadata.insert(
        Arc::from("tool_call_id"),
        metadata_value(result.call_id.as_str()),
    );
    metadata.insert(
        Arc::from("tool_status"),
        metadata_value(tool_status_name(&result.status)),
    );
    Item {
        message: Message {
            role: MessageRole::Tool,
            content: result.content,
            attachments: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: Some(result.call_id),
            metadata,
        },
        metadata: BTreeMap::new(),
    }
}

pub(super) fn assistant_tool_call_item(call: &ToolCall) -> Item {
    let mut metadata = BTreeMap::new();
    metadata.insert(Arc::from("kind"), metadata_value("tool_call"));
    metadata.insert(Arc::from("tool_call_id"), metadata_value(call.id.as_str()));
    metadata.insert(Arc::from("tool_name"), metadata_value(call.name.as_ref()));
    Item {
        message: Message {
            role: MessageRole::Assistant,
            content: Arc::from(""),
            attachments: Vec::new(),
            tool_calls: vec![call.clone()],
            tool_call_id: None,
            metadata: BTreeMap::new(),
        },
        metadata,
    }
}

pub(super) fn subagent_result_item(result: SubAgentRunOutput) -> Item {
    let mut metadata = BTreeMap::new();
    metadata.insert(Arc::from("kind"), metadata_value("subagent_result"));
    metadata.insert(
        Arc::from("subagent_id"),
        metadata_value(result.agent_id.as_str()),
    );
    metadata.insert(
        Arc::from("policy_id"),
        metadata_value(result.policy_id.as_ref()),
    );
    metadata.insert(
        Arc::from("child_run_id"),
        metadata_value(result.state.run_id.as_str()),
    );
    Item {
        message: Message {
            role: MessageRole::Tool,
            content: result.message.content,
            attachments: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            metadata,
        },
        metadata: BTreeMap::new(),
    }
}

pub(super) fn suborchestrator_result_item(
    spec: &SubOrchSpec,
    results: Vec<(Arc<str>, SubAgentRunOutput)>,
) -> Item {
    let mut metadata = BTreeMap::new();
    metadata.insert(Arc::from("kind"), metadata_value("suborchestrator_result"));
    metadata.insert(
        Arc::from("template"),
        metadata_value(spec.template.name.as_ref()),
    );
    metadata.insert(Arc::from("task_id"), metadata_value(spec.task_id.as_str()));
    metadata.insert(Arc::from("stages"), Value::from(results.len()));
    let content = if results.is_empty() {
        format!(
            "sub-orchestrator '{}' completed with no stages",
            spec.template.name
        )
    } else {
        results
            .iter()
            .map(|(stage, result)| format!("{}: {}", stage, result.message.content))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Item {
        message: Message {
            role: MessageRole::Tool,
            content: Arc::from(content),
            attachments: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            metadata,
        },
        metadata: BTreeMap::new(),
    }
}

pub(super) fn tool_status_name(status: &ToolStatus) -> &'static str {
    match status {
        ToolStatus::Succeeded => "succeeded",
        ToolStatus::Failed => "failed",
        ToolStatus::Denied => "denied",
    }
}

/// Build a `Value::String` from any string-like input.
///
/// Centralised so the (unavoidable, given `serde_json::Value`'s contract)
/// `&str → String` allocation lives in one place.
pub(super) fn metadata_value(s: impl Into<String>) -> Value {
    Value::String(s.into())
}
