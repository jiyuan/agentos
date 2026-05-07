use agentos_proto::{ChannelId, ConversationId, Envelope, Message, MessageRole};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;

pub(super) fn envelope_from_event(
    payload: &Value,
    channel_id: &ChannelId,
    allowed_source_ids: &[Arc<str>],
) -> Option<Envelope> {
    let event_type = payload
        .get("header")
        .and_then(|header| header.get("event_type"))
        .and_then(Value::as_str);
    if event_type.is_some_and(|value| value != "im.message.receive_v1") {
        return None;
    }

    let event = payload.get("event")?;
    let message = event.get("message")?;
    if message.get("message_type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    let chat_id = message.get("chat_id").and_then(Value::as_str)?;
    let sender_id = event
        .get("sender")
        .and_then(|sender| sender.get("sender_id"));
    if !feishu_allowed_source_matches(allowed_source_ids, sender_id) {
        return None;
    }
    let text_content = text_content(message.get("content").and_then(Value::as_str)?)?;
    let text = text_content.trim();
    if text.is_empty() {
        return None;
    }

    let sender = sender_id
        .and_then(preferred_feishu_sender_id)
        .map_or_else(|| Arc::from("feishu-user"), Arc::from);
    let mut metadata = BTreeMap::new();
    metadata.insert(Arc::from("kind"), Value::String("feishu".to_owned()));
    if let Some(event_id) = payload
        .get("header")
        .and_then(|header| header.get("event_id"))
        .and_then(Value::as_str)
    {
        metadata.insert(Arc::from("event_id"), Value::String(event_id.to_owned()));
    }
    if let Some(tenant_key) = payload
        .get("header")
        .and_then(|header| header.get("tenant_key"))
        .and_then(Value::as_str)
    {
        metadata.insert(
            Arc::from("tenant_key"),
            Value::String(tenant_key.to_owned()),
        );
    }
    if let Some(message_id) = message.get("message_id").and_then(Value::as_str) {
        metadata.insert(
            Arc::from("message_id"),
            Value::String(message_id.to_owned()),
        );
    }
    if let Some(chat_type) = message.get("chat_type").and_then(Value::as_str) {
        metadata.insert(Arc::from("chat_type"), Value::String(chat_type.to_owned()));
    }

    Some(Envelope {
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(chat_id),
        sender,
        message: Message::text(MessageRole::User, text),
        metadata,
    })
}

fn text_content(content: &str) -> Option<String> {
    serde_json::from_str::<Value>(content)
        .ok()?
        .get("text")?
        .as_str()
        .map(ToOwned::to_owned)
}

pub(super) fn feishu_drop_reason(
    payload: &Value,
    allowed_source_ids: &[Arc<str>],
) -> Option<String> {
    let event_type = payload
        .get("header")
        .and_then(|header| header.get("event_type"))
        .and_then(Value::as_str);
    if event_type.is_some_and(|value| value != "im.message.receive_v1") {
        return Some(format!(
            "unsupported event_type={}, expected im.message.receive_v1",
            event_type.unwrap_or("<missing>")
        ));
    }

    let Some(event) = payload.get("event") else {
        return Some("missing event body".to_owned());
    };
    let Some(message) = event.get("message") else {
        return Some("missing event.message".to_owned());
    };
    let message_type = message.get("message_type").and_then(Value::as_str);
    if message_type != Some("text") {
        return Some(format!(
            "unsupported message_type={}, expected text",
            message_type.unwrap_or("<missing>")
        ));
    }
    let Some(chat_id) = message.get("chat_id").and_then(Value::as_str) else {
        return Some("missing message.chat_id".to_owned());
    };
    let sender_id = event
        .get("sender")
        .and_then(|sender| sender.get("sender_id"));
    if !feishu_allowed_source_matches(allowed_source_ids, sender_id) {
        return Some(format!(
            "filtered by allowed sender ids: allowed={}, chat_id={}, sender_ids={}",
            feishu_allowed_ids_summary(allowed_source_ids),
            chat_id,
            feishu_sender_ids_summary(sender_id)
        ));
    }
    let Some(raw_content) = message.get("content").and_then(Value::as_str) else {
        return Some("missing message.content".to_owned());
    };
    let Some(text) = text_content(raw_content) else {
        return Some("message.content is not a text json payload".to_owned());
    };
    if text.trim().is_empty() {
        return Some("message text is empty after trimming".to_owned());
    }
    None
}

fn feishu_allowed_source_matches(
    allowed_source_ids: &[Arc<str>],
    sender_id: Option<&Value>,
) -> bool {
    if allowed_source_ids.is_empty() {
        return true;
    }
    allowed_source_ids.iter().any(|allowed| {
        sender_id.is_some_and(|sender_id| feishu_sender_id_matches(sender_id, allowed))
    })
}

fn feishu_sender_id_matches(sender_id: &Value, allowed: &str) -> bool {
    ["open_id", "user_id", "union_id"]
        .into_iter()
        .any(|key| sender_id.get(key).and_then(Value::as_str) == Some(allowed))
}

fn preferred_feishu_sender_id(sender_id: &Value) -> Option<&str> {
    sender_id
        .get("open_id")
        .and_then(Value::as_str)
        .or_else(|| sender_id.get("user_id").and_then(Value::as_str))
        .or_else(|| sender_id.get("union_id").and_then(Value::as_str))
}

fn feishu_sender_ids_summary(sender_id: Option<&Value>) -> String {
    let Some(sender_id) = sender_id else {
        return "<missing>".to_owned();
    };
    let mut parts = Vec::new();
    for key in ["open_id", "user_id", "union_id"] {
        if let Some(value) = sender_id.get(key).and_then(Value::as_str) {
            parts.push(format!("{key}={value}"));
        }
    }
    if parts.is_empty() {
        "<missing>".to_owned()
    } else {
        parts.join(",")
    }
}

fn feishu_allowed_ids_summary(allowed_source_ids: &[Arc<str>]) -> String {
    if allowed_source_ids.is_empty() {
        "<unset>".to_owned()
    } else {
        allowed_source_ids
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>()
            .join(",")
    }
}

pub(super) fn feishu_receive_id_type<'a>(receive_id: &str, configured: &'a str) -> &'a str {
    if receive_id.starts_with("oc_") {
        "chat_id"
    } else if receive_id.starts_with("ou_") {
        "open_id"
    } else if receive_id.starts_with("on_") {
        "union_id"
    } else {
        configured
    }
}

pub(super) fn feishu_allowed_source_ids_from_env() -> Vec<Arc<str>> {
    let mut values = Vec::new();
    extend_allowed_source_ids(&mut values, env::var("AGENTOS_FEISHU_ALLOWED_ID").ok());
    values
}

fn extend_allowed_source_ids(values: &mut Vec<Arc<str>>, raw: Option<String>) {
    let Some(raw) = raw else {
        return;
    };
    for value in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if values.iter().any(|existing| existing.as_ref() == value) {
            continue;
        }
        values.push(Arc::from(value));
    }
}

pub(super) fn curl_failure_message(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(stdout).trim().to_owned();
    if !stdout.is_empty() {
        return stdout;
    }
    "curl command failed".to_owned()
}
