use crate::crons::{CronSchedule, CronStore, CronTask};
use crate::http::shared_client;
use crate::skills::{create_skill, SkillCreation, SkillResourceKind};
use agentos_interfaces::tool::{Tool, ToolError, ToolSpec};
use agentos_proto::{ChannelId, ConversationId, ToolCall, ToolResult, ToolStatus};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, value::RawValue, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

/// Tool wrapping `crate::crons::CronStore::save_task` so sub-agents can
/// register a recurring task end-to-end (TOML file written under
/// `workspace/crons/<id>.toml`) inside the normal run-loop approval and
/// guardrail flow.
///
/// The gateway's scheduler picks new files up from disk on its next polling
/// cycle, so once this tool returns success the task is live without any
/// daemon restart.
#[derive(Default)]
pub struct CronCreatorTool;

#[derive(Debug, Deserialize)]
struct CronCreateArgs {
    /// Human-readable identifier, alphanumeric / `-` / `_` only. Used as the
    /// on-disk filename and to dedupe scheduler entries.
    id: String,
    /// Channel that should receive the recurring envelope (e.g. "telegram",
    /// "feishu"). Must match the registered `Channel::id()`.
    channel_id: String,
    /// Conversation id to deliver to (the user chat for Telegram, `oc_...`
    /// for Feishu, etc).
    conversation_id: String,
    /// User-side prompt the scheduler will replay each tick.
    prompt: String,
    /// One of `interval_seconds`, `interval_hours`, or `interval_days` is
    /// required.
    #[serde(default)]
    interval_seconds: Option<u64>,
    #[serde(default)]
    interval_hours: Option<u64>,
    #[serde(default)]
    interval_days: Option<u64>,
    /// Unix timestamp of the first run. Defaults to `now + interval` so the
    /// task fires after one full interval rather than immediately on
    /// gateway restart.
    #[serde(default)]
    first_run_unix: Option<u64>,
    /// Override the workspace cron directory. Defaults to `$AGENTOS_CRON_DIR`
    /// or `workspace/crons`.
    #[serde(default)]
    root: Option<PathBuf>,
}

#[async_trait]
impl Tool for CronCreatorTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("cron_create"),
            description: Arc::from(
                "Schedule a recurring AgentOS task. Persists a TOML file under \
                 workspace/crons/<id>.toml; the gateway scheduler picks it up \
                 on its next cycle and replays the supplied prompt at the \
                 chosen interval. Use this whenever a user asks to schedule, \
                 automate, or repeat a chat instruction.",
            ),
            input_schema: json!({
                "type": "object",
                "required": ["id", "channel_id", "conversation_id", "prompt"],
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Alphanumeric / -/_ identifier. Used as the filename and dedupe key."
                    },
                    "channel_id": {
                        "type": "string",
                        "description": "Channel to deliver to: telegram | feishu | tui."
                    },
                    "conversation_id": {
                        "type": "string",
                        "description": "Conversation id to deliver to (Telegram chat id, Feishu oc_..., etc)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The user-side message the scheduler replays each tick."
                    },
                    "interval_seconds": { "type": "integer", "minimum": 1 },
                    "interval_hours": { "type": "integer", "minimum": 1 },
                    "interval_days": { "type": "integer", "minimum": 1 },
                    "first_run_unix": {
                        "type": "integer",
                        "description": "Unix timestamp of the first run. Defaults to now + interval."
                    },
                    "root": {
                        "type": "string",
                        "description": "Override the workspace cron directory."
                    }
                }
            }),
            requires_isolation: false,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let parsed: CronCreateArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let start = Instant::now();

        let interval_seconds = match (
            parsed.interval_seconds,
            parsed.interval_hours,
            parsed.interval_days,
        ) {
            (Some(s), None, None) => s,
            (None, Some(h), None) => h.checked_mul(3_600).ok_or_else(|| {
                ToolError::Failed(Arc::from("interval_hours overflows u64 seconds"))
            })?,
            (None, None, Some(d)) => d.checked_mul(86_400).ok_or_else(|| {
                ToolError::Failed(Arc::from("interval_days overflows u64 seconds"))
            })?,
            (None, None, None) => {
                return Err(ToolError::Failed(Arc::from(
                    "one of interval_seconds, interval_hours, or interval_days is required",
                )));
            }
            _ => {
                return Err(ToolError::Failed(Arc::from(
                    "only one of interval_seconds, interval_hours, interval_days may be set",
                )));
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;
        let next_due_unix = parsed
            .first_run_unix
            .unwrap_or_else(|| now.saturating_add(interval_seconds));

        let schedule = CronSchedule::every_seconds(interval_seconds, next_due_unix)
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;

        let task = CronTask::new(
            parsed.id.as_str(),
            ChannelId::new(parsed.channel_id.as_str()),
            ConversationId::new(parsed.conversation_id.as_str()),
            parsed.prompt.as_str(),
            schedule,
        );

        let root = parsed.root.unwrap_or_else(default_cron_dir);
        let store = CronStore::new(root.clone());
        store
            .save_task(&task)
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;

        let path = store
            .task_path(&task.id)
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;
        let message = format!(
            "created cron '{}' (every {}s, next at {next_due_unix}) at {}",
            task.id,
            interval_seconds,
            path.display()
        );
        let bytes_out = message.len() as u64;
        Ok(ToolResult {
            call_id: call.id.clone(),
            status: ToolStatus::Succeeded,
            content: Arc::from(message),
            metadata: result_metadata(elapsed_ms(start), bytes_out),
        })
    }
}

