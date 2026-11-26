//! Nhật ký kiểm toán (audit log) hợp nhất theo từng thiết bị.
//!
//! Mọi hành động làm thay đổi điện thoại (debloat, khôi phục, gỡ app, dọn
//! Zalo, fix HyperOS, flash/khóa bootloader…) đều được ghi nối tiếp thành
//! một dòng JSON (JSON Lines) vào file riêng theo serial máy:
//! `<data_local_dir>/android-dashboard/audit/audit-<serial>.jsonl`.
//!
//! Trên Windows `data_local_dir` là `%LOCALAPPDATA%`, trên macOS là
//! `~/Library/Application Support`.
//!
//! Nguyên tắc: ghi audit KHÔNG BAO GIỜ được làm hỏng hành động chính —
//! mọi lỗi ghi đều bị nuốt lặng, không panic, không unwrap.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Thư mục chứa các file audit: `android-dashboard/audit` cạnh các thư mục
/// `reports` / `flash-reports` sẵn có.
pub fn audit_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("android-dashboard/audit")
}

/// Chỉ giữ `[A-Za-z0-9._-]` trong serial khi đưa vào tên file; ký tự khác
/// (dấu `:`, `/`, khoảng trắng… trong serial emulator/mạng) thay bằng `_`.
/// Serial rỗng cho ra `"unknown"`.
pub fn sanitize_serial(serial: &str) -> String {
    let cleaned: String = serial
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// Đường dẫn file audit của một thiết bị (tiện ích công khai cho các màn
/// hình cần mở/trỏ tới file audit của máy đang chọn).
#[allow(dead_code)]
pub fn audit_path(serial: &str) -> PathBuf {
    audit_path_in(&audit_dir(), serial)
}

fn audit_path_in(dir: &Path, serial: &str) -> PathBuf {
    dir.join(format!("audit-{}.jsonl", sanitize_serial(serial)))
}

#[derive(serde::Serialize)]
struct AuditEntry<'a> {
    ts: String,
    serial: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    action: &'a str,
    target: &'a str,
    detail: &'a str,
    ok: bool,
    message: &'a str,
}

fn build_line(
    serial: &str,
    model: Option<&str>,
    action: &str,
    target: &str,
    detail: &str,
    ok: bool,
    message: &str,
) -> String {
    let entry = AuditEntry {
        ts: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string(),
        serial,
        model,
        action,
        target,
        detail,
        ok,
        message,
    };
    // Serialize một struct toàn chuỗi/số/bool không thể lỗi thực tế; nếu có
    // vẫn không được panic trên đường ghi audit.
    serde_json::to_string(&entry).unwrap_or_else(|_| "{}".to_string())
}

fn record_to(
    dir: &Path,
    serial: &str,
    model: Option<&str>,
    action: &str,
    target: &str,
    detail: &str,
    ok: bool,
    message: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = audit_path_in(dir, serial);
    let line = build_line(serial, model, action, target, detail, ok, message);
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Ghi một sự kiện audit cho thiết bị `serial`.
///
/// - `action`: mã hành động dạng `nhom.hanh_dong` (vd `debloat.remove`,
///   `app.uninstall`, `flash.stock`, `zalo.cleanup`).
/// - `target`: đối tượng chính (tên package, phân vùng, file theme…).
/// - `detail`: bối cảnh phụ (chế độ/tier, đường dẫn log chi tiết…).
/// - `ok` + `message`: kết quả và thông điệp gốc (thường là chuỗi lỗi).
///
/// Hàm này không bao giờ panic và không trả lỗi: ghi audit là phụ trợ,
/// hỏng ghi thì hành động chính vẫn phải chạy tiếp.
pub fn record(
    serial: &str,
    model: Option<&str>,
    action: &str,
    target: &str,
    detail: &str,
    ok: bool,
    message: &str,
) {
    let _ = record_to(&audit_dir(), serial, model, action, target, detail, ok, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_serial_keeps_safe_chars() {
        assert_eq!(sanitize_serial("8a1f3c9d"), "8a1f3c9d");
        assert_eq!(sanitize_serial("R5CX-12.3_ab"), "R5CX-12.3_ab");
        assert_eq!(sanitize_serial("emulator-5554"), "emulator-5554");
    }

    #[test]
    fn sanitize_serial_replaces_unsafe_chars() {
        assert_eq!(sanitize_serial("192.168.1.5:5555"), "192.168.1.5_5555");
        assert_eq!(sanitize_serial("a/b c"), "a_b_c");
        assert_eq!(sanitize_serial(""), "unknown");
    }

    #[test]
    fn record_writes_parseable_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "android-dashboard-audit-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        record_to(
            &dir,
            "dev:1234",
            Some("Xiaomi 15 Ultra"),
            "debloat.remove",
            "com.example.bloat",
            "mode=AutoDeep \"quoted\" \\ path",
            true,
            "",
        )
        .unwrap();
        record_to(
            &dir,
            "dev:1234",
            None,
            "app.uninstall",
            "com.example.app",
            "",
            false,
            "lỗi \"adb\" ngắt kết nối",
        )
        .unwrap();

        let path = dir.join("audit-dev_1234.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["serial"], "dev:1234");
        assert_eq!(first["model"], "Xiaomi 15 Ultra");
        assert_eq!(first["action"], "debloat.remove");
        assert_eq!(first["target"], "com.example.bloat");
        assert_eq!(first["detail"], "mode=AutoDeep \"quoted\" \\ path");
        assert_eq!(first["ok"], true);
        assert!(first["ts"].as_str().is_some_and(|s| s.len() >= 19));

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["ok"], false);
        assert_eq!(second["message"], "lỗi \"adb\" ngắt kết nối");
        assert!(second.get("model").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
