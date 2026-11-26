use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use ai_insight::{InsightConfig, InsightEngine};
use serde::{Deserialize, Serialize};

/// Version 2 adds `engine`. Version 1 files (HTTP only) still load.
const INSIGHT_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct InsightFile {
    version: u32,
    #[serde(default)]
    engine: InsightEngine,
    base_url: String,
    model: String,
    api_key: String,
}

impl InsightFile {
    fn from_config(config: &InsightConfig) -> Self {
        Self {
            version: INSIGHT_VERSION,
            engine: config.engine,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
            api_key: config.api_key.clone(),
        }
    }

    fn into_config(self) -> Option<InsightConfig> {
        matches!(self.version, 1 | 2).then_some(InsightConfig {
            engine: self.engine,
            base_url: self.base_url,
            model: self.model,
            api_key: self.api_key,
        })
    }
}

/// Credentials already present on this machine for the user's Codex/OpenCode
/// installs. Used by the **Detect local logins** button in the settings dialog.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LocalCredentials {
    /// Codex is signed in with a ChatGPT account (no API key to copy).
    pub codex_chatgpt: bool,
    /// A real `OPENAI_API_KEY` stored by Codex, when present.
    pub codex_api_key: Option<String>,
    /// OpenCode Console/Zen API key from `opencode auth login`.
    pub opencode_api_key: Option<String>,
}

impl LocalCredentials {
    pub fn any_found(&self) -> bool {
        self.codex_chatgpt || self.codex_api_key.is_some() || self.opencode_api_key.is_some()
    }

    /// Short human-readable summary for the dialog status line.
    pub fn summary(&self) -> String {
        let mut found = Vec::new();
        if self.codex_chatgpt {
            found.push(crate::i18n::t("settings.cred_codex_chatgpt"));
        }
        if self.codex_api_key.is_some() {
            found.push(crate::i18n::t("settings.cred_codex_api"));
        }
        if self.opencode_api_key.is_some() {
            found.push(crate::i18n::t("settings.cred_opencode_api"));
        }
        if found.is_empty() {
            crate::i18n::t("settings.cred_none").to_string()
        } else {
            let items = found.join(", ");
            crate::i18n::tf("settings.cred_found", &[("items", &items)])
        }
    }
}

/// Returns the on-disk path of the insight JSON, if a config directory exists.
pub(crate) fn insight_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("android-dashboard")
            .join("insight.json"),
    )
}

/// Reads insight JSON from `path`. Missing or invalid files yield an empty config.
pub(crate) fn load_from_path(path: &Path) -> InsightConfig {
    match fs::read_to_string(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => InsightConfig::default(),
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "failed to read insight config"
            );
            InsightConfig::default()
        }
        Ok(json) => decode(&json).unwrap_or_else(|| {
            tracing::warn!(path = %path.display(), "invalid insight config file");
            InsightConfig::default()
        }),
    }
}

/// Writes insight JSON to `path` via a sibling `.tmp` file.
pub(crate) fn save_to_path(path: &Path, config: &InsightConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let json = encode(config).map_err(|err| std::io::Error::new(ErrorKind::InvalidData, err))?;
    let tmp = tmp_path(path);
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Loads the saved insight config, or empty when none is available.
pub(crate) fn load_or_default() -> InsightConfig {
    match insight_path() {
        Some(path) => load_from_path(&path),
        None => InsightConfig::default(),
    }
}

/// Writes the config to the insight JSON path.
/// Returns true when the file was written.
pub(crate) fn save(config: &InsightConfig) -> bool {
    let Some(path) = insight_path() else {
        tracing::warn!("no config directory; insight config not saved");
        return false;
    };
    if let Err(err) = save_to_path(&path, config) {
        tracing::warn!(
            error = %err,
            path = %path.display(),
            "failed to save insight config"
        );
        return false;
    }
    true
}

/// Reads the local Codex and OpenCode auth files without modifying them.
pub(crate) fn detect_local_credentials() -> LocalCredentials {
    let mut found = LocalCredentials::default();

    if let Some(path) = codex_auth_path() {
        if let Ok(text) = fs::read_to_string(path) {
            let (chatgpt, key) = parse_codex_auth(&text);
            found.codex_chatgpt = chatgpt;
            found.codex_api_key = key;
        }
    }

    for path in opencode_auth_paths() {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Some(key) = parse_opencode_auth(&text) {
                found.opencode_api_key = Some(key);
                break;
            }
        }
    }

    found
}