/// Resolve the on-disk root for cron task files, matching the convention used
/// by `agentos-cli`: respect `$AGENTOS_CRON_DIR` if set, else fall back to
/// `workspace/crons` relative to the gateway's cwd.
fn default_cron_dir() -> PathBuf {
    std::env::var_os("AGENTOS_CRON_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("workspace").join("crons"))
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

    fn unique_tmp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[tokio::test]
    async fn cron_creator_tool_persists_task_file() {
        let tmp = unique_tmp_dir("cron-creator-tool");
        let args = json!({
            "id": "daily-digest",
            "channel_id": "telegram",
            "conversation_id": "5480467472",
            "prompt": "Summarize the day's notes.",
            "interval_hours": 24,
            "root": tmp.to_string_lossy(),
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let result = CronCreatorTool
            .call(&tool_call("call_1"), &raw)
            .await
            .unwrap();
        assert_eq!(result.status, ToolStatus::Succeeded);
        assert!(result.content.contains("daily-digest"));

        let task_path = tmp.join("daily-digest.toml");
        assert!(task_path.is_file());
        let body = std::fs::read_to_string(&task_path).unwrap();
        let task: CronTask = toml::from_str(&body).unwrap();
        assert_eq!(task.id.as_ref(), "daily-digest");
        assert_eq!(task.channel_id.as_str(), "telegram");
        assert_eq!(task.conversation_id.as_str(), "5480467472");
        assert_eq!(task.prompt.as_ref(), "Summarize the day's notes.");
        assert_eq!(task.schedule.interval_seconds, 24 * 3600);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn cron_creator_tool_requires_an_interval() {
        let tmp = unique_tmp_dir("cron-creator-no-interval");
        let args = json!({
            "id": "x",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "hi",
            "root": tmp.to_string_lossy(),
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = CronCreatorTool
            .call(&tool_call("call_2"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("interval_seconds"));
    }

    #[tokio::test]
    async fn cron_creator_tool_rejects_multiple_interval_fields() {
        let tmp = unique_tmp_dir("cron-creator-many-intervals");
        let args = json!({
            "id": "x",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "hi",
            "interval_hours": 1,
            "interval_days": 1,
            "root": tmp.to_string_lossy(),
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = CronCreatorTool
            .call(&tool_call("call_3"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("only one of"));
    }

    #[tokio::test]
    async fn cron_creator_tool_rejects_invalid_id() {
        let tmp = unique_tmp_dir("cron-creator-bad-id");
        let args = json!({
            "id": "has spaces!",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "hi",
            "interval_seconds": 60,
            "root": tmp.to_string_lossy(),
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = CronCreatorTool
            .call(&tool_call("call_4"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("invalid cron id"));
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
