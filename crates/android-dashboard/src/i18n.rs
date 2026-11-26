//! Trung tâm Việt hóa tập trung cho toàn bộ giao diện `android-dashboard`.
//!
//! Nhúng sẵn `assets/i18n/vi.json` khi biên dịch và cho phép ghi đè tùy chọn
//! qua tệp `<thư_mục_exe>/i18n/vi.json` nếu người dùng muốn tùy chỉnh câu chữ.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use adb_client::DeviceState;
use adb_client::flash::StockFlashMode;
use ai_insight::InsightEngine;

const EMBEDDED_VI_JSON: &str = include_str!("../assets/i18n/vi.json");

static DICT: OnceLock<HashMap<String, String>> = OnceLock::new();

fn load_dict() -> &'static HashMap<String, String> {
    DICT.get_or_init(|| {
        let mut map: HashMap<String, String> =
            serde_json::from_str(EMBEDDED_VI_JSON).expect("embedded vi.json must be valid JSON");
        if let Some(override_path) = external_override_path() {
            if let Ok(raw) = std::fs::read_to_string(override_path) {
                if let Ok(extra) = serde_json::from_str::<HashMap<String, String>>(&raw) {
                    for (k, v) in extra {
                        if !v.trim().is_empty() {
                            map.insert(k, v);
                        }
                    }
                }
            }
        }
        map
    })
}

fn external_override_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("i18n").join("vi.json");
    candidate.is_file().then_some(candidate)
}

/// Tra chuỗi tiếng Việt theo `key`. Nếu chưa khai báo thì trả về chính `key`.
pub fn t<'a>(key: &'a str) -> &'a str {
    load_dict().get(key).map(String::as_str).unwrap_or(key)
}

/// Tra chuỗi tiếng Việt theo `key` và thay thế lần lượt các placeholder `{name}`.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let mut out = t(key).to_string();
    for (name, value) in args {
        let placeholder = format!("{{{name}}}");
        out = out.replace(&placeholder, value);
    }
    out
}

/// Việt hóa tên danh mục bộ nhớ từ `adb_client::storage_breakdown`.
pub fn storage_category(raw: &str) -> &'static str {
    match raw {
        "Photos & videos" => t("storage.cat_photos"),
        "Documents" => t("storage.cat_documents"),
        "Downloads" => t("storage.cat_downloads"),
        "Audio" => t("storage.cat_audio"),
        _ => t("storage.cat_other"),
    }
}

/// Việt hóa loại giao diện mạng từ `adb_client::network`.
pub fn network_transport(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("wifi") {
        t("network.transport_wifi").to_string()
    } else if raw.eq_ignore_ascii_case("mobile") {
        t("network.transport_mobile").to_string()
    } else if raw.eq_ignore_ascii_case("ethernet") {
        t("network.transport_ethernet").to_string()
    } else if raw.eq_ignore_ascii_case("other") {
        t("network.transport_other").to_string()
    } else {
        raw.to_string()
    }
}

/// Việt hóa trạng thái kết nối ADB của thiết bị.
pub fn device_state(state: &DeviceState) -> String {
    match state {
        DeviceState::Device => t("devices.state_device").to_string(),
        DeviceState::Offline => t("devices.state_offline").to_string(),
        DeviceState::Unauthorized => t("devices.state_unauthorized").to_string(),
        DeviceState::Other(value) => value.clone(),
    }
}

/// Việt hóa nhãn engine AI khi hiển thị lên giao diện.
pub fn insight_engine_label(engine: InsightEngine) -> &'static str {
    match engine {
        InsightEngine::Http => t("insight.engine_http"),
        InsightEngine::CodexCli => t("insight.engine_codex"),
        InsightEngine::OpenCodeCli => t("insight.engine_opencode"),
    }
}

/// Việt hóa nhãn 3 chế độ kịch bản Stock Fastboot ROM.
pub fn stock_mode_label(mode: StockFlashMode) -> &'static str {
    match mode {
        StockFlashMode::CleanAllAndLock => t("flash.stock_mode_lock"),
        StockFlashMode::CleanAll => t("flash.stock_mode_clean"),
        StockFlashMode::KeepUserData => t("flash.stock_mode_keep"),
    }
}

/// Việt hóa các thông báo lỗi chuẩn trả về từ `AdbError::user_message()`.
pub fn adb_error_vi(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed == "Device offline" {
        return t("adb_err.offline").to_string();
    }
    if trimmed == "Device unauthorized — accept the USB debugging prompt on the device" {
        return t("adb_err.unauthorized").to_string();
    }
    if trimmed == "Permission denied" {
        return t("adb_err.permission").to_string();
    }
    if trimmed == "Device not found" {
        return t("adb_err.not_found_dev").to_string();
    }
    if trimmed == "adb command failed" {
        return t("adb_err.cmd_failed").to_string();
    }
    if trimmed.starts_with("adb not found on PATH") {
        return t("adb_err.not_found").to_string();
    }
    if let Some(detail) = trimmed.strip_prefix("Failed to run adb: ") {
        return tf("adb_err.io", &[("detail", detail)]);
    }
    if let Some(detail) = trimmed.strip_prefix("Failed to parse device output: ") {
        return tf("adb_err.parse", &[("detail", detail)]);
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn embedded_vi_json_is_valid_and_non_empty() {
        let map: HashMap<String, String> =
            serde_json::from_str(EMBEDDED_VI_JSON).expect("valid embedded vi.json");
        assert!(
            map.len() >= 150,
            "expected at least 150 i18n keys, got {}",
            map.len()
        );
        for (k, v) in &map {
            assert!(!k.trim().is_empty(), "empty key found");
            assert!(!v.trim().is_empty(), "empty value for key {k}");
        }
    }

    #[test]
    fn tf_replaces_placeholders() {
        let s = tf("ui.footer.lines", &[("count", "42")]);
        assert_eq!(s, "Dòng: 42");
    }

    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    fn extract_quoted_keys(src: &str, fn_name: &str, out: &mut BTreeSet<String>) {
        let needle = format!("{fn_name}(\"");
        let mut idx = 0;
        while let Some(pos) = src[idx..].find(&needle) {
            let start = idx + pos + needle.len();
            if let Some(end_rel) = src[start..].find('"') {
                let key = &src[start..start + end_rel];
                if !key.is_empty() {
                    out.insert(key.to_string());
                }
                idx = start + end_rel + 1;
            } else {
                break;
            }
        }
    }

    #[test]
    fn all_code_i18n_keys_exist_in_vi_json() {
        let map: HashMap<String, String> =
            serde_json::from_str(EMBEDDED_VI_JSON).expect("valid embedded vi.json");
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src_dir, &mut files);

        let mut used_keys = BTreeSet::new();
        for file in files {
            let content = std::fs::read_to_string(&file).expect("read source file");
            extract_quoted_keys(&content, "i18n::t", &mut used_keys);
            extract_quoted_keys(&content, "i18n::tf", &mut used_keys);
            extract_quoted_keys(&content, "crate::i18n::t", &mut used_keys);
            extract_quoted_keys(&content, "crate::i18n::tf", &mut used_keys);
        }

        let missing: Vec<String> = used_keys
            .into_iter()
            .filter(|k| !map.contains_key(k))
            .collect();
        assert!(
            missing.is_empty(),
            "Missing keys in assets/i18n/vi.json: {missing:?}"
        );
    }
}
