//! Fast PC -> Android APK & split-bundle (`.apk`, `.apks`, `.xapk`, `.apkm`) installer via ADB.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::unbounded;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApkInstallOptions {
    pub replace_existing: bool,    // -r
    pub grant_permissions: bool,   // -g
    pub allow_downgrade: bool,     // -d
    pub allow_test_packages: bool, // -t
}

impl Default for ApkInstallOptions {
    fn default() -> Self {
        Self {
            replace_existing: true,
            grant_permissions: true,
            allow_downgrade: false,
            allow_test_packages: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApkKind {
    SingleApk,
    SplitBundle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApkFileInfo {
    pub path: PathBuf,
    pub file_name: String,
    pub bytes: u64,
    pub kind: ApkKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInstallReadiness {
    pub serial: String,
    pub model: String,
    pub android_version: String,
    pub abi: String,
    pub miui_adb_install_disabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApkInstallReport {
    pub file_name: String,
    pub elapsed_ms: u64,
    pub split_count: usize,
    pub raw_output: String,
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn resolve_adb(adb_bin: Option<&str>) -> String {
    crate::flash::resolve_adb(adb_bin)
}

pub fn classify_apk_path(path: &Path) -> Option<ApkKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "apk" => Some(ApkKind::SingleApk),
        "apks" | "xapk" | "apkm" => Some(ApkKind::SplitBundle),
        _ => None,
    }
}

pub fn inspect_apk_file(path: &Path) -> Result<ApkFileInfo, String> {
    if !path.is_file() {
        return Err(format!("Không tìm thấy file APK: {}", path.display()));
    }
    let kind = classify_apk_path(path).ok_or_else(|| {
        format!(
            "Định dạng không hỗ trợ (chỉ nhận .apk, .apks, .xapk, .apkm): {}",
            path.display()
        )
    })?;
    let meta = fs::metadata(path).map_err(|e| format!("Không đọc được metadata: {e}"))?;
    if meta.len() == 0 {
        return Err(format!("File APK bị rỗng (0 bytes): {}", path.display()));
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    Ok(ApkFileInfo {
        path: path.to_path_buf(),
        file_name,
        bytes: meta.len(),
        kind,
    })
}

pub fn collect_apks_from_dir(dir: &Path) -> Result<Vec<ApkFileInfo>, String> {
    if !dir.is_dir() {
        return Err(format!("Không phải thư mục hợp lệ: {}", dir.display()));
    }
    let entries = fs::read_dir(dir).map_err(|e| format!("Không mở được thư mục: {e}"))?;
    let mut items = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && classify_apk_path(&path).is_some() {
            if let Ok(info) = inspect_apk_file(&path) {
                items.push(info);
            }
        }
    }
    items.sort_by(|a, b| {
        a.file_name
            .to_ascii_lowercase()
            .cmp(&b.file_name.to_ascii_lowercase())
    });
    Ok(items)
}

pub fn build_install_flags(opts: &ApkInstallOptions) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if opts.replace_existing {
        flags.push("-r");
    }
    if opts.grant_permissions {
        flags.push("-g");
    }
    if opts.allow_downgrade {
        flags.push("-d");
    }
    if opts.allow_test_packages {
        flags.push("-t");
    }
    flags
}

fn spawn_pipe_reader(
    mut pipe: Box<dyn Read + Send>,
    tx: crossbeam_channel::Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 512];
        let mut pending = Vec::<u8>::new();
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\n' || b == b'\r' {
                            if !pending.is_empty() {
                                let line = String::from_utf8_lossy(&pending).to_string();
                                let _ = tx.send(line);
                                pending.clear();
                            }
                        } else {
                            pending.push(b);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !pending.is_empty() {
            let line = String::from_utf8_lossy(&pending).to_string();
            let _ = tx.send(line);
        }
    })
}

fn run_command_with_timeout(
    program: &str,
    args: &[String],
    timeout: Duration,
    mut on_line: impl FnMut(&str),
) -> Result<(bool, String), String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Không thể chạy '{program}': {e}"))?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let (tx, rx) = unbounded::<String>();
    let a = spawn_pipe_reader(Box::new(out), tx.clone());
    let b = spawn_pipe_reader(Box::new(err), tx);

