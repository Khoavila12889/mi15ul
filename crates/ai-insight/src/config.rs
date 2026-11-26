//! Provider engine, endpoint, model, and API key.

use serde::{Deserialize, Serialize};

/// How Insight reaches the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightEngine {
    /// OpenAI-compatible Chat Completions over HTTP.
    #[default]
    Http,
    /// Local `codex exec`, reusing the existing Codex login.
    #[serde(rename = "codex_cli")]
    CodexCli,
    /// Local `opencode run`, reusing the existing OpenCode login.
    #[serde(rename = "opencode_cli")]
    OpenCodeCli,
}

impl InsightEngine {
    pub fn label(self) -> &'static str {
        match self {
            InsightEngine::Http => "HTTP API key",
            InsightEngine::CodexCli => "Codex CLI (local login)",
            InsightEngine::OpenCodeCli => "OpenCode CLI (local login)",
        }
    }

    pub fn is_cli(self) -> bool {
        !matches!(self, InsightEngine::Http)
    }
}

/// Provider engine and the fields it needs for one request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InsightConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub engine: InsightEngine,
}

impl InsightConfig {
    /// Returns true when `api_key` is non-empty after trim.
    pub fn has_api_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// Returns true when the selected engine has everything it needs.
    ///
    /// The CLI engines rely on the login already stored by Codex/OpenCode, so
    /// they only need the binary to be installed at request time.
    pub fn is_configured(&self) -> bool {
        match self.engine {
            InsightEngine::Http => {
                self.has_api_key()
                    && !self.base_url.trim().is_empty()
                    && !self.model.trim().is_empty()
            }
            InsightEngine::CodexCli | InsightEngine::OpenCodeCli => true,
        }
    }

    /// Builds `{base_url}/chat/completions` with no trailing slash on the base.
    pub fn completions_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.base_url.trim().trim_end_matches('/')
        )
    }

    /// Returns the host portion of `base_url` for logs (no scheme, no path).
    pub fn host_for_log(&self) -> String {
        if self.engine.is_cli() {
            return self.engine.label().to_string();
        }
        self.base_url
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or(self.base_url.trim())
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(api_key: &str, base_url: &str, model: &str) -> InsightConfig {
        InsightConfig {
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
            engine: InsightEngine::Http,
        }
    }

    #[test]
    fn completions_url_strips_trailing_slash() {
        let config = config("", "https://api.example.com/", "example-model");
        assert_eq!(
            config.completions_url(),
            "https://api.example.com/chat/completions"
        );
    }

    #[test]
    fn host_for_log_drops_scheme() {
        let config = config("", "https://api.example.com/v1", "example-model");
        assert_eq!(config.host_for_log(), "api.example.com");
    }

    #[test]
    fn has_api_key_rejects_blank() {
        assert!(!config("  ", "https://api.example.com", "example-model").has_api_key());
    }

    #[test]
    fn is_configured_requires_all_three() {
        assert!(!config("", "", "").is_configured());
        assert!(!config("sk-test", "", "").is_configured());
        assert!(!config("sk-test", "https://api.example.com", "").is_configured());
        assert!(!config("sk-test", "", "example-model").is_configured());
        assert!(!config("  ", "https://api.example.com", "example-model").is_configured());
        assert!(!config("sk-test", "  ", "example-model").is_configured());
        assert!(!config("sk-test", "https://api.example.com", "  ").is_configured());
        assert!(config("sk-test", "https://api.example.com", "example-model").is_configured());
    }

    #[test]
    fn cli_engines_are_configured_without_api_key() {
        for engine in [InsightEngine::CodexCli, InsightEngine::OpenCodeCli] {
            let config = InsightConfig {
                engine,
                ..InsightConfig::default()
            };
            assert!(config.is_configured());
            assert!(
                config.host_for_log().starts_with("Codex")
                    || config.host_for_log().starts_with("OpenCode")
            );
        }
    }

    #[test]
    fn engine_round_trips_through_json() {
        let json = serde_json::to_string(&InsightEngine::OpenCodeCli).expect("serialize");
        assert_eq!(json, "\"opencode_cli\"");
        let parsed: InsightEngine = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, InsightEngine::OpenCodeCli);
    }

    #[test]
    fn default_engine_is_http() {
        assert_eq!(InsightEngine::default(), InsightEngine::Http);
    }
}
