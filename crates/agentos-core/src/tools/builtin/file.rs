use super::common::{elapsed_ms, result_metadata, safe_workspace_path, workspace_root};
use agentos_interfaces::tool::{Tool, ToolError, ToolSpec};
use agentos_proto::{ToolCall, ToolResult, ToolStatus};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, value::RawValue};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
pub struct FileTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileArgs {
    operation: String,
    path: PathBuf,
    #[serde(default)]
    content: Option<String>,
}

#[async_trait]
impl Tool for FileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("file"),
            description: Arc::from("Read or write a UTF-8 file."),
            input_schema: json!({
                "type": "object",
                "required": ["operation", "path"],
                "properties": {
                    "operation": { "type": "string", "enum": ["read", "write"] },
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                }
            }),
            requires_isolation: false,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let parsed: FileArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let start = Instant::now();
        let safe_path = safe_workspace_path(&workspace_root(), &parsed.path)
            .map_err(|err| ToolError::Failed(Arc::from(err)))?;
        match parsed.operation.as_str() {
            "read" => {
                let content = std::fs::read_to_string(&safe_path)
                    .map_err(|err| ToolError::Failed(err.to_string().into()))?;
                let bytes_out = content.len() as u64;
                Ok(ToolResult {
                    call_id: call.id.clone(),
                    status: ToolStatus::Succeeded,
                    content: Arc::from(content),
                    metadata: result_metadata(elapsed_ms(start), bytes_out),
                })
            }
            "write" => {
                let content = parsed.content.unwrap_or_default();
                // Models routinely request writes into directories that
                // don't exist yet (e.g. `workspace/skills/rss-digest/SKILL.md`).
                // Create the parent chain so they don't get ENOENT on first
                // touch — they can always rm afterwards if it wasn't desired.
                if let Some(parent) = safe_path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)
                            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
                    }
                }
                std::fs::write(&safe_path, content.as_bytes())
                    .map_err(|err| ToolError::Failed(err.to_string().into()))?;
                let message = format!("wrote {} bytes", content.len());
                Ok(ToolResult {
                    call_id: call.id.clone(),
                    status: ToolStatus::Succeeded,
                    content: Arc::from(message),
                    metadata: result_metadata(elapsed_ms(start), content.len() as u64),
                })
            }
            operation => Err(ToolError::Failed(
                format!("unsupported file operation: {operation}").into(),
            )),
        }
    }
}
