use crate::runner::{
    approval_prompt_envelope, resume_run, run_envelope, PausedRun, ResumeDecision, RunOutcome,
    RunnerDeps, RunnerError,
};
use agentos_interfaces::{Channel, ChannelError, RunState};
use agentos_proto::{Envelope, InterruptionId, RunId};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;

const REPLY_PREFIX_METADATA_KEY: &str = "gateway_reply_prefix";

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("channel failed: {0}")]
    Channel(#[from] ChannelError),
    #[error("runner failed: {0}")]
    Runner(#[from] RunnerError),
}

#[derive(Debug)]
pub enum GatewayRun {
    Finished {
        state: RunState,
        output: Envelope,
    },
    Paused {
        paused: PausedRun,
        prompt: Option<Envelope>,
    },
}

pub struct Gateway {
    inbound_tx: mpsc::Sender<Envelope>,
    inbound_rx: mpsc::Receiver<Envelope>,
}

impl Gateway {
    pub fn bounded(capacity: usize) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(capacity);
        Self {
            inbound_tx,
            inbound_rx,
        }
    }

    pub fn sender(&self) -> mpsc::Sender<Envelope> {
        self.inbound_tx.clone()
    }

    pub async fn receive(&mut self) -> Option<Envelope> {
        self.inbound_rx.recv().await
    }
}

pub struct GatewayService<'a> {
    deps: &'a RunnerDeps<'a>,
    sender: Arc<str>,
}

impl<'a> GatewayService<'a> {
    pub fn new(deps: &'a RunnerDeps<'a>, sender: impl Into<Arc<str>>) -> Self {
        Self {
            deps,
            sender: sender.into(),
        }
    }

    pub async fn receive_and_run<C>(
        &self,
        channel: &mut C,
        run_id: RunId,
    ) -> Result<Option<GatewayRun>, GatewayError>
    where
        C: Channel,
    {
        let Some(input) = channel.receive().await else {
            return Ok(None);
        };
        self.run_envelope(channel, input, run_id).await.map(Some)
    }

    pub async fn run_envelope<C>(
        &self,
        channel: &C,
        mut input: Envelope,
        run_id: RunId,
    ) -> Result<GatewayRun, GatewayError>
    where
        C: Channel,
    {
        let reply_prefix = extract_reply_prefix(&mut input);
        let channel_id = input.channel_id.clone();
        let conversation_id = input.conversation_id.clone();
        match run_envelope(input, run_id, self.deps).await? {
            RunOutcome::Finished { state, mut output } => {
                apply_reply_prefix_value(&mut output, reply_prefix.as_deref());
                channel.send(output.clone()).await?;
                Ok(GatewayRun::Finished { state, output })
            }
            RunOutcome::Paused(state) => {
                let paused = PausedRun {
                    channel_id,
                    conversation_id,
                    state,
                };
                self.send_approval_prompt(channel, paused, reply_prefix)
                    .await
            }
        }
    }

    pub async fn resume<C>(
        &self,
        channel: &C,
        paused: PausedRun,
        approval_id: &InterruptionId,
        decision: ResumeDecision,
    ) -> Result<GatewayRun, GatewayError>
    where
        C: Channel,
    {
        let reply_prefix = reply_prefix_from_state(&paused.state);
        let channel_id = paused.channel_id.clone();
        let conversation_id = paused.conversation_id.clone();
        match resume_run(paused, approval_id, decision, self.deps).await? {
            RunOutcome::Finished { state, mut output } => {
                apply_reply_prefix_value(&mut output, reply_prefix.as_deref());
                channel.send(output.clone()).await?;
                Ok(GatewayRun::Finished { state, output })
            }
            RunOutcome::Paused(state) => {
                let paused = PausedRun {
                    channel_id,
                    conversation_id,
                    state,
                };
                self.send_approval_prompt(channel, paused, reply_prefix)
                    .await
            }
        }
    }

    async fn send_approval_prompt<C>(
        &self,
        channel: &C,
        paused: PausedRun,
        reply_prefix: Option<Arc<str>>,
    ) -> Result<GatewayRun, GatewayError>
    where
        C: Channel,
    {
        let mut prompt = approval_prompt_envelope(&paused, Arc::clone(&self.sender));
        if let Some(prompt) = &mut prompt {
            apply_reply_prefix_value(prompt, reply_prefix.as_deref());
            channel.send(prompt.clone()).await?;
        }
        Ok(GatewayRun::Paused { paused, prompt })
    }
}

pub fn extract_reply_prefix(env: &mut Envelope) -> Option<Arc<str>> {
    let content = env.message.content.as_ref();
    let prefix_end = task_prefix_end(content)?;
    let prefix: Arc<str> = Arc::from(content[..prefix_end].to_owned());
    let stripped = content[prefix_end..].trim_start();
    env.message.content = Arc::from(stripped.to_owned());
    env.metadata.insert(
        Arc::from(REPLY_PREFIX_METADATA_KEY),
        serde_json::Value::String(prefix.as_ref().to_owned()),
    );
    Some(prefix)
}

pub fn apply_reply_prefix(env: &mut Envelope) {
    let Some(prefix) = env
        .metadata
        .get(REPLY_PREFIX_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return;
    };
    apply_reply_prefix_value(env, Some(&prefix));
}

fn apply_reply_prefix_value(env: &mut Envelope, prefix: Option<&str>) {
    let Some(prefix) = prefix else {
        return;
    };
    if env.message.content.starts_with(prefix) {
        return;
    }
    env.message.content = Arc::from(format!("{prefix} {}", env.message.content));
}

fn reply_prefix_from_state(state: &RunState) -> Option<Arc<str>> {
    state.transcript.items.iter().rev().find_map(|item| {
        item.metadata
            .get(REPLY_PREFIX_METADATA_KEY)
            .and_then(serde_json::Value::as_str)
            .map(|prefix| Arc::from(prefix.to_owned()))
    })
}

fn task_prefix_end(content: &str) -> Option<usize> {
    let rest = content.strip_prefix("[task:")?;
    let id_end = rest.find(']')?;
    let id = &rest[..id_end];
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return None;
    }
    Some("[task:".len() + id_end + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentos_proto::{ChannelId, ConversationId, Message, MessageRole};
    use std::collections::BTreeMap;

    #[test]
    fn task_prefix_is_stripped_from_input_and_restored_on_reply() {
        let mut input = Envelope {
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("chat"),
            sender: Arc::from("user"),
            message: Message::text(MessageRole::User, "[task:abc_123] /help"),
            metadata: BTreeMap::new(),
        };

        let prefix = extract_reply_prefix(&mut input).expect("prefix");
        assert_eq!(prefix.as_ref(), "[task:abc_123]");
        assert_eq!(input.message.content.as_ref(), "/help");

        let mut output = Envelope {
            channel_id: input.channel_id.clone(),
            conversation_id: input.conversation_id.clone(),
            sender: Arc::from("assistant"),
            message: Message::text(MessageRole::Assistant, "help text"),
            metadata: input.metadata.clone(),
        };
        apply_reply_prefix(&mut output);

        assert_eq!(output.message.content.as_ref(), "[task:abc_123] help text");
    }
}