    let start = Instant::now();
    let mut output = String::new();
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            on_line(&line);
            output.push_str(&line);
            output.push('\n');
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = a.join();
                let _ = b.join();
                return Err(format!("Lệnh '{program}' quá thời gian chờ ({}s).", timeout.as_secs()));
            }
        }
    };
    let _ = a.join();
    let _ = b.join();
    while let Ok(line) = rx.try_recv() {
        on_line(&line);
        output.push_str(&line);
        output.push('\n');
    }

    Ok((status.success(), output))
}

fn adb_getprop(adb: &str, serial: &str, prop: &str) -> Result<String, String> {
    let args = vec![
        "-s".to_string(),
        serial.to_string(),
        "shell".to_string(),
        "getprop".to_string(),
        prop.to_string(),
    ];
    let (ok, out) = run_command_with_timeout(adb, &args, Duration::from_secs(10), |_| {})?;
    if !ok {
        return Err(out.trim().to_string());
    }
    Ok(out.trim().to_string())
}

pub fn check_device_readiness(
    adb_bin: Option<&str>,
    serial: &str,
) -> Result<DeviceInstallReadiness, String> {
    if serial.trim().is_empty() {
        return Err("Chưa chọn thiết bị ADB.".into());
    }
    let adb = resolve_adb(adb_bin);
    let args = vec![
        "-s".to_string(),
        serial.to_string(),
        "shell".to_string(),
        "getprop ro.product.model; echo '---'; getprop ro.build.version.release; echo '---'; getprop ro.product.cpu.abi; echo '---'; getprop persist.security.adbinstall; echo '---'; getprop ro.miui.ui.version.name; echo '---'; getprop ro.mi.os.version.name".to_string(),
    ];
    let (ok, out) = run_command_with_timeout(&adb, &args, Duration::from_secs(12), |_| {})?;
    if !ok {
        let trimmed = out.trim();
        return Err(if trimmed.is_empty() {
            format!("Không đọc được thông tin thiết bị {serial}.")
        } else {
            trimmed.to_string()
        });
    }

    let parts: Vec<&str> = out.split("---").map(str::trim).collect();
    let model = parts.first().copied().unwrap_or("").to_string();
    let android_version = parts.get(1).copied().unwrap_or("").to_string();
    let abi = parts.get(2).copied().unwrap_or("").to_string();
    let adb_install_prop = parts.get(3).copied().unwrap_or("");
    let miui_ver = parts.get(4).copied().unwrap_or("");
    let hyper_ver = parts.get(5).copied().unwrap_or("");

    let is_xiaomi = !miui_ver.is_empty() || !hyper_ver.is_empty();
    let miui_adb_install_disabled = is_xiaomi && adb_install_prop == "0";

    Ok(DeviceInstallReadiness {
        serial: serial.to_string(),
        model: if model.is_empty() {
            serial.to_string()
        } else {
            model
        },
        android_version,
        abi,
        miui_adb_install_disabled,
    })
}

fn detect_abi_in_filename(file_name: &str) -> Option<&'static str> {
    let norm = file_name.to_ascii_lowercase().replace('-', "_");
    for (needle, canonical) in [
        ("arm64_v8a", "arm64_v8a"),
        ("armeabi_v7a", "armeabi_v7a"),
        ("x86_64", "x86_64"),
        ("armeabi", "armeabi"),
        ("x86", "x86"),
    ] {
        if norm.contains(needle) {
            return Some(canonical);
        }
    }
    None
}

