use super::common::{cron_root_for_tests, default_cron_dir, elapsed_ms, result_metadata};
use crate::crons::{CronSchedule, CronStore, CronTask};
use agentos_interfaces::tool::{Tool, ToolError, ToolSpec};
use agentos_proto::{ChannelId, ConversationId, ToolCall, ToolResult, ToolStatus};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, value::RawValue};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

/// Deserialised tool input.
///
/// Note: `root` (and the test-only override) is intentionally *not* exposed
/// on the LLM-visible schema. The model picking its own cron directory is a
/// foot-gun — it'll happily write to `workspace/` and then claim success.
/// The runtime resolves the directory itself via `$AGENTOS_CRON_DIR` or the
/// `workspace/crons` default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
                    "interval_days": { "type": "integer", "minimum": 1 }
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
        // Always fire after one full interval — past timestamps would cause
        // the scheduler to fire immediately on the next tick, which is
        // surprising. Operators can edit the TOML by hand if they want a
        // specific first-run wall-clock time.
        let next_due_unix = now.saturating_add(interval_seconds);

        let schedule = CronSchedule::every_seconds(interval_seconds, next_due_unix)
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;

        let task = CronTask::new(
            parsed.id.as_str(),
            ChannelId::new(parsed.channel_id.as_str()),
            ConversationId::new(parsed.conversation_id.as_str()),
            parsed.prompt.as_str(),
            schedule,
        );

        let store = CronStore::new(cron_root_for_tests().unwrap_or_else(default_cron_dir));
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

/// Tool: enumerate every persisted cron task. Reads `workspace/crons/*.toml`
/// via `CronStore::load_scheduler` and returns a compact JSON-encoded summary
/// the model can reason about ("delete the broken one", "tell me what runs
/// every day at 9am", etc).
#[derive(Default)]
pub struct CronListTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CronListArgs {}

#[async_trait]
impl Tool for CronListTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("cron_list"),
            description: Arc::from(
                "Enumerate every persisted cron task (id, channel, conversation, \
                 interval, next-due, enabled). Use this when the user asks which \
                 crons exist or wants to confirm a previous schedule.",
            ),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            requires_isolation: false,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let _parsed: CronListArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let start = Instant::now();
        let store = CronStore::new(cron_root_for_tests().unwrap_or_else(default_cron_dir));
        let scheduler = store
            .load_scheduler()
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;
        let summaries = scheduler
            .tasks()
            .iter()
            .map(|task| {
                json!({
                    "id": task.id.as_ref(),
                    "channel_id": task.channel_id.as_str(),
                    "conversation_id": task.conversation_id.as_str(),
                    "prompt": task.prompt.as_ref(),
                    "interval_seconds": task.schedule.interval_seconds,
                    "next_due_unix": task.schedule.next_due_unix,
                    "enabled": task.enabled,
                })
            })
            .collect::<Vec<_>>();
        let body = serde_json::to_string(&summaries)
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let bytes_out = body.len() as u64;
        Ok(ToolResult {
            call_id: call.id.clone(),
            status: ToolStatus::Succeeded,
            content: Arc::from(body),
            metadata: result_metadata(elapsed_ms(start), bytes_out),
        })
    }
}

/// Tool: delete a persisted cron task by id. Just removes the TOML file —
/// the scheduler will stop replaying it on its next cycle. Idempotent: a
/// missing file is treated as a no-op so retries are safe.
#[derive(Default)]
pub struct CronRemoveTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CronRemoveArgs {
    id: String,
}

