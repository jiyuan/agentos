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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<Arc<str>, Value>,
}

impl Message {
    pub fn text(role: MessageRole, content: impl Into<Arc<str>>) -> Self {
        Self {
            role,
            content: content.into(),
            attachments: Vec::new(),
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
            metadata: BTreeMap::new(),
        }
    }
}
