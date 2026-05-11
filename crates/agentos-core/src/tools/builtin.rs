use crate::http::shared_client;
use crate::skills::{create_skill, SkillCreation, SkillResourceKind};
use agentos_interfaces::tool::{Tool, ToolError, ToolSpec};
use agentos_proto::{ToolCall, ToolResult, ToolStatus};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, value::RawValue, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
pub struct ShellTool;

#[derive(Debug, Deserialize)]
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
            command.current_dir(cwd);
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

#[derive(Default)]
pub struct FileTool;

#[derive(Debug, Deserialize)]
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
        match parsed.operation.as_str() {
            "read" => {
                let content = std::fs::read_to_string(&parsed.path)
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
                if let Some(parent) = parsed.path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)
                            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
                    }
                }
                std::fs::write(&parsed.path, content.as_bytes())
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

#[derive(Default)]
pub struct HttpTool;

#[derive(Debug, Deserialize)]
struct HttpArgs {
    url: String,
    #[serde(default = "default_get")]
    method: String,
}

#[async_trait]
impl Tool for HttpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("http"),
            description: Arc::from("Fetch an HTTP or HTTPS URL with a GET request."),
            input_schema: json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": { "type": "string" },
                    "method": { "type": "string", "enum": ["GET"] }
                }
            }),
            requires_isolation: false,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let parsed: HttpArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        if !parsed.method.eq_ignore_ascii_case("GET") {
            return Err(ToolError::Failed(Arc::from("http tool only supports GET")));
        }
        if !(parsed.url.starts_with("http://") || parsed.url.starts_with("https://")) {
            return Err(ToolError::Failed(Arc::from(
                "http tool requires an http:// or https:// URL",
            )));
        }

        let start = Instant::now();
        let response = shared_client()
            .get(&parsed.url)
            .send()
            .await
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let status_code = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let bytes_out = body.len() as u64;
        let mut metadata = result_metadata(elapsed_ms(start), bytes_out);
        metadata.insert(
            Arc::from("status_line"),
            Value::String(format!(
                "HTTP {} {}",
                status_code.as_u16(),
                status_code.canonical_reason().unwrap_or("")
            )),
        );

        Ok(ToolResult {
            call_id: call.id.clone(),
            status: status_from_http_code(status_code.as_u16()),
            content: Arc::from(body),
            metadata,
        })
    }
}

fn status_from_http_code(code: u16) -> ToolStatus {
    if (200..300).contains(&code) {
        ToolStatus::Succeeded
    } else {
        ToolStatus::Failed
    }
}

fn default_get() -> String {
    "GET".to_owned()
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn result_metadata(duration_ms: u64, bytes_out: u64) -> BTreeMap<Arc<str>, Value> {
    let mut metadata = BTreeMap::new();
    metadata.insert(Arc::from("duration_ms"), Value::from(duration_ms));
    metadata.insert(Arc::from("bytes_out"), Value::from(bytes_out));
    metadata
}

/// Tool wrapping `crate::skills::create_skill` so model-driven sub-agents can
/// scaffold new workspace skills end-to-end (folder, frontmatter SKILL.md,
/// optional resource subdirectories, and validation pass) inside the normal
/// run-loop approval and guardrail flow.
#[derive(Default)]
pub struct SkillCreatorTool;

#[derive(Debug, Deserialize)]
struct SkillCreateArgs {
    name: String,
    description: String,
    #[serde(default)]
    resources: Vec<String>,
    /// Override the workspace skills directory. Defaults to `workspace/skills`
    /// relative to the gateway's cwd, matching `runtime::skills_root()`.
    #[serde(default)]
    root: Option<PathBuf>,
}

#[async_trait]
impl Tool for SkillCreatorTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("skill_create"),
            description: Arc::from(
                "Scaffold a new workspace skill following the Anthropic \
                 SKILL.md folder format. Creates workspace/skills/<name>/SKILL.md \
                 with YAML frontmatter and an optional set of bundled resource \
                 directories (scripts, references, assets). Validates the \
                 result before returning.",
            ),
            input_schema: json!({
                "type": "object",
                "required": ["name", "description"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name in lowercase hyphen-case (e.g. rss-digest)."
                    },
                    "description": {
                        "type": "string",
                        "description": "One-line description used as the catalog summary."
                    },
                    "resources": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["scripts", "references", "assets"] },
                        "description": "Optional bundled resource subdirectories to create."
                    },
                    "root": {
                        "type": "string",
                        "description": "Override for the workspace skills root. Defaults to workspace/skills."
                    }
                }
            }),
            requires_isolation: false,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let parsed: SkillCreateArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let start = Instant::now();

        let mut resources = BTreeSet::new();
        for entry in &parsed.resources {
            let kind = match entry.as_str() {
                "scripts" => SkillResourceKind::Scripts,
                "references" => SkillResourceKind::References,
                "assets" => SkillResourceKind::Assets,
                other => {
                    return Err(ToolError::Failed(Arc::from(format!(
                        "unknown resource kind '{other}'; expected scripts, references, or assets"
                    ))));
                }
            };
            resources.insert(kind);
        }

        let creation = SkillCreation {
            name: Arc::from(parsed.name.as_str()),
            description: Arc::from(parsed.description.as_str()),
            resources,
        };
        let root: PathBuf = parsed
            .root
            .unwrap_or_else(|| Path::new("workspace").join("skills"));
        let skill = create_skill(&root, &creation)
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;

        let message = format!("created skill '{}' at {}", skill.name, skill.path.display());
        let bytes_out = message.len() as u64;
        Ok(ToolResult {
            call_id: call.id.clone(),
            status: ToolStatus::Succeeded,
            content: Arc::from(message),
            metadata: result_metadata(elapsed_ms(start), bytes_out),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentos_proto::ToolCallId;

    fn tool_call(id: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId::new(id),
            name: Arc::from("skill_create"),
            args: RawValue::from_string("{}".to_owned()).unwrap(),
        }
    }

    #[tokio::test]
    async fn skill_creator_tool_scaffolds_skill_dir_and_markdown() {
        let tmp = std::env::temp_dir().join(format!(
            "skill-creator-tool-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let args = json!({
            "name": "test-skill",
            "description": "Smoke test for SkillCreatorTool.",
            "resources": ["scripts"],
            "root": tmp.to_string_lossy(),
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let call = tool_call("call_1");
        let result = SkillCreatorTool.call(&call, &raw).await.unwrap();
        assert_eq!(result.status, ToolStatus::Succeeded);
        assert!(result.content.contains("test-skill"));
        let skill_md = tmp.join("test-skill").join("SKILL.md");
        let body = std::fs::read_to_string(&skill_md).unwrap();
        assert!(body.contains("name: test-skill"));
        assert!(body.contains("Smoke test for SkillCreatorTool."));
        assert!(tmp.join("test-skill").join("scripts").is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn skill_creator_tool_rejects_unknown_resource() {
        let tmp = std::env::temp_dir().join(format!(
            "skill-creator-bad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let args = json!({
            "name": "another-skill",
            "description": "n/a",
            "resources": ["nope"],
            "root": tmp.to_string_lossy(),
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = SkillCreatorTool
            .call(&tool_call("call_2"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("unknown resource kind"));
    }
}