pub fn filter_split_apks_for_abi(apks: &[PathBuf], device_abi: &str) -> Vec<PathBuf> {
    let dev_norm = device_abi.trim().to_ascii_lowercase().replace('-', "_");
    let mut filtered: Vec<PathBuf> = apks
        .iter()
        .filter(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match detect_abi_in_filename(&name) {
                None => true,
                Some(apk_abi) => dev_norm.is_empty() || apk_abi == dev_norm,
            }
        })
        .cloned()
        .collect();

    // Place base APK first for `adb install-multiple`.
    filtered.sort_by_key(|path| {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let rank = if name == "base.apk" {
            0
        } else if !name.starts_with("split_") && !name.contains("config.") {
            1
        } else {
            2
        };
        (rank, name)
    });
    filtered
}

pub fn translate_install_error(raw: &str) -> String {
    let upper = raw.to_ascii_uppercase();
    if upper.contains("INSTALL_FAILED_USER_RESTRICTED") {
        return "Bị chặn trên máy (INSTALL_FAILED_USER_RESTRICTED): Hãy bật 'Install via USB / Cài đặt qua USB' trong Developer Options và bấm Cho phép trên điện thoại.".into();
    }
    if upper.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        return "Phiên bản APK thấp hơn bản đang cài (INSTALL_FAILED_VERSION_DOWNGRADE): Bật tùy chọn 'Allow Downgrade (-d)' hoặc gỡ bản cũ.".into();
    }
    if upper.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE")
        || upper.contains("INSTALL_PARSE_FAILED_INCONSISTENT_CERTIFICATES")
    {
        return "Chữ ký APK khác với bản đang cài (UPDATE_INCOMPATIBLE): Hãy gỡ ứng dụng cũ trước khi cài bản này.".into();
    }
    if upper.contains("INSTALL_FAILED_NO_MATCHING_ABIS") {
        return "APK không tương thích kiến trúc CPU (NO_MATCHING_ABIS): Cần bản arm64-v8a hoặc universal.".into();
    }
    if upper.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE") {
        return "Bộ nhớ điện thoại không đủ dung lượng trống (INSUFFICIENT_STORAGE).".into();
    }
    if upper.contains("INSTALL_FAILED_OLDER_SDK") {
        return "APK yêu cầu phiên bản Android cao hơn máy hiện tại (OLDER_SDK).".into();
    }
    if upper.contains("INSTALL_FAILED_INVALID_APK") || upper.contains("INSTALL_PARSE_FAILED_NOT_APK")
    {
        return "File APK bị hỏng hoặc không hợp lệ.".into();
    }

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "Cài đặt thất bại (không có chi tiết lỗi từ ADB).".into()
    } else {
        trimmed.to_string()
    }
}

