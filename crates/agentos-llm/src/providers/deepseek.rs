use crate::providers::content::append_descriptors;
use crate::providers::{format_provider_error, post_json};
use agentos_interfaces::tool::ToolSpec;
use agentos_proto::{Message, MessageRole, ToolCall, ToolCallId};
use serde_json::{json, value::RawValue, Value};
use std::env;
use std::sync::Arc;

pub async fn complete(
    model: &str,
    messages: &[Message],
    tools: &[ToolSpec],
) -> Result<Message, String> {
    let api_key =
        env::var("DEEPSEEK_API_KEY").map_err(|_| "missing DEEPSEEK_API_KEY".to_owned())?;
    let base_url = env::var("AGENTOS_DEEPSEEK_BASE_URL")
        .or_else(|_| env::var("DEEPSEEK_BASE_URL"))
        .or_else(|_| env::var("DEEPSEEK_HOST"))
        .unwrap_or_else(|_| "https://api.deepseek.com".to_owned());
    let serialized = messages.iter().map(flat_message).collect::<Vec<_>>();
    let mut payload = json!({
        "model": model,
        "messages": serialized,
        "stream": false
    });
    if !tools.is_empty() {
        // DeepSeek follows OpenAI's Chat Completions shape for function tools.
        payload["tools"] = json!(tools.iter().map(tool_to_function).collect::<Vec<_>>());
    }
    let response = post_json(
        "llm",
        &format!("{}/chat/completions", base_url.trim_end_matches('/')),
        &[
            ("Authorization", format!("Bearer {api_key}")),
            ("Content-Type", "application/json".to_owned()),
        ],
        &payload,
    )
    .await?;
    if let Some(error) = response.body.get("error") {
        return Err(format_provider_error("DeepSeek", &response, error));
    }
    let message = response
        .body
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| {
            format!(
                "DeepSeek response missing assistant message: {}",
                response.body
            )
        })?;
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let tool_calls = parse_tool_calls(message);
    Ok(Message {
        role: MessageRole::Assistant,
        content: Arc::from(content),
        attachments: Vec::new(),
        tool_calls,
        tool_call_id: None,
        metadata: Default::default(),
    })
}

fn tool_to_function(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name.as_ref(),
            "description": spec.description.as_ref(),
            "parameters": spec.input_schema,
        }
    })
}

fn parse_tool_calls(message: &Value) -> Vec<ToolCall> {
    let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return Vec::new();
    };
    calls
        .iter()
        .filter_map(|call| {
            let id = call.get("id").and_then(Value::as_str)?;
            let function = call.get("function")?;
            let name = function.get("name").and_then(Value::as_str)?;
            let args_str = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let args = RawValue::from_string(args_str.to_owned()).ok()?;
            Some(ToolCall {
                id: ToolCallId::new(id),
                name: Arc::from(name),
                args,
            })
        })
        .collect()
}

fn flat_message(message: &Message) -> Value {
    // Tool-role: emit as OpenAI-compatible tool message linked to the call id.
    if message.role == MessageRole::Tool {
        let tool_call_id = message
            .tool_call_id
            .as_ref()
            .map(|id| id.as_str().to_owned())
            .unwrap_or_default();
        return json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": message.content.as_ref(),
        });
    }
    // Assistant turn that requested tools: include tool_calls.
    if message.role == MessageRole::Assistant && !message.tool_calls.is_empty() {
        let calls = message
            .tool_calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id.as_str(),
                    "type": "function",
                    "function": {
                        "name": call.name.as_ref(),
                        "arguments": call.args.get(),
                    }
                })
            })
            .collect::<Vec<_>>();
        let content = if message.content.is_empty() {
            Value::Null
        } else {
            Value::String(message.content.to_string())
        };
        return json!({
            "role": "assistant",
            "content": content,
            "tool_calls": calls,
        });
    }
    let role = match message.role {
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Tool => unreachable!("tool handled above"),
    };
    let base = message.content.to_string();
    let content = append_descriptors(&base, &message.attachments);
    json!({ "role": role, "content": content })
}
