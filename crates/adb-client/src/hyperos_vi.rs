//! HyperOS Việt Hóa diagnostics, health check, single-config MTZ pipeline,
//! XML translation memory, device strategy selector, backup/rollback & dry-run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::hyperos::{
    self, THEME_DATA_ROOT, TempDirGuard, ThemeGuardStatus, extract_archive, extract_xml_tag,
    get_setting, get_theme_guard_status, new_temp_dir, now_timestamp_label, resolve_adb_verified,
    run_adb_shell, run_command_capture, safe_serial, set_system_locale, sha1_bytes, unescape_xml,
    validate_archive_entries,
};

const EMBEDDED_THEME_TOML: &str = include_str!("../../../themes/theme.toml");

pub fn default_vi_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("android-dashboard")
        .join("hyperos-vi")
}

fn unix_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_line_kv(out: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in out.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let clean = v.trim();
            map.insert(
                k.trim().to_string(),
                if clean == "null" {
                    String::new()
                } else {
                    clean.to_string()
                },
            );
        }
    }
    map
}

// ---------------------------------------------------------------------------
// 3. Single-config theme.toml Pipeline & MTZ Validator
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeEntryConfig {
    pub title: String,
    pub author: String,
    pub designer: String,
    pub version: String,
    pub ui_version: i64,
    pub editor_version: String,
    pub miui_adapter_version: String,
    pub resource_type: String,
    pub is_third_parties: bool,
}