fn codex_auth_path() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    Some(home.join("auth.json"))
}

fn opencode_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(data_home).join("opencode").join("auth.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("auth.json"),
        );
    }
    if let Some(local) = dirs::data_local_dir() {
        paths.push(local.join("opencode").join("auth.json"));
    }
    if let Some(roaming) = dirs::data_dir() {
        paths.push(roaming.join("opencode").join("auth.json"));
    }
    paths
}

/// Returns `(chatgpt_login, api_key)` from a Codex `auth.json`.
fn parse_codex_auth(json: &str) -> (bool, Option<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return (false, None);
    };

    let api_key = value
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string);

    let chatgpt = value
        .get("auth_mode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("chatgpt"))
        || value
            .get("tokens")
            .and_then(|tokens| tokens.get("access_token"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.trim().is_empty());

    (chatgpt, api_key)
}

/// Returns the `opencode` provider key from an OpenCode `auth.json`.
fn parse_opencode_auth(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .get("opencode")?
        .get("key")?
        .as_str()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

fn encode(config: &InsightConfig) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&InsightFile::from_config(config))
}

fn decode(json: &str) -> Option<InsightConfig> {
    serde_json::from_str::<InsightFile>(json)
        .ok()
        .and_then(InsightFile::into_config)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> PathBuf {
        let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "android-dashboard-insight-test-{}-{n}.json",
            std::process::id()
        ))
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new() -> Self {
            Self(temp_path())
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(tmp_path(&self.0));
        }
    }

    fn sample() -> InsightConfig {
        InsightConfig {
            api_key: "sk-test".into(),
            base_url: "https://api.example.com".into(),
            model: "example-model".into(),
            engine: InsightEngine::Http,
        }
    }

    #[test]
    fn encode_decode_round_trips() {
        let original = sample();
        let json = encode(&original).expect("encode");
        let restored = decode(&json).expect("decode");
        assert_eq!(restored, original);
    }

    #[test]
    fn encode_decode_round_trips_cli_engine() {
        let original = InsightConfig {
            engine: InsightEngine::CodexCli,
            ..sample()
        };
        let json = encode(&original).expect("encode");
        let restored = decode(&json).expect("decode");
        assert_eq!(restored.engine, InsightEngine::CodexCli);
    }

    #[test]
    fn decode_accepts_version_one_as_http() {
        let json = r#"{
            "version": 1,
            "base_url": "https://api.example.com",
            "model": "example-model",
            "api_key": "sk-test"
        }"#;
        let config = decode(json).expect("v1 config");
        assert_eq!(config.engine, InsightEngine::Http);
        assert_eq!(config.api_key, "sk-test");
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode("not json").is_none());
    }

    #[test]
    fn decode_rejects_other_version() {
        let mut value: serde_json::Value =
            serde_json::from_str(&encode(&sample()).expect("encode")).expect("json");
        value["version"] = serde_json::json!(3);
        assert!(decode(&value.to_string()).is_none());
    }

    #[test]
    fn load_missing_path_yields_empty() {
        let temp = TempFile::new();
        assert!(!temp.0.exists());
        let config = load_from_path(&temp.0);
        assert!(!config.is_configured());
    }

    #[test]
    fn save_then_load_round_trips() {
        let temp = TempFile::new();
        let original = sample();
        save_to_path(&temp.0, &original).expect("save");
        let loaded = load_from_path(&temp.0);
        assert_eq!(loaded, original);
    }

    #[test]
    fn parses_codex_chatgpt_auth() {
        let json = r#"{
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": { "access_token": "eyJhbGci" }
        }"#;
        assert_eq!(parse_codex_auth(json), (true, None));
    }

    #[test]
    fn parses_codex_api_key_auth() {
        let json = r#"{ "auth_mode": "apikey", "OPENAI_API_KEY": " sk-proj-123 " }"#;
        assert_eq!(
            parse_codex_auth(json),
            (false, Some("sk-proj-123".to_string()))
        );
    }

    #[test]
    fn parses_opencode_key() {
        let json = r#"{ "opencode": { "type": "api", "key": "sk-zen" } }"#;
        assert_eq!(parse_opencode_auth(json), Some("sk-zen".to_string()));
        assert_eq!(parse_opencode_auth("{}"), None);
    }

    #[test]
    fn detects_credentials_from_disk() {
        // Smoke test only: it must not panic when nothing is logged in.
        let found = detect_local_credentials();
        let _ = found.summary();
    }
}
