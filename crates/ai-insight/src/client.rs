use std::time::Duration;

use serde_json::{Value, json};

use crate::cli;
use crate::config::{InsightConfig, InsightEngine};
use crate::reduce::InsightSnapshot;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const TEMPERATURE: f64 = 0.2;
const MAX_TOKENS: u32 = 350;
const MAX_NEXT_CHECKS: u32 = 3;
const MAX_REPLY_WORDS: u32 = 120;

#[derive(Debug, thiserror::Error)]
pub enum InsightError {
    #[error("AI provider base URL, model, and API key must be set")]
    NotConfigured,
    #[error("HTTP {status}")]
    Http { status: u16 },
    #[error("{0}")]
    Transport(String),
    #[error("empty model reply")]
    EmptyReply,
    #[error("{0}")]
    Cli(String),
}

/// Sends the snapshot to the configured engine and returns the assistant text.
///
/// Does not log the API key or request/response bodies at info.
pub fn complete(
    config: &InsightConfig,
    snapshot: &InsightSnapshot,
    _generation: u64,
) -> Result<String, InsightError> {
    if !config.is_configured() {
        tracing::warn!("insight request skipped: AI provider is not configured");
        return Err(InsightError::NotConfigured);
    }

    let snapshot_json = snapshot
        .to_pretty_json()
        .map_err(|err| InsightError::Transport(err.to_string()))?;

    match config.engine {
        InsightEngine::Http => {
            let text = complete_http(config, &system_prompt(), &snapshot_json)?;
            ensure_reply(text)
        }
        InsightEngine::CodexCli => {
            let prompt = format!("{}\n\nSnapshot:\n{snapshot_json}", system_prompt());
            ensure_reply(cli::run_codex(config, &prompt)?)
        }
        InsightEngine::OpenCodeCli => {
            let prompt = format!("{}\n\nSnapshot:\n{snapshot_json}", system_prompt());
            ensure_reply(cli::run_opencode(config, &prompt)?)
        }
    }
}

fn ensure_reply(text: String) -> Result<String, InsightError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Err(InsightError::EmptyReply)
    } else {
        Ok(trimmed.to_string())
    }
}

fn system_prompt() -> String {
    format!(
        "You are an Android runtime analyst inside a terminal HUD.\n\
Comment only on the supplied snapshot. Do not invent stack frames.\n\
Output:\n\
1) Top issues: what / evidence count / likely cause\n\
2) Next checks, max {MAX_NEXT_CHECKS} bullets\n\
Max {MAX_REPLY_WORDS} words. No preamble. No HEALTHY/DEGRADING/FAILING verdict line."
    )
}

fn complete_http(
    config: &InsightConfig,
    system: &str,
    snapshot_json: &str,
) -> Result<String, InsightError> {
    let url = config.completions_url();
    let body = json!({
        "model": config.model,
        "temperature": TEMPERATURE,
        "max_tokens": MAX_TOKENS,
        "stream": false,
        "thinking": { "type": "disabled" },
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": snapshot_json },
        ],
    });

    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    let result = agent
        .post(&url)
        .set(
            "Authorization",
            &format!("Bearer {}", config.api_key.trim()),
        )
        .set("Content-Type", "application/json")
        .send_json(body);

    match result {
        Ok(response) => {
            let text = response
                .into_string()
                .map_err(|err| InsightError::Transport(err.to_string()))?;
            extract_assistant_text(&text)
        }
        Err(ureq::Error::Status(status, _response)) => Err(InsightError::Http { status }),
        Err(err) => Err(InsightError::Transport(err.to_string())),
    }
}

fn extract_assistant_text(body: &str) -> Result<String, InsightError> {
    let value: Value =
        serde_json::from_str(body).map_err(|err| InsightError::Transport(err.to_string()))?;
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    match content {
        Some(text) => Ok(text.to_string()),
        None => Err(InsightError::EmptyReply),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_assistant_text_from_openai_shape() {
        let body = r#"{ "choices": [{ "message": { "content": "FAILING  disk full" } }] }"#;
        assert_eq!(extract_assistant_text(body).unwrap(), "FAILING  disk full");
    }

    #[test]
    fn ensure_reply_rejects_blank_output() {
        assert!(ensure_reply("  \n".to_string()).is_err());
        assert_eq!(ensure_reply(" ok \n".to_string()).unwrap(), "ok");
    }
}
