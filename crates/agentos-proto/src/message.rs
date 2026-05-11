use crate::ids::ToolCallId;
use crate::tool::ToolCall;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Document,
}

/// A binary payload attached to a message (image, document, etc).
///
/// Channels download the bytes into the workspace before constructing the
/// envelope, then point downstream consumers at `path`. `source` carries the
/// channel-native identifier (Telegram `file_id`, Feishu `image_key` /
/// `file_key`) so outbound flows can re-reference the upload if needed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub name: Arc<str>,
    pub path: PathBuf,
    pub mime: Option<Arc<str>>,
    pub size: Option<u64>,
    pub source: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: Arc<str>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// Assistant-side tool requests emitted by an LLM provider. Empty for
    /// non-assistant messages and for assistant messages that contain only
    /// text. The accompanying tool result lands in a separate `Tool`-role
    /// message whose `tool_call_id` matches one of these entries' `id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Set on `Tool`-role messages to link the result back to the assistant
    /// turn that requested it. Providers need this to satisfy their
    /// tool-call/tool-result pairing requirements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<ToolCallId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<Arc<str>, Value>,
}

impl Message {
    pub fn text(role: MessageRole, content: impl Into<Arc<str>>) -> Self {
        Self {
            role,
            content: content.into(),
            attachments: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_attachments(
        role: MessageRole,
        content: impl Into<Arc<str>>,
        attachments: Vec<Attachment>,
    ) -> Self {
        Self {
            role,
            content: content.into(),
            attachments,
            tool_calls: Vec::new(),
            tool_call_id: None,
            metadata: BTreeMap::new(),
        }
    }
}