#[async_trait]
impl Tool for CronRemoveTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Arc::from("cron_remove"),
            description: Arc::from(
                "Delete a persisted cron task by id. Use this when the user asks \
                 to cancel, remove, or stop a scheduled task. Idempotent.",
            ),
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Cron id as previously returned by cron_create or cron_list."
                    }
                }
            }),
            requires_isolation: false,
        }
    }

    async fn call(&self, call: &ToolCall, args: &RawValue) -> Result<ToolResult, ToolError> {
        let parsed: CronRemoveArgs = serde_json::from_str(args.get())
            .map_err(|err| ToolError::Failed(err.to_string().into()))?;
        let start = Instant::now();
        let store = CronStore::new(cron_root_for_tests().unwrap_or_else(default_cron_dir));
        let path = store
            .task_path(&parsed.id)
            .map_err(|err| ToolError::Failed(Arc::from(err.to_string())))?;
        let message = match std::fs::remove_file(&path) {
            Ok(()) => format!("removed cron '{}' ({})", parsed.id, path.display()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                format!("cron '{}' was not present (no-op)", parsed.id)
            }
            Err(err) => return Err(ToolError::Failed(err.to_string().into())),
        };
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
    use super::super::common::test_support::{tool_call, CronDirGuard};
    use super::*;
    use serde_json::Value;

    #[tokio::test]
    async fn cron_creator_tool_persists_task_file() {
        let guard = CronDirGuard::new("cron-creator-tool");
        let args = json!({
            "id": "daily-digest",
            "channel_id": "telegram",
            "conversation_id": "5480467472",
            "prompt": "Summarize the day's notes.",
            "interval_hours": 24,
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let result = CronCreatorTool
            .call(&tool_call("cron_create", "call_1"), &raw)
            .await
            .unwrap();
        assert_eq!(result.status, ToolStatus::Succeeded);
        assert!(result.content.contains("daily-digest"));

        let task_path = guard.dir.join("daily-digest.toml");
        assert!(task_path.is_file());
        let body = std::fs::read_to_string(&task_path).unwrap();
        let task: CronTask = toml::from_str(&body).unwrap();
        assert_eq!(task.id.as_ref(), "daily-digest");
        assert_eq!(task.channel_id.as_str(), "telegram");
        assert_eq!(task.conversation_id.as_str(), "5480467472");
        assert_eq!(task.prompt.as_ref(), "Summarize the day's notes.");
        assert_eq!(task.schedule.interval_seconds, 24 * 3600);
    }

    #[tokio::test]
    async fn cron_creator_tool_rejects_root_override_from_caller() {
        let _guard = CronDirGuard::new("cron-creator-rooted");
        let args = json!({
            "id": "rooted",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "hi",
            "interval_seconds": 60,
            "root": "workspace",
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = CronCreatorTool
            .call(&tool_call("cron_create", "call_root"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("unknown field") && msg.contains("root"));
    }

    #[tokio::test]
    async fn cron_creator_tool_requires_an_interval() {
        let _guard = CronDirGuard::new("cron-creator-no-interval");
        let args = json!({
            "id": "x",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "hi",
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = CronCreatorTool
            .call(&tool_call("cron_create", "call_2"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("interval_seconds"));
    }

    #[tokio::test]
    async fn cron_creator_tool_rejects_multiple_interval_fields() {
        let _guard = CronDirGuard::new("cron-creator-many-intervals");
        let args = json!({
            "id": "x",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "hi",
            "interval_hours": 1,
            "interval_days": 1,
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = CronCreatorTool
            .call(&tool_call("cron_create", "call_3"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("only one of"));
    }

    #[tokio::test]
    async fn cron_creator_tool_rejects_invalid_id() {
        let _guard = CronDirGuard::new("cron-creator-bad-id");
        let args = json!({
            "id": "has spaces!",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "hi",
            "interval_seconds": 60,
        });
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let err = CronCreatorTool
            .call(&tool_call("cron_create", "call_4"), &raw)
            .await
            .unwrap_err();
        let ToolError::Failed(msg) = err;
        assert!(msg.contains("invalid cron id"));
    }

    #[tokio::test]
    async fn cron_list_tool_returns_persisted_tasks() {
        let guard = CronDirGuard::new("cron-list-tool");
        for id in ["one", "two"] {
            let args = json!({
                "id": id,
                "channel_id": "telegram",
                "conversation_id": "1",
                "prompt": format!("ping-{id}"),
                "interval_seconds": 3600,
            });
            let raw = RawValue::from_string(args.to_string()).unwrap();
            CronCreatorTool
                .call(&tool_call("cron_create", "create"), &raw)
                .await
                .unwrap();
        }
        let raw = RawValue::from_string("{}".to_owned()).unwrap();
        let result = CronListTool
            .call(&tool_call("cron_list", "list"), &raw)
            .await
            .unwrap();
        let body: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body.len(), 2);
        let ids: Vec<&str> = body.iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"one"));
        assert!(ids.contains(&"two"));
        drop(guard);
    }

    #[tokio::test]
    async fn cron_remove_tool_deletes_file_and_is_idempotent() {
        let guard = CronDirGuard::new("cron-remove-tool");
        let create_args = json!({
            "id": "doomed",
            "channel_id": "telegram",
            "conversation_id": "1",
            "prompt": "x",
            "interval_seconds": 60,
        });
        CronCreatorTool
            .call(
                &tool_call("cron_create", "create"),
                &RawValue::from_string(create_args.to_string()).unwrap(),
            )
            .await
            .unwrap();
        assert!(guard.dir.join("doomed.toml").is_file());

        let remove_args = RawValue::from_string(r#"{"id":"doomed"}"#.to_owned()).unwrap();
        let result = CronRemoveTool
            .call(&tool_call("cron_remove", "remove-1"), &remove_args)
            .await
            .unwrap();
        assert_eq!(result.status, ToolStatus::Succeeded);
        assert!(result.content.contains("removed"));
        assert!(!guard.dir.join("doomed.toml").exists());

        let result = CronRemoveTool
            .call(&tool_call("cron_remove", "remove-2"), &remove_args)
            .await
            .unwrap();
        assert_eq!(result.status, ToolStatus::Succeeded);
        assert!(result.content.contains("no-op"));
    }
}
