use crate::ids::ToolCallId;
use serde::{Deserialize, Serialize};
use serde_json::{value::RawValue, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: Arc<str>,
    pub args: Box<RawValue>,
}

impl PartialEq for ToolCall {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.name == other.name && self.args.get() == other.args.get()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Succeeded,
    Failed,
    Denied,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: ToolCallId,
    pub status: ToolStatus,
    pub content: Arc<str>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<Arc<str>, Value>,
}