impl Default for ThemeEntryConfig {
    fn default() -> Self {
        Self {
            title: "MTZVN".to_string(),
            author: String::new(),
            designer: String::new(),
            version: "1.0".to_string(),
            ui_version: 17,
            editor_version: "1.6.5".to_string(),
            miui_adapter_version: "3.3".to_string(),
            resource_type: "theme".to_string(),
            is_third_parties: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeTomlConfig {
    pub defaults: ThemeEntryConfig,
    pub themes: BTreeMap<String, ThemeEntryConfig>,
}

fn unquote_toml_value(raw: &str) -> String {
    let s = raw.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

fn apply_toml_field(cfg: &mut ThemeEntryConfig, key: &str, val: &str) -> Result<(), String> {
    match key {
        "title" => cfg.title = unquote_toml_value(val),
        "author" => cfg.author = unquote_toml_value(val),
        "designer" => cfg.designer = unquote_toml_value(val),
        "version" => cfg.version = unquote_toml_value(val),
        "ui_version" => {
            cfg.ui_version = val
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("ui_version không hợp lệ: {val}"))?;
        }
        "editor_version" => cfg.editor_version = unquote_toml_value(val),
        "miui_adapter_version" => cfg.miui_adapter_version = unquote_toml_value(val),
        "resource_type" => cfg.resource_type = unquote_toml_value(val),
        "is_third_parties" => {
            cfg.is_third_parties = match val.trim() {
                "true" => true,
                "false" => false,
                other => return Err(format!("is_third_parties không hợp lệ: {other}")),
            };
        }
        _ => {}
    }
    Ok(())
}

/// Parses `themes/theme.toml` (`[default]` + `[themes.<name>]` sections).
pub fn parse_theme_toml(input: &str) -> Result<ThemeTomlConfig, String> {
    let mut cfg = ThemeTomlConfig::default();
    let mut section = String::new();
    let mut raw_themes: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

    for line in input.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let no_comment = if let Some(idx) = stripped.find('#') {
            let before = &stripped[..idx];
            if before.matches('"').count() % 2 == 0 {
                before.trim()
            } else {
                stripped
            }
        } else {
            stripped
        };
        if no_comment.is_empty() {
            continue;
        }
        if no_comment.starts_with('[') && no_comment.ends_with(']') {
            section = no_comment[1..no_comment.len() - 1].trim().to_string();
            continue;
        }
        let Some((k, v)) = no_comment.split_once('=') else {
            return Err(format!("Dòng không hợp lệ trong theme.toml: {no_comment}"));
        };
        let key = k.trim();
        let val = v.trim();
        if section == "default" || section.is_empty() {
            apply_toml_field(&mut cfg.defaults, key, val)?;
        } else if let Some(stem) = section.strip_prefix("themes.") {
            raw_themes
                .entry(stem.trim().to_string())
                .or_default()
                .push((key.to_string(), val.to_string()));
        }
    }

    for (stem, kvs) in raw_themes {
        let mut entry = cfg.defaults.clone();
        for (k, v) in kvs {
            apply_toml_field(&mut entry, &k, &v)?;
        }
        cfg.themes.insert(stem, entry);
    }

    Ok(cfg)
}

pub fn load_theme_config(themes_dir: Option<&Path>, stem: &str) -> ThemeEntryConfig {
    let disk_toml = themes_dir
        .map(|d| d.join("theme.toml"))
        .filter(|p| p.is_file())
        .and_then(|p| fs::read_to_string(p).ok());
    let parsed = disk_toml
        .as_deref()
        .and_then(|s| parse_theme_toml(s).ok())
        .or_else(|| parse_theme_toml(EMBEDDED_THEME_TOML).ok())
        .unwrap_or_default();
    parsed
        .themes
        .get(stem)
        .cloned()
        .unwrap_or(parsed.defaults)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn generate_description_xml(cfg: &ThemeEntryConfig, module_version: &str) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    xml.push_str("<MIUI-Theme>\n");
    xml.push_str(&format!("\t<title>{}</title>\n", escape_xml(&cfg.title)));
    xml.push_str(&format!("\t<designer>{}</designer>\n", escape_xml(&cfg.designer)));
    xml.push_str(&format!("\t<author>{}</author>\n", escape_xml(&cfg.author)));
    xml.push_str(&format!("\t<version>{}</version>\n", escape_xml(&cfg.version)));
    xml.push_str(&format!("\t<uiVersion>{}</uiVersion>\n", cfg.ui_version));
    xml.push_str(&format!(
        "\t<editorVersion>{}</editorVersion>\n",
        escape_xml(&cfg.editor_version)
    ));
    xml.push_str(&format!(
        "\t<resourceType>{}</resourceType>\n",
        escape_xml(&cfg.resource_type)
    ));
    if !module_version.trim().is_empty() {
        xml.push_str(&format!(
            "\t<moduleVersion>{}</moduleVersion>\n",
            escape_xml(module_version.trim())
        ));
    }
    xml.push_str(&format!(
        "\t<miuiAdapterVersion>{}</miuiAdapterVersion>\n",
        escape_xml(&cfg.miui_adapter_version)
    ));
    xml.push_str("</MIUI-Theme>\n");
    xml
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MtzValidationReport {
    pub ok: bool,
    pub mtz_path: String,
    pub title: String,
    pub author: String,
    pub designer: String,
    pub ui_version: String,
    pub module_count: usize,
    pub checks: Vec<ValidationCheck>,
}

pub fn validate_mtz_archive(mtz_path: &Path, cfg: &ThemeEntryConfig) -> MtzValidationReport {
    let mut checks = Vec::new();
    let path_str = mtz_path.display().to_string();

    let archive_ok = validate_archive_entries(mtz_path);
    checks.push(ValidationCheck {
        name: "Cấu trúc nén & đường dẫn an toàn".into(),
        passed: archive_ok.is_ok(),
        detail: match &archive_ok {
            Ok(()) => "Hợp lệ, không trùng tên hay path traversal".into(),
            Err(e) => e.clone(),
        },
    });
    if archive_ok.is_err() {
        return MtzValidationReport {
            ok: false,
            mtz_path: path_str,
            title: String::new(),
            author: String::new(),
            designer: String::new(),
            ui_version: String::new(),
            module_count: 0,
            checks,
        };
    }

    let Ok(unpack_dir) = new_temp_dir("mtz_validate") else {
        checks.push(ValidationCheck {
            name: "Giải nén kiểm tra".into(),
            passed: false,
            detail: "Không tạo được thư mục tạm".into(),
        });
        return MtzValidationReport {
            ok: false,
            mtz_path: path_str,
            title: String::new(),
            author: String::new(),
            designer: String::new(),
            ui_version: String::new(),
            module_count: 0,
            checks,
        };
    };
    let _guard = TempDirGuard(unpack_dir.clone());
    if let Err(e) = extract_archive(mtz_path, &unpack_dir) {
        checks.push(ValidationCheck {
            name: "Giải nén MTZ".into(),
            passed: false,
            detail: e,
        });
        return MtzValidationReport {
            ok: false,
            mtz_path: path_str,
            title: String::new(),
            author: String::new(),
            designer: String::new(),
            ui_version: String::new(),
            module_count: 0,
            checks,
        };
    }

    let desc_path = unpack_dir.join("description.xml");
    let desc_str = fs::read_to_string(&desc_path).unwrap_or_default();
    let has_xml_root = desc_str.contains("<MIUI-Theme>") && desc_str.contains("</MIUI-Theme>");
    checks.push(ValidationCheck {
        name: "XML description.xml hợp lệ".into(),
        passed: desc_path.is_file() && has_xml_root,
        detail: if has_xml_root {
            "Có thẻ gốc <MIUI-Theme> hợp lệ".into()
        } else {
            "Thiếu file description.xml hoặc sai cấu trúc <MIUI-Theme>".into()
        },
    });

    let title = extract_xml_tag(&desc_str, "title").unwrap_or_default();
    let author = extract_xml_tag(&desc_str, "author").unwrap_or_default();
    let designer = extract_xml_tag(&desc_str, "designer").unwrap_or_default();
    let ui_version = extract_xml_tag(&desc_str, "uiVersion").unwrap_or_default();

    let clean_author = !author.to_ascii_lowercase().contains("truong")
        && !designer.to_ascii_lowercase().contains("truong")
        && author == cfg.author
        && designer == cfg.designer;
    checks.push(ValidationCheck {
        name: "Không sót tên author/designer cũ".into(),
        passed: clean_author,
        detail: format!(
            "author='{}', designer='{}' (chuẩn theme.toml: '{}')",
            author, designer, cfg.author
        ),
    });

    let mut module_count = 0usize;
    if let Ok(rd) = fs::read_dir(&unpack_dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_file() && ent.file_name().to_string_lossy() != "description.xml" {
                module_count += 1;
            }
        }
    }
    checks.push(ValidationCheck {
        name: "Có module overlay trong MTZ".into(),
        passed: module_count > 0 && !title.trim().is_empty(),
        detail: format!("{module_count} module, title='{title}', uiVersion='{ui_version}'"),
    });

    let ok = checks.iter().all(|c| c.passed);
    MtzValidationReport {
        ok,
        mtz_path: path_str,
        title,
        author,
        designer,
        ui_version,
        module_count,
        checks,
    }
}

pub fn rebuild_mtz_description_from_config(
    mtz_path: &Path,
    cfg: &ThemeEntryConfig,
) -> Result<MtzValidationReport, String> {
    if !mtz_path.is_file() {
        return Err(format!("Không tìm thấy tệp MTZ: {}", mtz_path.display()));
    }
    let unpack_dir = new_temp_dir("mtz_rebuild_desc")?;
    let _guard = TempDirGuard(unpack_dir.clone());
    extract_archive(mtz_path, &unpack_dir)?;

    let existing_desc = fs::read_to_string(unpack_dir.join("description.xml")).unwrap_or_default();
    let module_version = extract_xml_tag(&existing_desc, "moduleVersion").unwrap_or_default();
    let new_xml = generate_description_xml(cfg, &module_version);
    let xml_tmp = unpack_dir.join("description.xml");
    fs::write(&xml_tmp, new_xml.as_bytes())
        .map_err(|e| format!("Không ghi được description.xml mới: {e}"))?;

    #[cfg(windows)]
    {
        let mtz_esc = mtz_path.display().to_string().replace('\'', "''");
        let xml_esc = xml_tmp.display().to_string().replace('\'', "''");
        let ps = format!(
            "Add-Type -AssemblyName System.IO.Compression; \
             Add-Type -AssemblyName System.IO.Compression.FileSystem; \
             $zip = [System.IO.Compression.ZipFile]::Open('{mtz_esc}', [System.IO.Compression.ZipArchiveMode]::Update); \
             try {{ \
               $old = $zip.GetEntry('description.xml'); if ($old) {{ $old.Delete() }}; \
               $ent = $zip.CreateEntry('description.xml'); \
               $bytes = [System.IO.File]::ReadAllBytes('{xml_esc}'); \
               $st = $ent.Open(); $st.Write($bytes, 0, $bytes.Length); $st.Dispose(); \
             }} finally {{ $zip.Dispose() }}"
        );
        let args = vec!["-NoProfile".to_string(), "-Command".to_string(), ps];
        let (ok, out) = run_command_capture("powershell", &args, Duration::from_secs(60))?;
        if !ok {
            return Err(format!("Cập nhật description.xml thất bại: {out}"));
        }
    }
    #[cfg(not(windows))]
    {
        let args = vec![
            "-uj".to_string(),
            mtz_path.display().to_string(),
            xml_tmp.display().to_string(),
        ];
        let (ok, out) = run_command_capture("zip", &args, Duration::from_secs(60))?;
        if !ok {
            return Err(format!("Cập nhật description.xml thất bại (zip): {out}"));
        }
    }

    Ok(validate_mtz_archive(mtz_path, cfg))
}

// ---------------------------------------------------------------------------
// 1. Revert Diagnostics (Snapshot + ThemeManager Logcat + Diff Classifier)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayFileDigest {
    pub name: String,
    pub size_bytes: u64,
    pub sha1: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThemeLocaleSnapshot {
    pub id: String,
    pub label: String,
    pub captured_at_epoch: u64,
    pub system_locales: String,
    pub persist_sys_locale: String,
    pub global_config_line: String,
    pub overlay_files: Vec<OverlayFileDigest>,
    pub overlay_digest: String,
    pub rights_files: Vec<String>,
    pub theme_manager_state: String,
    pub theme_manager_appops: String,
    pub scheduler_jobs: String,
    pub dumpsys_locale: String,
    pub logcat_excerpt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevertCause {
    NoRevertDetected,
    ThemeOverlaysWiped,
    LocaleReverted,
    BothReverted,
}

impl RevertCause {
    pub fn label(self) -> &'static str {
        match self {
            RevertCause::NoRevertDetected => "Không phát hiện revert",
            RevertCause::ThemeOverlaysWiped => "ThemeManager đã xóa overlay Việt hóa trong /data/system/theme",
            RevertCause::LocaleReverted => "Ngôn ngữ system_locales / persist.sys.locale bị rơi về Anh/Trung",
            RevertCause::BothReverted => "Cả Theme overlay lẫn Locale đều bị quay về gốc",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevertDiffReport {
    pub cause: RevertCause,
    pub before_label: String,
    pub after_label: String,
    pub locale_changed: bool,
    pub persist_locale_changed: bool,
    pub missing_overlays: Vec<String>,
    pub changed_overlays: Vec<String>,
    pub added_overlays: Vec<String>,
    pub summary: String,
}

pub fn compute_overlays_digest(files: &[OverlayFileDigest]) -> String {
    let mut sorted = files.to_vec();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut buf = String::new();
    for f in sorted {
        buf.push_str(&format!("{}:{}:{}\n", f.name, f.size_bytes, f.sha1));
    }
    sha1_bytes(buf.as_bytes())
}

fn snapshots_dir(serial: &str) -> PathBuf {
    default_vi_data_dir()
        .join("snapshots")
        .join(safe_serial(serial))
}

fn parse_snapshot_output(out: &str, id: &str, label: &str, now: u64) -> ThemeLocaleSnapshot {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Sec {
        None,
        Meta,
        Overlays,
        Rights,
        PkgState,
        Jobs,
        Locale,
        Logcat,
    }

    let mut snap = ThemeLocaleSnapshot {
        id: id.to_string(),
        label: label.to_string(),
        captured_at_epoch: now,
        ..Default::default()
    };
    let mut sec = Sec::None;
    let mut pkg_lines = Vec::new();
    let mut job_lines = Vec::new();
    let mut loc_lines = Vec::new();
    let mut log_lines = Vec::new();

    for line in out.lines() {
        let trimmed = line.trim_end_matches('\r');
        match trimmed.trim() {
            "===META===" => {
                sec = Sec::Meta;
                continue;
            }
            "===OVERLAYS===" => {
                sec = Sec::Overlays;
                continue;
            }
            "===RIGHTS===" => {
                sec = Sec::Rights;
                continue;
            }
            "===PKG===" => {
                sec = Sec::PkgState;
                continue;
            }
            "===JOBS===" => {
                sec = Sec::Jobs;
                continue;
            }
            "===LOCALE===" => {
                sec = Sec::Locale;
                continue;
            }
            "===LOGCAT===" => {
                sec = Sec::Logcat;
                continue;
            }
            _ => {}
        }

        match sec {
            Sec::Meta => {
                if let Some((k, v)) = trimmed.split_once('=') {
                    let clean = match v.trim() {
                        "null" => String::new(),
                        other => other.to_string(),
                    };
                    match k.trim() {
                        "SYS_LOC" => snap.system_locales = clean,
                        "PERSIST_LOC" => snap.persist_sys_locale = clean,
                        "CFG_LINE" => snap.global_config_line = clean,
                        "APPOPS" => snap.theme_manager_appops = clean,
                        _ => {}
                    }
                }
            }
            Sec::Overlays => {
                let parts: Vec<&str> = trimmed.split('|').collect();
                if parts.len() >= 3 {
                    let name = parts[0].trim().to_string();
                    let size_bytes = parts[1].trim().parse::<u64>().unwrap_or(0);
                    let sha1 = parts[2].trim().to_string();
                    if !name.is_empty() {
                        snap.overlay_files.push(OverlayFileDigest {
                            name,
                            size_bytes,
                            sha1,
                        });
                    }
                }
            }
            Sec::Rights => {
                let r = trimmed.trim();
                if !r.is_empty() {
                    snap.rights_files.push(r.to_string());
                }
            }
            Sec::PkgState => pkg_lines.push(trimmed.to_string()),
            Sec::Jobs => job_lines.push(trimmed.to_string()),
            Sec::Locale => loc_lines.push(trimmed.to_string()),
            Sec::Logcat => log_lines.push(trimmed.to_string()),
            Sec::None => {}
        }
    }

    snap.overlay_files.sort_by(|a, b| a.name.cmp(&b.name));
    snap.overlay_digest = compute_overlays_digest(&snap.overlay_files);
    snap.theme_manager_state = pkg_lines.join("\n").trim().to_string();
    snap.scheduler_jobs = job_lines.join("\n").trim().to_string();
    snap.dumpsys_locale = loc_lines.join("\n").trim().to_string();
    snap.logcat_excerpt = log_lines.join("\n").trim().to_string();
    snap
}

pub fn capture_revert_snapshot(
    adb_bin: Option<&str>,
    serial: &str,
    label: &str,
    mut on_log: impl FnMut(String),
) -> Result<(ThemeLocaleSnapshot, PathBuf), String> {
    let adb = resolve_adb_verified(adb_bin);
    let script = r#"
echo "===META==="
echo "SYS_LOC=$(settings get system system_locales 2>/dev/null)"
echo "PERSIST_LOC=$(getprop persist.sys.locale 2>/dev/null)"
echo "CFG_LINE=$(dumpsys activity 2>/dev/null | grep -m1 mGlobalConfiguration)"
echo "APPOPS=$(cmd appops get com.android.thememanager RUN_ANY_IN_BACKGROUND 2>/dev/null | tr '\n' ' ')"
echo "===OVERLAYS==="
for f in /data/system/theme/*; do
  [ -f "$f" ] || continue
  bn=$(basename "$f")
  sz=$(wc -c < "$f" 2>/dev/null || echo 0)
  sh=$(sha1sum "$f" 2>/dev/null | cut -d' ' -f1)
  echo "$bn|$sz|$sh"
done
echo "===RIGHTS==="
ls -1 /data/system/theme/rights 2>/dev/null || true
echo "===PKG==="
dumpsys package com.android.thememanager 2>/dev/null | grep -E 'enabled=|suspended=|stopped=' | head -n 6
echo "===JOBS==="
dumpsys jobscheduler 2>/dev/null | grep -iE 'thememanager|ThemeScheduler|ThemeResource' | head -n 20
echo "===LOCALE==="
dumpsys locale 2>/dev/null | head -n 40
echo "===LOGCAT==="
logcat -d -v time -t 300 2>/dev/null | grep -iE 'thememanager|ThemeManager|ThemeScheduler|ThemeResource|drm|rights|Locale|system_locales|themeChanged' | tail -n 120
"#;
    let (ok, out) = run_adb_shell(&adb, serial, script, Duration::from_secs(25))?;
    if !ok && !out.contains("===META===") {
        return Err(format!("Không chụp được snapshot chẩn đoán: {out}"));
    }

    let now = unix_epoch_secs();
    let safe_lbl: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let id = format!(
        "{}_{}",
        now,
        if safe_lbl.is_empty() {
            "snap"
        } else {
            &safe_lbl
        }
    );
    let snap = parse_snapshot_output(&out, &id, label, now);

    let dir = snapshots_dir(serial);
    fs::create_dir_all(&dir).map_err(|e| format!("Không tạo thư mục snapshots: {e}"))?;
    let json_path = dir.join(format!("{id}.json"));
    let log_path = dir.join(format!("{id}_logcat.txt"));
    let bytes = serde_json::to_vec_pretty(&snap).map_err(|e| e.to_string())?;
    fs::write(&json_path, bytes).map_err(|e| format!("Không lưu được snapshot JSON: {e}"))?;
    let _ = fs::write(&log_path, &snap.logcat_excerpt);

    on_log(format!(
        "[Snapshot] Đã lưu '{}' ({} file overlay, system_locales={}, logcat {} dòng) -> {}",
        snap.label,
        snap.overlay_files.len(),
        snap.system_locales,
        snap.logcat_excerpt.lines().count(),
        json_path.display()
    ));
    Ok((snap, json_path))
}

pub fn list_snapshots(serial: &str) -> Vec<(PathBuf, ThemeLocaleSnapshot)> {
    let dir = snapshots_dir(serial);
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(txt) = fs::read_to_string(&p) {
                    if let Ok(s) = serde_json::from_str::<ThemeLocaleSnapshot>(&txt) {
                        out.push((p, s));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.1.captured_at_epoch.cmp(&a.1.captured_at_epoch));
    out
}

pub fn compare_snapshots(
    before: &ThemeLocaleSnapshot,
    after: &ThemeLocaleSnapshot,
) -> RevertDiffReport {
    let before_map: BTreeMap<&str, &OverlayFileDigest> = before
        .overlay_files
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();
    let after_map: BTreeMap<&str, &OverlayFileDigest> = after
        .overlay_files
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();

    let mut missing_overlays = Vec::new();
    let mut changed_overlays = Vec::new();
    let mut added_overlays = Vec::new();

    for (name, bf) in &before_map {
        match after_map.get(name) {
            None => missing_overlays.push((*name).to_string()),
            Some(af) if af.sha1 != bf.sha1 || af.size_bytes != bf.size_bytes => {
                changed_overlays.push((*name).to_string());
            }
            _ => {}
        }
    }
    for name in after_map.keys() {
        if !before_map.contains_key(name) {
            added_overlays.push((*name).to_string());
        }
    }

    let locale_changed = before.system_locales != after.system_locales;
    let persist_locale_changed = before.persist_sys_locale != after.persist_sys_locale;
    let overlay_lost = !missing_overlays.is_empty()
        || (!before.overlay_files.is_empty() && after.overlay_files.is_empty());
    let locale_reverted = locale_changed
        && (after.system_locales.is_empty() || !after.system_locales.to_ascii_lowercase().contains("vi"));

    let cause = match (overlay_lost, locale_reverted) {
        (true, true) => RevertCause::BothReverted,
        (true, false) => RevertCause::ThemeOverlaysWiped,
        (false, true) => RevertCause::LocaleReverted,
        (false, false) => RevertCause::NoRevertDetected,
    };

    let summary = format!(
        "{} | Locale: '{}' -> '{}' | Overlay: {} -> {} tệp (mất {}, đổi {})",
        cause.label(),
        before.system_locales,
        after.system_locales,
        before.overlay_files.len(),
        after.overlay_files.len(),
        missing_overlays.len(),
        changed_overlays.len()
    );

    RevertDiffReport {
        cause,
        before_label: before.label.clone(),
        after_label: after.label.clone(),
        locale_changed,
        persist_locale_changed,
        missing_overlays,
        changed_overlays,
        added_overlays,
        summary,
    }
}

// ---------------------------------------------------------------------------
// 2. Health Check, Persistent State & 1-Click Reapply
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevertHistoryEntry {
    pub detected_at_epoch: u64,
    pub reason: String,
    pub locale_before: String,
    pub locale_after: String,
    pub overlay_count_before: usize,
    pub overlay_count_after: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceViState {
    pub last_theme_title: String,
    pub last_theme_path: String,
    pub expected_locale: String,
    pub applied_at_epoch: u64,
    pub baseline_overlay_count: usize,
    pub baseline_overlay_digest: String,
    pub revert_history: Vec<RevertHistoryEntry>,
}

#[derive(Clone, Debug)]
pub struct HealthCheckReport {
    pub checked_at_epoch: u64,
    pub current_system_locales: String,
    pub current_persist_locale: String,
    pub active_overlay_count: usize,
    pub current_overlay_digest: String,
    pub theme_guard: ThemeGuardStatus,
    pub reverted: bool,
    pub reasons: Vec<String>,
    pub state: DeviceViState,
}

fn vi_state_path(serial: &str) -> PathBuf {
    default_vi_data_dir()
        .join("state")
        .join(format!("{}.json", safe_serial(serial)))
}

pub fn load_device_vi_state(serial: &str) -> DeviceViState {
    let p = vi_state_path(serial);
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<DeviceViState>(&s).ok())
        .unwrap_or_default()
}

pub fn save_device_vi_state(serial: &str, state: &DeviceViState) {
    let p = vi_state_path(serial);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(state) {
        let _ = fs::write(p, bytes);
    }
}

fn query_live_overlays(adb: &str, serial: &str) -> Vec<OverlayFileDigest> {
    let script = r#"for f in /data/system/theme/*; do [ -f "$f" ] || continue; bn=$(basename "$f"); sz=$(wc -c < "$f" 2>/dev/null || echo 0); sh=$(sha1sum "$f" 2>/dev/null | cut -d' ' -f1); echo "$bn|$sz|$sh"; done"#;
    let out = run_adb_shell(adb, serial, script, Duration::from_secs(15))
        .ok()
        .map(|(_, o)| o)
        .unwrap_or_default();
    let mut files = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.trim().split('|').collect();
        if parts.len() >= 3 {
            let name = parts[0].trim().to_string();
            if !name.is_empty() {
                files.push(OverlayFileDigest {
                    name,
                    size_bytes: parts[1].trim().parse().unwrap_or(0),
                    sha1: parts[2].trim().to_string(),
                });
            }
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

pub fn record_vi_applied(
    adb_bin: Option<&str>,
    serial: &str,
    theme_path: Option<&Path>,
    theme_title: Option<&str>,
    locale: &str,
) -> DeviceViState {
    let adb = resolve_adb_verified(adb_bin);
    let overlays = query_live_overlays(&adb, serial);
    let digest = compute_overlays_digest(&overlays);
    let mut state = load_device_vi_state(serial);
    if let Some(t) = theme_title.filter(|s| !s.trim().is_empty()) {
        state.last_theme_title = t.to_string();
    }
    if let Some(p) = theme_path {
        state.last_theme_path = p.display().to_string();
    }
    if !locale.trim().is_empty() {
        state.expected_locale = locale.trim().to_string();
    } else if state.expected_locale.is_empty() {
        state.expected_locale = "vi-VN,en-US".to_string();
    }
    state.applied_at_epoch = unix_epoch_secs();
    state.baseline_overlay_count = overlays.len();
    state.baseline_overlay_digest = digest;
    save_device_vi_state(serial, &state);
    state
}

pub fn check_vi_health(adb_bin: Option<&str>, serial: &str) -> Result<HealthCheckReport, String> {
    let adb = resolve_adb_verified(adb_bin);
    let (ok, meta_out) = run_adb_shell(
        &adb,
        serial,
        "echo SYS_LOC=$(settings get system system_locales 2>/dev/null); echo PERSIST_LOC=$(getprop persist.sys.locale 2>/dev/null)",
        Duration::from_secs(10),
    )?;
    if !ok && meta_out.is_empty() {
        return Err("Không đọc được trạng thái sức khỏe Việt hóa từ ADB.".into());
    }
    let kv = parse_line_kv(&meta_out);
    let sys_loc = kv.get("SYS_LOC").cloned().unwrap_or_default();
    let persist_loc = kv.get("PERSIST_LOC").cloned().unwrap_or_default();
    let overlays = query_live_overlays(&adb, serial);
    let digest = compute_overlays_digest(&overlays);
    let guard = get_theme_guard_status(Some(&adb), serial);

    let mut state = load_device_vi_state(serial);
    let now = unix_epoch_secs();
    let mut reasons = Vec::new();

    let expected_vi = state.expected_locale.to_ascii_lowercase().contains("vi")
        || !state.expected_locale.is_empty();
    if expected_vi && !sys_loc.to_ascii_lowercase().contains("vi") {
        reasons.push(format!(
            "system_locales bị đổi thành '{}' (kỳ vọng '{}')",
            if sys_loc.is_empty() { "(trống)" } else { &sys_loc },
            state.expected_locale
        ));
    }
    if state.baseline_overlay_count > 0 && overlays.is_empty() {
        reasons.push(format!(
            "Overlay Việt hóa trong /data/system/theme bị xóa sạch ({} -> 0 tệp)",
            state.baseline_overlay_count
        ));
    } else if !state.baseline_overlay_digest.is_empty()
        && !overlays.is_empty()
        && digest != state.baseline_overlay_digest
        && overlays.len() < state.baseline_overlay_count
    {
        reasons.push(format!(
            "Số tệp overlay bị giảm ({} -> {} tệp) và checksum thay đổi",
            state.baseline_overlay_count,
            overlays.len()
        ));
    }
    if state.baseline_overlay_count > 0 && !guard.locked {
        reasons.push("Khóa DRM Chủ đề đang mở — ThemeManager có thể tự hoàn tác sau 30–60 phút".into());
    }

    let reverted = !reasons.is_empty()
        && (reasons.iter().any(|r| r.contains("system_locales") || r.contains("overlay")));
    if reverted {
        let joined = reasons.join(" | ");
        let is_dup = state
            .revert_history
            .first()
            .is_some_and(|e| e.reason == joined && now.saturating_sub(e.detected_at_epoch) < 120);
        if !is_dup {
            state.revert_history.insert(
                0,
                RevertHistoryEntry {
                    detected_at_epoch: now,
                    reason: joined,
                    locale_before: state.expected_locale.clone(),
                    locale_after: sys_loc.clone(),
                    overlay_count_before: state.baseline_overlay_count,
                    overlay_count_after: overlays.len(),
                },
            );
            state.revert_history.truncate(30);
            save_device_vi_state(serial, &state);
        }
    }

    Ok(HealthCheckReport {
        checked_at_epoch: now,
        current_system_locales: sys_loc,
        current_persist_locale: persist_loc,
        active_overlay_count: overlays.len(),
        current_overlay_digest: digest,
        theme_guard: guard,
        reverted,
        reasons,
        state,
    })
}

pub fn one_click_reapply(
    adb_bin: Option<&str>,
    serial: &str,
    theme_path: Option<&Path>,
    template_path: Option<&Path>,
    locale: &str,
    dry_run: bool,
    mut on_log: impl FnMut(String),
) -> Result<String, String> {
    let target_locale = if locale.trim().is_empty() {
        "vi-VN,en-US"
    } else {
        locale.trim()
    };
    let mtz_name = theme_path
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());

    if dry_run {
        let cmds = plan_dry_run_commands("reapply", serial, target_locale, mtz_name, true);
        for c in &cmds {
            on_log(format!("[DRY-RUN] {c}"));
        }
        return Ok(format!(
            "[Dry-Run] Đã lập {} lệnh áp lại Việt hóa (chưa thực thi).",
            cmds.len()
        ));
    }

    let _ = create_vi_backup(adb_bin, serial, |l| on_log(l));
    let _ = set_system_locale(
        adb_bin,
        serial,
        target_locale,
        &hyperos::default_backups_dir(),
        |l| on_log(l),
    )?;

    if let Some(mtz) = theme_path.filter(|p| p.is_file()) {
        let staged = hyperos::deploy_mtz_via_adb(
            adb_bin,
            serial,
            mtz,
            template_path,
            true,
            true,
            |l| on_log(l),
        )?;
        let _ = record_vi_applied(
            adb_bin,
            serial,
            Some(mtz),
            Some(&staged.title),
            target_locale,
        );
        Ok(format!(
            "Đã đặt lại locale '{}' và nạp lại \"{}\" — hãy bấm [Áp dụng] trên điện thoại rồi bấm Khóa DRM!",
            target_locale, staged.title
        ))
    } else {
        let _ = record_vi_applied(adb_bin, serial, None, None, target_locale);
        Ok(format!(
            "Đã khôi phục locale '{target_locale}' và khóa ngôn ngữ per-app."
        ))
    }
}

// ---------------------------------------------------------------------------
// 4. Translation Management (XML Extractor, Version Diff, Translation Memory)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStringCatalog {
    pub app: String,
    pub entries: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppTranslationCoverage {
    pub app: String,
    pub total_keys: usize,
    pub translated_keys: usize,
    pub missing_keys: usize,
    pub coverage_pct: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppDiffSummary {
    pub app: String,
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub sample_added: Vec<String>,
    pub sample_changed: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranslationDiff {
    pub apps: Vec<AppDiffSummary>,
    pub total_added: usize,
    pub total_changed: usize,
    pub total_removed: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranslationMemory {
    pub updated_at_epoch: u64,
    pub apps: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranslationMemoryReport {
    pub source_label: String,
    pub total_apps: usize,
    pub total_keys: usize,
    pub memory_total_keys: usize,
    pub storage_path: String,
    pub per_app: Vec<AppTranslationCoverage>,
}

/// Extracts `(name, unescaped_value)` pairs from Android `strings.xml` or Xiaomi `theme_values.xml`.
pub fn parse_android_strings_xml(xml: &str) -> Vec<(String, String)> {
    let re = Regex::new(r#"(?s)<string\b[^>]*name\s*=\s*"([^"]+)"[^>]*>(.*?)</string>"#)
        .expect("string xml regex");
    let mut items = Vec::new();
    for cap in re.captures_iter(xml) {
        let name = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let val = cap
            .get(2)
            .map(|m| unescape_xml(m.as_str().trim()))
            .unwrap_or_default();
        if !name.is_empty() {
            items.push((name.to_string(), val));
        }
    }
    items
}

fn collect_xml_strings_in_dir(dir: &Path, entries: &mut BTreeMap<String, String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            collect_xml_strings_in_dir(&p, entries);
        } else if let Some(fname) = p.file_name().and_then(|n| n.to_str()) {
            let low = fname.to_ascii_lowercase();
            if low == "theme_values.xml" || low == "strings.xml" {
                if let Ok(xml) = fs::read_to_string(&p) {
                    for (k, v) in parse_android_strings_xml(&xml) {
                        entries.insert(k, v);
                    }
                }
            }
        }
    }
}

pub fn extract_strings_from_mtz(mtz_path: &Path) -> Result<Vec<AppStringCatalog>, String> {
    validate_archive_entries(mtz_path)?;
    let unpack_dir = new_temp_dir("mtz_strings")?;
    let _guard = TempDirGuard(unpack_dir.clone());
    extract_archive(mtz_path, &unpack_dir)?;

    let mut catalogs = Vec::new();
    let rd = fs::read_dir(&unpack_dir).map_err(|e| format!("Không đọc thư mục MTZ: {e}"))?;
    for ent in rd.flatten() {
        let p = ent.path();
        if !p.is_file() {
            continue;
        }
        let fname = ent.file_name().to_string_lossy().to_string();
        if fname == "description.xml" || fname == "icons" {
            continue;
        }
        if let Ok(mod_dir) = new_temp_dir("mtz_mod_str") {
            let _mod_guard = TempDirGuard(mod_dir.clone());
            if extract_archive(&p, &mod_dir).is_ok() {
                let mut entries = BTreeMap::new();
                collect_xml_strings_in_dir(&mod_dir, &mut entries);
                if !entries.is_empty() {
                    catalogs.push(AppStringCatalog {
                        app: fname,
                        entries,
                    });
                }
            }
        }
    }
    catalogs.sort_by(|a, b| a.app.cmp(&b.app));
    Ok(catalogs)
}

pub fn extract_strings_from_dir(dir: &Path) -> Result<Vec<AppStringCatalog>, String> {
    if !dir.is_dir() {
        return Err(format!("Không phải thư mục: {}", dir.display()));
    }
    let mut grouped: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let rd = fs::read_dir(dir).map_err(|e| format!("Không đọc được {}: {e}", dir.display()))?;
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            let app = ent.file_name().to_string_lossy().to_string();
            let mut entries = BTreeMap::new();
            collect_xml_strings_in_dir(&p, &mut entries);
            if !entries.is_empty() {
                grouped.insert(app, entries);
            }
        } else if let Some(fname) = p.file_name().and_then(|n| n.to_str()) {
            let low = fname.to_ascii_lowercase();
            if low == "theme_values.xml" || low == "strings.xml" {
                if let Ok(xml) = fs::read_to_string(&p) {
                    let bucket = grouped.entry("root".to_string()).or_default();
                    for (k, v) in parse_android_strings_xml(&xml) {
                        bucket.insert(k, v);
                    }
                }
            }
        }
    }
    Ok(grouped
        .into_iter()
        .map(|(app, entries)| AppStringCatalog { app, entries })
        .collect())
}

pub fn diff_string_catalogs(
    old_apps: &[AppStringCatalog],
    new_apps: &[AppStringCatalog],
) -> TranslationDiff {
    let old_map: BTreeMap<&str, &BTreeMap<String, String>> =
        old_apps.iter().map(|a| (a.app.as_str(), &a.entries)).collect();
    let new_map: BTreeMap<&str, &BTreeMap<String, String>> =
        new_apps.iter().map(|a| (a.app.as_str(), &a.entries)).collect();

    let mut all_apps: BTreeSet<&str> = BTreeSet::new();
    all_apps.extend(old_map.keys().copied());
    all_apps.extend(new_map.keys().copied());

    let mut diff = TranslationDiff::default();
    for app in all_apps {
        let old_e = old_map.get(app).copied();
        let new_e = new_map.get(app).copied();
        let mut s = AppDiffSummary {
            app: app.to_string(),
            ..Default::default()
        };

        let mut keys: BTreeSet<&str> = BTreeSet::new();
        if let Some(m) = old_e {
            keys.extend(m.keys().map(String::as_str));
        }
        if let Some(m) = new_e {
            keys.extend(m.keys().map(String::as_str));
        }

        for k in keys {
            match (old_e.and_then(|m| m.get(k)), new_e.and_then(|m| m.get(k))) {
                (None, Some(_)) => {
                    s.added += 1;
                    if s.sample_added.len() < 5 {
                        s.sample_added.push(k.to_string());
                    }
                }
                (Some(_), None) => {
                    s.removed += 1;
                }
                (Some(ov), Some(nv)) if ov != nv => {
                    s.changed += 1;
                    if s.sample_changed.len() < 5 {
                        s.sample_changed.push(k.to_string());
                    }
                }
                (Some(_), Some(_)) => {
                    s.unchanged += 1;
                }
                (None, None) => {}
            }
        }

        diff.total_added += s.added;
        diff.total_changed += s.changed;
        diff.total_removed += s.removed;
        diff.apps.push(s);
    }
    diff
}

fn translation_memory_path() -> PathBuf {
    default_vi_data_dir().join("translation_memory.json")
}

pub fn load_translation_memory() -> TranslationMemory {
    let p = translation_memory_path();
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<TranslationMemory>(&s).ok())
        .unwrap_or_default()
}

pub fn save_translation_memory(tm: &TranslationMemory) -> Result<PathBuf, String> {
    let p = translation_memory_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Không tạo thư mục TM: {e}"))?;
    }
    let bytes = serde_json::to_vec_pretty(tm).map_err(|e| e.to_string())?;
    fs::write(&p, bytes).map_err(|e| format!("Không lưu translation_memory.json: {e}"))?;
    Ok(p)
}

pub fn import_and_score_translation_memory(
    catalogs: &[AppStringCatalog],
    source_label: &str,
) -> Result<TranslationMemoryReport, String> {
    let mut tm = load_translation_memory();
    tm.updated_at_epoch = unix_epoch_secs();
    for cat in catalogs {
        let bucket = tm.apps.entry(cat.app.clone()).or_default();
        for (k, v) in &cat.entries {
            if !v.trim().is_empty() {
                bucket.insert(k.clone(), v.clone());
            }
        }
    }
    let saved_path = save_translation_memory(&tm)?;

    let mut per_app = Vec::new();
    let mut total_keys = 0usize;
    for cat in catalogs {
        let total = cat.entries.len();
        total_keys += total;
        let tm_bucket = tm.apps.get(&cat.app);
        let translated = cat
            .entries
            .keys()
            .filter(|k| {
                tm_bucket
                    .and_then(|b| b.get(*k))
                    .is_some_and(|v| !v.trim().is_empty())
            })
            .count();
        let missing = total.saturating_sub(translated);
        let coverage_pct = if total == 0 {
            100.0
        } else {
            (translated as f32 / total as f32) * 100.0
        };
        per_app.push(AppTranslationCoverage {
            app: cat.app.clone(),
            total_keys: total,
            translated_keys: translated,
            missing_keys: missing,
            coverage_pct,
        });
    }
    let memory_total_keys: usize = tm.apps.values().map(|m| m.len()).sum();

    Ok(TranslationMemoryReport {
        source_label: source_label.to_string(),
        total_apps: catalogs.len(),
        total_keys,
        memory_total_keys,
        storage_path: saved_path.display().to_string(),
        per_app,
    })
}

// ---------------------------------------------------------------------------
// 5. Device Profile, Auto Strategy Selector & Magisk Module Overlay Builder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViStrategy {
    ThemeOverlayWithGuard,
    MagiskModuleOverlay,
}

impl ViStrategy {
    pub fn label(self) -> &'static str {
        match self {
            ViStrategy::ThemeOverlayWithGuard => "Theme Overlay (.mtz) + Khóa DRM Chủ đề (Không Root)",
            ViStrategy::MagiskModuleOverlay => "Magisk Module Overlay hệ thống (Đã Root — Bền vững)",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceViProfile {
    pub serial: String,
    pub model: String,
    pub device: String,
    pub rom_version: String,
    pub android_release: String,
    pub sdk: String,
    pub region: String,
    pub hwc: String,
    pub bootloader_locked: Option<bool>,
    pub verified_boot_state: String,
    pub has_su: bool,
    pub has_magisk: bool,
    pub magisk_version: String,
    pub recommended_strategy: ViStrategy,
    pub strategy_reason: String,
}

pub fn choose_vi_strategy(
    has_su: bool,
    has_magisk: bool,
    bootloader_locked: bool,
) -> (ViStrategy, String) {
    if has_magisk || has_su {
        (
            ViStrategy::MagiskModuleOverlay,
            "Phát hiện máy có quyền Root / Magisk: ưu tiên xuất Magisk Module gắn thẳng overlay vào /system/media/theme/default/, không bị ThemeManager xóa DRM.".into(),
        )
    } else if bootloader_locked {
        (
            ViStrategy::ThemeOverlayWithGuard,
            "Bootloader đang khóa & chưa root: bắt buộc dùng luồng Theme Overlay (.mtz) + Khóa DRM Chủ đề + cmd locale (an toàn cho VNeID & Ngân hàng).".into(),
        )
    } else {
        (
            ViStrategy::ThemeOverlayWithGuard,
            "Bootloader đã mở khóa nhưng chưa root: dùng luồng Theme Overlay (.mtz) + Khóa DRM Chủ đề (hoặc cài Magisk nếu muốn chuyển sang module hệ thống).".into(),
        )
    }
}

pub fn detect_device_vi_profile(
    adb_bin: Option<&str>,
    serial: &str,
) -> Result<DeviceViProfile, String> {
    let adb = resolve_adb_verified(adb_bin);
    let script = concat!(
        "echo MODEL=$(getprop ro.product.model); ",
        "echo DEVICE=$(getprop ro.product.device); ",
        "echo ROM=$(getprop ro.build.version.incremental); ",
        "echo AND=$(getprop ro.build.version.release); ",
        "echo SDK=$(getprop ro.build.version.sdk); ",
        "echo REG=$(getprop ro.miui.region); ",
        "echo HWC=$(getprop ro.boot.hwc); ",
        "echo LOCK=$(getprop ro.boot.flash.locked); ",
        "echo VBOOT=$(getprop ro.boot.verifiedbootstate); ",
        "echo SU_BIN=$(which su 2>/dev/null); ",
        "echo MAG_BIN=$(which magisk ksud apd 2>/dev/null | head -n 1); ",
        "echo MAG_VER=$(magisk -v 2>/dev/null || true)"
    );
    let (ok, out) = run_adb_shell(&adb, serial, script, Duration::from_secs(15))?;
    if !ok && out.is_empty() {
        return Err("Không đọc được Device Profile qua ADB.".into());
    }
    let kv = parse_line_kv(&out);
    let has_su = !kv.get("SU_BIN").map_or(true, String::is_empty);
    let has_magisk = !kv.get("MAG_BIN").map_or(true, String::is_empty)
        || !kv.get("MAG_VER").map_or(true, String::is_empty);
    let bootloader_locked = match kv.get("LOCK").map(String::as_str) {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    };
    let (strategy, reason) =
        choose_vi_strategy(has_su, has_magisk, bootloader_locked.unwrap_or(true));

    Ok(DeviceViProfile {
        serial: serial.to_string(),
        model: kv.get("MODEL").cloned().unwrap_or_default(),
        device: kv.get("DEVICE").cloned().unwrap_or_default(),
        rom_version: kv.get("ROM").cloned().unwrap_or_default(),
        android_release: kv.get("AND").cloned().unwrap_or_default(),
        sdk: kv.get("SDK").cloned().unwrap_or_default(),
        region: kv.get("REG").cloned().unwrap_or_default(),
        hwc: kv.get("HWC").cloned().unwrap_or_default(),
        bootloader_locked,
        verified_boot_state: kv.get("VBOOT").cloned().unwrap_or_default(),
        has_su,
        has_magisk,
        magisk_version: kv.get("MAG_VER").cloned().unwrap_or_default(),
        recommended_strategy: strategy,
        strategy_reason: reason,
    })
}

pub fn export_magisk_vi_module(
    mtz_path: &Path,
    out_zip: &Path,
    cfg: &ThemeEntryConfig,
) -> Result<PathBuf, String> {
    validate_archive_entries(mtz_path)?;
    let unpack_dir = new_temp_dir("mtz_magisk_unpack")?;
    let _unpack_guard = TempDirGuard(unpack_dir.clone());
    extract_archive(mtz_path, &unpack_dir)?;

    let stage = new_temp_dir("magisk_vi_stage")?;
    let _stage_guard = TempDirGuard(stage.clone());
    let overlay_dst = stage
        .join("system")
        .join("media")
        .join("theme")
        .join("default");
    fs::create_dir_all(&overlay_dst).map_err(|e| format!("Không tạo thư mục module: {e}"))?;

    let mut copied = 0usize;
    let rd = fs::read_dir(&unpack_dir).map_err(|e| e.to_string())?;
    for ent in rd.flatten() {
        let p = ent.path();
        if !p.is_file() {
            continue;
        }
        let fname = ent.file_name().to_string_lossy().to_string();
        if fname == "description.xml" {
            continue;
        }
        let dst_name = match fname.as_str() {
            "com.android.systemui" => "com.android.systemui",
            _ => fname.as_str(),
        };
        fs::copy(&p, overlay_dst.join(dst_name))
            .map_err(|e| format!("Không copy module {fname}: {e}"))?;
        copied += 1;
    }
    if copied == 0 {
        return Err("MTZ không có module overlay để đóng gói Magisk Module.".into());
    }

    let module_prop = format!(
        "id=hyperos_vi_overlay\n\
         name=HyperOS Việt Hóa Overlay ({})\n\
         version={}\n\
         versionCode=100\n\
         author={}\n\
         description=Tự động gắn overlay Việt hóa vào /system/media/theme/default và giữ locale vi-VN,en-US.\n",
        cfg.title,
        cfg.version,
        if cfg.author.is_empty() { "HyperOS VI" } else { &cfg.author }
    );
    fs::write(stage.join("module.prop"), module_prop.as_bytes())
        .map_err(|e| format!("Không ghi module.prop: {e}"))?;

    let service_sh = "#!/system/bin/sh\n\
        until [ \"$(getprop sys.boot_completed)\" = \"1\" ]; do sleep 2; done\n\
        resetprop persist.sys.locale vi-VN\n\
        settings put system system_locales vi-VN,en-US\n";
    fs::write(stage.join("service.sh"), service_sh.as_bytes())
        .map_err(|e| format!("Không ghi service.sh: {e}"))?;

    let customize_sh = "#!/system/bin/sh\n\
        ui_print \"Installing HyperOS Viet Hoa Overlay Module\"\n\
        set_perm_recursive \"$MODPATH/system\" 0 0 0755 0644\n\
        set_perm \"$MODPATH/service.sh\" 0 0 0755\n";
    fs::write(stage.join("customize.sh"), customize_sh.as_bytes())
        .map_err(|e| format!("Không ghi customize.sh: {e}"))?;

    if let Some(parent) = out_zip.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Không tạo thư mục đích: {e}"))?;
    }
    if out_zip.exists() {
        let _ = fs::remove_file(out_zip);
    }

    #[cfg(windows)]
    {
        let src_esc = stage.display().to_string().replace('\'', "''");
        let dst_esc = out_zip.display().to_string().replace('\'', "''");
        let ps = format!("Compress-Archive -Path '{src_esc}\\*' -DestinationPath '{dst_esc}' -Force");
        let args = vec!["-NoProfile".to_string(), "-Command".to_string(), ps];
        let (ok, out) = run_command_capture("powershell", &args, Duration::from_secs(60))?;
        if !ok || !out_zip.is_file() {
            return Err(format!("Đóng gói Magisk Module thất bại: {out}"));
        }
    }
    #[cfg(not(windows))]
    {
        let args = vec![
            "-rq".to_string(),
            out_zip.display().to_string(),
            ".".to_string(),
        ];
        let mut cmd = std::process::Command::new("zip");
        cmd.args(&args).current_dir(&stage);
        let status = cmd.status().map_err(|e| format!("Không chạy được zip: {e}"))?;
        if !status.success() {
            return Err("Đóng gói Magisk Module thất bại (zip).".into());
        }
    }

    Ok(out_zip.to_path_buf())
}

// ---------------------------------------------------------------------------
// 6. Backup, 1-Click Rollback & Dry-Run Planner
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViBackupRecord {
    pub id: String,
    pub created_at_epoch: u64,
    pub system_locales: String,
    pub persist_sys_locale: String,
    pub millet_raw: String,
    pub theme_guard_locked: bool,
    pub theme_guard_frozen: bool,
    pub deployed_mrm_files: Vec<String>,
    pub backup_dir: String,
}

fn vi_backups_dir(serial: &str) -> PathBuf {
    default_vi_data_dir()
        .join("backups")
        .join(safe_serial(serial))
}

pub fn plan_dry_run_commands(
    action: &str,
    serial: &str,
    locale: &str,
    mtz_name: Option<&str>,
    freeze_theme: bool,
) -> Vec<String> {
    let mut cmds = Vec::new();
    match action {
        "locale" => {
            cmds.push(format!("adb -s {serial} shell settings get system system_locales"));
            cmds.push(format!(
                "adb -s {serial} shell settings put system system_locales '{locale}'"
            ));
            if locale.to_ascii_lowercase().starts_with("vi") {
                cmds.push(format!(
                    "adb -s {serial} shell \"for p in $(pm list packages -3 --user 0 | sed 's/^package://'); do cmd locale set-app-locales $p --locales vi-VN; done\""
                ));
            }
        }
        "deploy_mtz" => {
            let name = mtz_name.unwrap_or("<selected>.mtz");
            cmds.push(format!("[PC] Kiểm tra & đóng gói {name} theo themes/theme.toml -> .mrm/.mrc"));
            cmds.push(format!(
                "adb -s {serial} push <stage_dir> /data/local/tmp/mtz_stage_<uuid>"
            ));
            cmds.push(format!(
                "adb -s {serial} shell cp & sha1sum verify -> {THEME_DATA_ROOT}"
            ));
            cmds.push(format!(
                "adb -s {serial} shell am start -S --user 0 -n com.android.thememanager/.module.detail.view.ThemeDetailActivity ..."
            ));
        }
        "lock_guard" => {
            cmds.push(format!(
                "adb -s {serial} shell cmd jobscheduler cancel com.android.thememanager"
            ));
            cmds.push(format!(
                "adb -s {serial} shell cmd appops set com.android.thememanager RUN_ANY_IN_BACKGROUND ignore"
            ));
            cmds.push(format!(
                "adb -s {serial} shell cmd netpolicy add restrict-background-blacklist <thememanager_uid>"
            ));
            cmds.push(format!(
                "adb -s {serial} shell am force-stop com.android.thememanager"
            ));
            if freeze_theme {
                cmds.push(format!(
                    "adb -s {serial} shell pm disable-user --user 0 com.android.thememanager"
                ));
                cmds.push(format!(
                    "adb -s {serial} shell pm disable-user --user 0 com.miui.themestore"
                ));
            }
        }
        "reapply" => {
            cmds.extend(plan_dry_run_commands("locale", serial, locale, mtz_name, freeze_theme));
            cmds.extend(plan_dry_run_commands("deploy_mtz", serial, locale, mtz_name, freeze_theme));
            cmds.push(format!(
                "[Hướng dẫn] Sau khi bấm [Áp dụng] trên điện thoại -> chạy bước lock_guard"
            ));
        }
        _ => {
            cmds.push(format!(
                "adb -s {serial} shell settings put system system_locales '{locale}'"
            ));
        }
    }
    cmds
}

pub fn create_vi_backup(
    adb_bin: Option<&str>,
    serial: &str,
    mut on_log: impl FnMut(String),
) -> Result<ViBackupRecord, String> {
    let adb = resolve_adb_verified(adb_bin);
    let ts = now_timestamp_label();
    let epoch = unix_epoch_secs();
    let id = format!("vi_bak_{ts}");

    let sys_loc = get_setting(&adb, serial, "system", "system_locales");
    let millet_raw = get_setting(&adb, serial, "system", "MILLET_NO_RESTRICT_APP");
    let (_, persist_loc) = run_adb_shell(
        &adb,
        serial,
        "getprop persist.sys.locale",
        Duration::from_secs(10),
    )
    .unwrap_or((false, String::new()));
    let guard = get_theme_guard_status(Some(&adb), serial);

    let (_, mrm_out) = run_adb_shell(
        &adb,
        serial,
        &format!("ls -1 '{THEME_DATA_ROOT}/meta/theme/' 2>/dev/null || true"),
        Duration::from_secs(10),
    )
    .unwrap_or((false, String::new()));
    let deployed_mrm_files: Vec<String> = mrm_out
        .lines()
        .map(str::trim)
        .filter(|s| s.ends_with(".mrm"))
        .map(str::to_string)
        .collect();

    let dir = vi_backups_dir(serial).join(&id);
    let mrm_dir = dir.join("meta_theme");
    fs::create_dir_all(&mrm_dir).map_err(|e| format!("Không tạo được thư mục backup: {e}"))?;

    for f in &deployed_mrm_files {
        let remote = format!("{THEME_DATA_ROOT}/meta/theme/{f}");
        let local = mrm_dir.join(f);
        let args = vec![
            "-s".to_string(),
            serial.to_string(),
            "pull".to_string(),
            remote,
            local.display().to_string(),
        ];
        let _ = run_command_capture(&adb, &args, Duration::from_secs(15));
    }

    let rec = ViBackupRecord {
        id: id.clone(),
        created_at_epoch: epoch,
        system_locales: sys_loc,
        persist_sys_locale: persist_loc.trim().to_string(),
        millet_raw,
        theme_guard_locked: guard.locked,
        theme_guard_frozen: guard.frozen,
        deployed_mrm_files,
        backup_dir: dir.display().to_string(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&rec).map_err(|e| e.to_string())?;
    fs::write(dir.join("backup.json"), manifest_bytes)
        .map_err(|e| format!("Không ghi được backup.json: {e}"))?;

    on_log(format!(
        "[Backup] Đã sao lưu trạng thái '{}' (locale='{}', {} tệp .mrm).",
        rec.id,
        rec.system_locales,
        rec.deployed_mrm_files.len()
    ));
    Ok(rec)
}

pub fn list_vi_backups(serial: &str) -> Vec<ViBackupRecord> {
    let dir = vi_backups_dir(serial);
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for ent in rd.flatten() {
            let manifest = ent.path().join("backup.json");
            if manifest.is_file() {
                if let Ok(txt) = fs::read_to_string(&manifest) {
                    if let Ok(rec) = serde_json::from_str::<ViBackupRecord>(&txt) {
                        out.push(rec);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.created_at_epoch.cmp(&a.created_at_epoch));
    out
}

pub fn rollback_vi_backup(
    adb_bin: Option<&str>,
    serial: &str,
    record: &ViBackupRecord,
    dry_run: bool,
    mut on_log: impl FnMut(String),
) -> Result<String, String> {
    let loc_cmd = if record.system_locales.trim().is_empty() {
        "settings delete system system_locales".to_string()
    } else {
        format!("settings put system system_locales '{}'", record.system_locales)
    };
    let millet_cmd = if record.millet_raw.trim().is_empty() {
        "settings delete system MILLET_NO_RESTRICT_APP".to_string()
    } else {
        format!(
            "settings put system MILLET_NO_RESTRICT_APP '{}'",
            record.millet_raw
        )
    };

    if dry_run {
        on_log(format!("[DRY-RUN] adb -s {serial} shell {loc_cmd}"));
        on_log(format!("[DRY-RUN] adb -s {serial} shell {millet_cmd}"));
        on_log(format!(
            "[DRY-RUN] Khôi phục ThemeManager -> locked={}, frozen={}",
            record.theme_guard_locked, record.theme_guard_frozen
        ));
        return Ok(format!(
            "[Dry-Run] Xem trước lệnh rollback về '{}'.",
            record.id
        ));
    }

    let adb = resolve_adb_verified(adb_bin);
    let (ok1, out1) = run_adb_shell(&adb, serial, &loc_cmd, Duration::from_secs(15))?;
    if !ok1 {
        return Err(format!("Rollback system_locales thất bại: {out1}"));
    }
    let _ = run_adb_shell(&adb, serial, &millet_cmd, Duration::from_secs(15));

    if record.theme_guard_locked {
        let _ = hyperos::lock_theme_anti_revert(
            Some(&adb),
            serial,
            record.theme_guard_frozen,
            |l| on_log(l),
        );
    } else {
        let _ = hyperos::unlock_theme_manager(Some(&adb), serial, |l| on_log(l));
    }

    let msg = format!(
        "Đã rollback trạng thái về '{}' (system_locales='{}').",
        record.id,
        if record.system_locales.is_empty() {
            "(mặc định)"
        } else {
            &record.system_locales
        }
    );
    on_log(msg.clone());
    Ok(msg)
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_toml_and_generates_clean_xml() {
        let toml = r#"
# Single config
[default]
author = ""
designer = ""
version = "1.0"
ui_version = 17
editor_version = "1.6.5"
miui_adapter_version = "3.3"
resource_type = "theme"
is_third_parties = false

[themes.MTZOS4]
title = "MTZVN"
"#;
        let parsed = parse_theme_toml(toml).expect("parse theme.toml");
        let entry = parsed.themes.get("MTZOS4").expect("MTZOS4 entry");
        assert_eq!(entry.title, "MTZVN");
        assert_eq!(entry.author, "");
        assert_eq!(entry.designer, "");
        assert_eq!(entry.ui_version, 17);
        assert!(!entry.is_third_parties);

        let xml = generate_description_xml(entry, "com.android.settings:0.0.2;");
        assert!(xml.contains("<title>MTZVN</title>"));
        assert!(xml.contains("<author></author>"));
        assert!(xml.contains("<designer></designer>"));
        assert!(!xml.contains("Truong"));
    }

    #[test]
    fn validates_repo_mtz_with_theme_toml() {
        let workspace_themes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes");
        let mtz_path = workspace_themes.join("MTZOS4.mtz");
        if !mtz_path.is_file() {
            return;
        }
        let cfg = load_theme_config(Some(&workspace_themes), "MTZOS4");
        let rep = validate_mtz_archive(&mtz_path, &cfg);
        assert!(rep.ok, "MTZ validation checks failed: {:?}", rep.checks);
        assert!(rep.module_count >= 14);
        assert_eq!(rep.author, "");
    }

    #[test]
    fn extracts_strings_and_diffs_catalogs_with_tm_coverage() {
        let old_xml = r#"
<MIUI_Theme_Values>
    <string name="app_name">Cài đặt</string>
    <string name="wifi_title">WLAN</string>
    <string name="old_key">Cũ</string>
</MIUI_Theme_Values>"#;
        let new_xml = r#"
<MIUI_Theme_Values>
    <string name="app_name">Cài đặt hệ thống</string>
    <string name="wifi_title">WLAN</string>
    <string name="new_ai_title">HyperAI</string>
</MIUI_Theme_Values>"#;

        let old_entries: BTreeMap<String, String> =
            parse_android_strings_xml(old_xml).into_iter().collect();
        let new_entries: BTreeMap<String, String> =
            parse_android_strings_xml(new_xml).into_iter().collect();

        let old_cat = vec![AppStringCatalog {
            app: "com.android.settings".into(),
            entries: old_entries,
        }];
        let new_cat = vec![AppStringCatalog {
            app: "com.android.settings".into(),
            entries: new_entries,
        }];

        let diff = diff_string_catalogs(&old_cat, &new_cat);
        assert_eq!(diff.total_added, 1);
        assert_eq!(diff.total_changed, 1);
        assert_eq!(diff.total_removed, 1);
        assert_eq!(diff.apps[0].unchanged, 1);
    }

    #[test]
    fn compares_revert_snapshots_and_classifies_cause() {
        let before = ThemeLocaleSnapshot {
            id: "1".into(),
            label: "before".into(),
            system_locales: "vi-VN,en-US".into(),
            persist_sys_locale: "en-US".into(),
            overlay_files: vec![
                OverlayFileDigest {
                    name: "com.android.settings".into(),
                    size_bytes: 100,
                    sha1: "aaa".into(),
                },
                OverlayFileDigest {
                    name: "statusbar".into(),
                    size_bytes: 200,
                    sha1: "bbb".into(),
                },
            ],
            ..Default::default()
        };
        let after_wiped = ThemeLocaleSnapshot {
            id: "2".into(),
            label: "after".into(),
            system_locales: "vi-VN,en-US".into(),
            persist_sys_locale: "en-US".into(),
            overlay_files: vec![],
            ..Default::default()
        };
        let diff = compare_snapshots(&before, &after_wiped);
        assert_eq!(diff.cause, RevertCause::ThemeOverlaysWiped);
        assert_eq!(diff.missing_overlays.len(), 2);

        let after_both = ThemeLocaleSnapshot {
            system_locales: "en-US".into(),
            ..after_wiped
        };
        let diff2 = compare_snapshots(&before, &after_both);
        assert_eq!(diff2.cause, RevertCause::BothReverted);
    }

    #[test]
    fn chooses_vi_strategy_by_root_and_bootloader() {
        let (s1, _) = choose_vi_strategy(false, false, true);
        assert_eq!(s1, ViStrategy::ThemeOverlayWithGuard);

        let (s2, _) = choose_vi_strategy(true, true, false);
        assert_eq!(s2, ViStrategy::MagiskModuleOverlay);
    }

    #[test]
    fn dry_run_plan_returns_exact_adb_commands() {
        let cmds = plan_dry_run_commands("lock_guard", "15e033a2", "vi-VN,en-US", None, true);
        assert!(cmds.iter().any(|c| c.contains("jobscheduler cancel com.android.thememanager")));
        assert!(cmds.iter().any(|c| c.contains("pm disable-user --user 0 com.android.thememanager")));
    }
}
