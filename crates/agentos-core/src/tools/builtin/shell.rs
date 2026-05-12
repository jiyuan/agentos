use super::common::{elapsed_ms, result_metadata, safe_workspace_path, workspace_root};
use agentos_interfaces::tool::{Tool, ToolError, ToolSpec};
use agentos_proto::{ToolCall, ToolResult, ToolStatus};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, value::RawValue, Value};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
pub struct ShellTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellArgs {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

#[async_trait]
impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("shell"),
            description: Arc::from("Run an allowlisted shell command with structured arguments."),
            input_schema: json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "cwd": { "type": "string" }
                }
            }),
            requires_isolation: true,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let parsed: ShellArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let start = Instant::now();
        let mut command = Command::new(&parsed.command);
        command.args(&parsed.args);
        if let Some(cwd) = parsed.cwd {
            let safe = safe_workspace_path(&workspace_root(), &cwd)
                .map_err(|err| ToolError::Failed(Arc::from(err)))?;
            command.current_dir(safe);
        }

        let output = command
            .output()
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let duration_ms = elapsed_ms(start);
        let mut content = String::from_utf8_lossy(&output.stdout).into_owned();
        if content.is_empty() {
            content = String::from_utf8_lossy(&output.stderr).into_owned();
        }
        let bytes_out = content.len() as u64;
        let mut metadata = result_metadata(duration_ms, bytes_out);
        metadata.insert(
            Arc::from("exit_code"),
            output
                .status
                .code()
                .map_or(Value::Null, |code| Value::from(code as i64)),
        );
        metadata.insert(
            Arc::from("stderr_bytes"),
            Value::from(output.stderr.len() as u64),
        );

        Ok(ToolResult {
            call_id: call.id.clone(),
            status: if output.status.success() {
                ToolStatus::Succeeded
            } else {
                ToolStatus::Failed
            },
            content: Arc::from(content),
            metadata,
        })
    }
}
