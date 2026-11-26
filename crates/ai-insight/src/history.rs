//! Persistent JSON Lines history. Provider credentials are never serialized.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static WRITE_LOCK: Mutex<()> = Mutex::new(());
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

pub fn history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| {
        p.join("android-dashboard")
            .join("logs")
            .join("insight.jsonl")
    })
}

pub(crate) fn record(mut value: serde_json::Value) {
    let result = (|| -> std::io::Result<()> {
        let _lock = WRITE_LOCK
            .lock()
            .map_err(|_| std::io::Error::other("history lock poisoned"))?;
        let path =
            history_path().ok_or_else(|| std::io::Error::other("no local data directory"))?;
        fs::create_dir_all(path.parent().expect("history parent"))?;
        if fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_LOG_BYTES) {
            let previous = path.with_extension("previous.jsonl");
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&path, previous)?;
        }
        value["timestamp_unix_ms"] = serde_json::json!(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        );
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(&mut file, &value)?;
        file.write_all(b"\n")?;
        file.flush()
    })();
    if let Err(error) = result {
        tracing::warn!(%error, "could not persist Insight history");
    }
}