fn collect_extracted_apks(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("Không đọc được thư mục giải nén: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_extracted_apks(&path, out)?;
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("apk"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn extract_split_bundle(bundle: &Path, dest_dir: &Path) -> Result<(), String> {
    let bundle_str = bundle
        .to_str()
        .ok_or("Đường dẫn bundle không phải Unicode hợp lệ.")?
        .to_string();
    let dest_str = dest_dir
        .to_str()
        .ok_or("Đường dẫn tạm không phải Unicode hợp lệ.")?
        .to_string();

    #[cfg(windows)]
    let (prog, args) = (
        "tar",
        vec![
            "-xf".to_string(),
            bundle_str,
            "-C".to_string(),
            dest_str,
        ],
    );
    #[cfg(not(windows))]
    let (prog, args) = (
        "unzip",
        vec![
            "-o".to_string(),
            "-q".to_string(),
            bundle_str,
            "-d".to_string(),
            dest_str,
        ],
    );

    let (ok, out) = run_command_with_timeout(prog, &args, Duration::from_secs(120), |_| {})?;
    if !ok {
        return Err(format!("Không giải nén được bundle APK ({prog}): {}", out.trim()));
    }
    Ok(())
}

pub fn install_apk(
    adb_bin: Option<&str>,
    serial: &str,
    path: &Path,
    opts: &ApkInstallOptions,
    device_abi: Option<&str>,
    mut on_progress: impl FnMut(String),
) -> Result<ApkInstallReport, String> {
    if serial.trim().is_empty() || serial.chars().any(char::is_control) {
        return Err("Serial thiết bị không hợp lệ.".into());
    }
    let info = inspect_apk_file(path)?;
    let adb = resolve_adb(adb_bin);
    let flags = build_install_flags(opts);
    let started = Instant::now();

    match info.kind {
        ApkKind::SingleApk => {
            on_progress(format!("Đang cài {}…", info.file_name));
            let path_str = info
                .path
                .to_str()
                .ok_or("Đường dẫn APK không phải Unicode hợp lệ.")?
                .to_string();
            let mut args = vec!["-s".to_string(), serial.to_string(), "install".to_string()];
            args.extend(flags.into_iter().map(str::to_string));
            args.push(path_str);

            let (ok, out) =
                run_command_with_timeout(&adb, &args, Duration::from_secs(180), |line| {
                    on_progress(format!("  {line}"));
                })?;

            let has_success = out.contains("Success");
            let has_failure = out.contains("Failure [");
            if !ok || !has_success || has_failure {
                return Err(translate_install_error(&out));
            }
            Ok(ApkInstallReport {
                file_name: info.file_name,
                elapsed_ms: started.elapsed().as_millis() as u64,
                split_count: 1,
                raw_output: out,
            })
        }
        ApkKind::SplitBundle => {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let temp_dir = std::env::temp_dir().join(format!(
                "adb-apk-extract-{}-{}",
                std::process::id(),
                stamp
            ));
            fs::create_dir_all(&temp_dir)
                .map_err(|e| format!("Không tạo được thư mục tạm để giải nén: {e}"))?;
            let _guard = TempDirGuard(temp_dir.clone());

            on_progress(format!("Đang giải nén bundle {}…", info.file_name));
            extract_split_bundle(&info.path, &temp_dir)?;

            let mut extracted = Vec::new();
            collect_extracted_apks(&temp_dir, &mut extracted)?;
            if extracted.is_empty() {
                return Err(format!(
                    "Không tìm thấy file .apk nào bên trong {}.",
                    info.file_name
                ));
            }

            let resolved_abi = match device_abi.map(str::trim).filter(|s| !s.is_empty()) {
                Some(abi) => abi.to_string(),
                None => adb_getprop(&adb, serial, "ro.product.cpu.abi").unwrap_or_default(),
            };
            let selected = filter_split_apks_for_abi(&extracted, &resolved_abi);
            if selected.is_empty() {
                return Err(format!(
                    "Không có split APK nào khớp kiến trúc '{resolved_abi}' trong {}.",
                    info.file_name
                ));
            }
            let split_count = selected.len();
            on_progress(format!(
                "Đang cài {} split APK (ABI: {})…",
                split_count,
                if resolved_abi.is_empty() {
                    "auto"
                } else {
                    &resolved_abi
                }
            ));

            let mut args = vec![
                "-s".to_string(),
                serial.to_string(),
                "install-multiple".to_string(),
            ];
            args.extend(flags.into_iter().map(str::to_string));
            for p in &selected {
                let s = p
                    .to_str()
                    .ok_or("Đường dẫn split APK không phải Unicode hợp lệ.")?
                    .to_string();
                args.push(s);
            }

            let (ok, out) =
                run_command_with_timeout(&adb, &args, Duration::from_secs(180), |line| {
                    on_progress(format!("  {line}"));
                })?;

            let has_success = out.contains("Success");
            let has_failure = out.contains("Failure [");
            if !ok || !has_success || has_failure {
                return Err(translate_install_error(&out));
            }
            Ok(ApkInstallReport {
                file_name: info.file_name,
                elapsed_ms: started.elapsed().as_millis() as u64,
                split_count,
                raw_output: out,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_install_flags_correctly() {
        assert_eq!(
            build_install_flags(&ApkInstallOptions::default()),
            vec!["-r", "-g"]
        );
        let all = ApkInstallOptions {
            replace_existing: true,
            grant_permissions: true,
            allow_downgrade: true,
            allow_test_packages: true,
        };
        assert_eq!(build_install_flags(&all), vec!["-r", "-g", "-d", "-t"]);
        let none = ApkInstallOptions {
            replace_existing: false,
            grant_permissions: false,
            allow_downgrade: false,
            allow_test_packages: false,
        };
        assert!(build_install_flags(&none).is_empty());
    }

    #[test]
    fn classifies_apk_and_bundle_extensions() {
        assert_eq!(
            classify_apk_path(Path::new("app.apk")),
            Some(ApkKind::SingleApk)
        );
        assert_eq!(
            classify_apk_path(Path::new("app.APK")),
            Some(ApkKind::SingleApk)
        );
        assert_eq!(
            classify_apk_path(Path::new("app.apks")),
            Some(ApkKind::SplitBundle)
        );
        assert_eq!(
            classify_apk_path(Path::new("app.xapk")),
            Some(ApkKind::SplitBundle)
        );
        assert_eq!(
            classify_apk_path(Path::new("app.APKM")),
            Some(ApkKind::SplitBundle)
        );
        assert_eq!(classify_apk_path(Path::new("rom.zip")), None);
        assert_eq!(classify_apk_path(Path::new("boot.img")), None);
    }

    #[test]
    fn filters_split_apks_by_device_abi() {
        let input = vec![
            PathBuf::from("/tmp/split_config.armeabi_v7a.apk"),
            PathBuf::from("/tmp/split_config.arm64_v8a.apk"),
            PathBuf::from("/tmp/split_config.x86_64.apk"),
            PathBuf::from("/tmp/split_config.x86.apk"),
            PathBuf::from("/tmp/split_config.xxxhdpi.apk"),
            PathBuf::from("/tmp/base.apk"),
        ];
        let out = filter_split_apks_for_abi(&input, "arm64-v8a");
        let names: Vec<String> = out
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "base.apk",
                "split_config.arm64_v8a.apk",
                "split_config.xxxhdpi.apk"
            ]
        );
    }

    #[test]
    fn translates_common_adb_install_errors() {
        assert!(translate_install_error("adb: failed: Failure [INSTALL_FAILED_USER_RESTRICTED: Install canceled by user]").contains("Install via USB"));
        assert!(translate_install_error("Failure [INSTALL_FAILED_VERSION_DOWNGRADE]").contains("-d"));
        assert!(translate_install_error("Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: signatures do not match]").contains("Chữ ký"));
        assert!(translate_install_error("Failure [INSTALL_FAILED_NO_MATCHING_ABIS: Failed to extract native libraries]").contains("arm64-v8a"));
        assert!(translate_install_error("Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]").contains("dung lượng"));
        assert!(translate_install_error("Failure [INSTALL_FAILED_OLDER_SDK]").contains("Android"));
        assert!(translate_install_error("Failure [INSTALL_PARSE_FAILED_NOT_APK]").contains("không hợp lệ"));
    }

    #[test]
    fn collects_sorted_apks_from_directory() {
        let dir = std::env::temp_dir().join("adb-client-apk-collect-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Zalo.apk"), b"apk1").unwrap();
        fs::write(dir.join("app.xapk"), b"xapk1").unwrap();
        fs::write(dir.join("notes.txt"), b"ignore").unwrap();
        fs::write(dir.join("empty.apk"), b"").unwrap();

        let items = collect_apks_from_dir(&dir).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].file_name, "app.xapk");
        assert_eq!(items[0].kind, ApkKind::SplitBundle);
        assert_eq!(items[1].file_name, "Zalo.apk");
        assert_eq!(items[1].kind, ApkKind::SingleApk);

        let _ = fs::remove_dir_all(&dir);
    }
}
