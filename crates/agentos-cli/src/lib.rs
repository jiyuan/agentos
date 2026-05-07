//! Library surface shared by the agentos-cli binaries (TUI + gateway).
//!
//! Currently exposes the slash-command parser and renderers so the TUI and the
//! Telegram/Feishu gateway can speak the same `/help`, `/skills`, `/crons`,
//! `/tools`, `/memory`, `/orchestrator`, `/model`, `/clear` vocabulary.

pub mod slash;
