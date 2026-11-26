//! Explicit, reviewed Fastboot image plans, single-partition flashing, temporary boot,
//! A/B slot management, ADB sideload, and verified Stock Fastboot ROM flash/lock scripts.
use crossbeam_channel::unbounded;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Android sparse image magic header (`0xED26FF3A` little-endian).
const SPARSE_HEADER_MAGIC: [u8; 4] = [0x3A, 0xFF, 0x26, 0xED];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub product: String,
    pub build: String,
    pub mode: String,
    pub images: Vec<Image>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub partition: String,
    pub file: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub serial: String,
    pub product: String,
    pub slot: String,
    pub mode: String,
    pub unlocked: bool,
    pub slot_count: Option<u32>,
    pub battery_voltage: Option<String>,
    pub battery_soc_ok: Option<bool>,
    pub max_download_size: Option<u64>,
    pub anti_rollback: Option<String>,
    pub version_bootloader: Option<String>,
    /// Cached variables from `fastboot getvar all` to avoid repeated USB round-trips.
    pub vars: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SideloadDevice {
    pub serial: String,
    pub state: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlashOptions {
    /// Pass `--disable-verity` when flashing `vbmeta*` partitions.
    pub disable_verity: bool,
    /// Pass `--disable-verification` when flashing `vbmeta*` partitions.
    pub disable_verification: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StockFlashProgress {
    pub current_partition: Option<String>,
    pub current_chunk: Option<(u32, u32)>,
    pub completed_steps: usize,
    pub total_steps: usize,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub remaining_summary: Vec<String>,
    pub eta_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct ProgressUpdate {
    pub step: usize,
    pub total: usize,
    pub percent: Option<f32>,
    pub message: String,
    pub stock: Option<StockFlashProgress>,
}

impl ProgressUpdate {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            step: 0,
            total: 0,
            percent: None,
            message: message.into(),
            stock: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    device: Device,
    manifest: Manifest,
    files: Vec<PathBuf>,
    sizes: Vec<u64>,
}

impl Plan {
    pub fn device(&self) -> &Device {
        &self.device
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    pub fn sizes(&self) -> &[u64] {
        &self.sizes
    }
    pub fn total_bytes(&self) -> u64 {
        self.sizes.iter().sum()
    }
}

/// Resolves the `fastboot` binary path, preferring an explicit override, then `PATH`,
/// then the default Android SDK platform-tools directory on Windows.
pub fn resolve_fastboot(custom: Option<&str>) -> String {
    resolve_tool(custom, "fastboot", "fastboot.exe")
}

/// Resolves the `adb` binary path, preferring an explicit override, then `PATH`,
/// then the default Android SDK platform-tools directory on Windows.
pub fn resolve_adb(custom: Option<&str>) -> String {
    resolve_tool(custom, "adb", "adb.exe")
}

fn resolve_tool(custom: Option<&str>, default_name: &str, windows_exe: &str) -> String {
    if let Some(c) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        return c.to_string();
    }
    #[cfg(windows)]
    {
        if Command::new(default_name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return default_name.to_string();
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let sdk = PathBuf::from(local)
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join(windows_exe);
            if sdk.is_file() {
                return sdk.display().to_string();
            }
        }
    }
    let _ = windows_exe;
    default_name.to_string()
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
                                let line = String::from_utf8_lossy(&pending).trim().to_string();
                                pending.clear();
                                if !line.is_empty() {
                                    let _ = tx.send(line);
                                }
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
            let line = String::from_utf8_lossy(&pending).trim().to_string();
            if !line.is_empty() {
                let _ = tx.send(line);
            }
        }
    })
}

/// Runs a CLI tool (`fastboot` or `adb`) and streams stdout/stderr lines in real time.
pub fn run_program_streaming(
    program: &str,
    serial: Option<&str>,
    args: &[&str],
    timeout: Duration,
    fail_on_failed_text: bool,
    mut on_line: impl FnMut(&str),
) -> Result<String, String> {
    let mut cmd = Command::new(program);
    if let Some(s) = serial {
        cmd.args(["-s", s]);
    }
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
        .map_err(|e| format!("Cannot start '{program}': {e}"))?;

    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let (tx, rx) = unbounded::<String>();
    let a = spawn_pipe_reader(Box::new(out), tx.clone());
    let b = spawn_pipe_reader(Box::new(err), tx);

    let mut full_output = String::new();
    let start = Instant::now();
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            on_line(&line);
            full_output.push_str(&line);
            full_output.push('\n');
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = a.join();
                let _ = b.join();
                while let Ok(line) = rx.try_recv() {
                    on_line(&line);
                    full_output.push_str(&line);
                    full_output.push('\n');
                }
                return Err(format!(
                    "Command '{program}' timed out or could not be monitored. Device state may be unknown; do not automatically retry a write."
                ));
            }
        }
    };
    let _ = a.join();
    let _ = b.join();
    while let Ok(line) = rx.try_recv() {
        on_line(&line);
        full_output.push_str(&line);
        full_output.push('\n');
    }

    if !status.success() || (fail_on_failed_text && full_output.contains("FAILED")) {
        let msg = full_output.trim();
        return Err(if msg.is_empty() {
            format!("'{program}' exited with status {status}")
        } else {
            msg.to_string()
        });
    }
    Ok(full_output)
}

pub fn run(serial: Option<&str>, args: &[&str], timeout: Duration) -> Result<String, String> {
    let fb = resolve_fastboot(None);
    run_program_streaming(&fb, serial, args, timeout, true, |_| {})
}

pub fn run_with_bin(
    fastboot_bin: Option<&str>,
    serial: Option<&str>,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let fb = resolve_fastboot(fastboot_bin);
    run_program_streaming(&fb, serial, args, timeout, true, |_| {})
}

pub fn devices() -> Result<Vec<String>, String> {
    devices_with_bin(None)
}

pub fn devices_with_bin(fastboot_bin: Option<&str>) -> Result<Vec<String>, String> {
    Ok(
        run_with_bin(fastboot_bin, None, &["devices"], Duration::from_secs(8))?
            .lines()
            .filter_map(|line| {
                let mut w = line.split_whitespace();
                let serial = w.next()?;
                (w.next()? == "fastboot").then(|| serial.to_owned())
            })
            .collect(),
    )
}

/// Lists ADB devices currently in `sideload` or `recovery` state.
pub fn sideload_devices() -> Result<Vec<SideloadDevice>, String> {
    sideload_devices_with_bin(None)
}

pub fn sideload_devices_with_bin(adb_bin: Option<&str>) -> Result<Vec<SideloadDevice>, String> {
    let adb = resolve_adb(adb_bin);
    let out = run_program_streaming(
        &adb,
        None,
        &["devices"],
        Duration::from_secs(8),
        false,
        |_| {},
    )?;
    Ok(parse_sideload_devices(&out))
}

fn parse_sideload_devices(text: &str) -> Vec<SideloadDevice> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("List of devices")
                || trimmed.starts_with('*')
            {
                return None;
            }
            let mut parts = trimmed.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next()?;
            if state == "sideload" || state == "recovery" {
                Some(SideloadDevice {
                    serial: serial.to_owned(),
                    state: state.to_owned(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn parse_var(text: &str, key: &str) -> Result<String, String> {
    text.lines()
        .find_map(|line| {
            line.trim()
                .trim_start_matches("(bootloader)")
                .trim()
                .strip_prefix(&format!("{key}:"))
                .map(|v| v.trim().to_string())
        })
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Fastboot did not report {key}"))
}

fn parse_hex_u64(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else if s.bytes().any(|b| matches!(b, b'a'..=b'f' | b'A'..=b'F')) {
        u64::from_str_radix(s, 16).ok()
    } else {
        s.parse::<u64>()
            .ok()
            .or_else(|| u64::from_str_radix(s, 16).ok())
    }
}

pub fn var(serial: &str, key: &str) -> Result<String, String> {
    var_with_bin(None, serial, key)
}

pub fn var_with_bin(fastboot_bin: Option<&str>, serial: &str, key: &str) -> Result<String, String> {
    parse_var(
        &run_with_bin(
            fastboot_bin,
            Some(serial),
            &["getvar", key],
            Duration::from_secs(8),
        )?,
        key,
    )
}

/// Probes a Fastboot device and returns its metadata, including whether the bootloader is unlocked.
/// Unlike [`probe`], this succeeds for locked bootloaders so the UI can display device status.
pub fn probe_info(serial: &str) -> Result<Device, String> {
    probe_info_with_bin(None, serial)
}

fn parse_all_vars(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let cleaned = line.trim().trim_start_matches("(bootloader)").trim();
        if cleaned.is_empty()
            || cleaned.starts_with("Finished.")
            || cleaned.starts_with("all:")
            || cleaned.starts_with("FAILED")
        {
            continue;
        }
        if let Some((k, v)) = cleaned.rsplit_once(':') {
            let key = k.trim();
            let val = v.trim();
            if !key.is_empty() && !val.is_empty() {
                map.insert(key.to_string(), val.to_string());
            }
        }
    }
    map
}

pub fn probe_info_with_bin(fastboot_bin: Option<&str>, serial: &str) -> Result<Device, String> {
    if serial.is_empty() || !devices_with_bin(fastboot_bin)?.iter().any(|s| s == serial) {
        return Err("Selected Fastboot device is not connected".into());
    }

    // Fetch all variables in a single USB round-trip to avoid destabilizing
    // Qualcomm/Xiaomi fastboot on AMD USB 3.0 controllers.
    let vars = run_with_bin(
        fastboot_bin,
        Some(serial),
        &["getvar", "all"],
        Duration::from_secs(10),
    )
    .map(|out| parse_all_vars(&out))
    .unwrap_or_default();

    let get_opt = |key: &str| -> Option<String> {
        if !vars.is_empty() {
            vars.get(key).cloned()
        } else {
            var_with_bin(fastboot_bin, serial, key).ok()
        }
    };

    let unlocked_raw = get_opt("unlocked")
        .or_else(|| var_with_bin(fastboot_bin, serial, "unlocked").ok())
        .unwrap_or_default()
        .to_lowercase();
    let unlocked = matches!(unlocked_raw.as_str(), "yes" | "1" | "true");

    let mode = match get_opt("is-userspace").as_deref() {
        Some("yes") => "fastbootd",
        Some("no") => "bootloader",
        _ => "bootloader",
    };

    let product = match get_opt("product") {
        Some(p) => p,
        None => var_with_bin(fastboot_bin, serial, "product")?,
    };
    let slot = get_opt("current-slot").unwrap_or_default();
    let slot_count = get_opt("slot-count").and_then(|v| v.parse::<u32>().ok());
    let battery_voltage = get_opt("battery-voltage");
    let battery_soc_ok = get_opt("battery-soc-ok")
        .map(|v| matches!(v.to_lowercase().as_str(), "yes" | "true" | "1"));
    let max_download_size = get_opt("max-download-size").and_then(|v| parse_hex_u64(&v));
    let anti_rollback = get_opt("anti");
    let version_bootloader = get_opt("version-bootloader");

    Ok(Device {
        serial: serial.into(),
        product,
        slot,
        mode: mode.into(),
        unlocked,
        slot_count,
        battery_voltage,
        battery_soc_ok,
        max_download_size,
        anti_rollback,
        version_bootloader,
        vars,
    })
}

/// Probes a Fastboot device and requires an unlocked bootloader before any write operation.
pub fn probe(serial: &str) -> Result<Device, String> {
    probe_with_bin(None, serial)
}

pub fn probe_with_bin(fastboot_bin: Option<&str>, serial: &str) -> Result<Device, String> {
    let dev = probe_info_with_bin(fastboot_bin, serial)?;
    if !dev.unlocked {
        return Err("Bootloader must report unlocked: yes. This module never unlocks it.".into());
    }
    Ok(dev)
}

pub fn valid_partition(partition: &str, mode: &str) -> bool {
    let base = strip_slot_suffix(partition);
    match mode {
        "bootloader" => matches!(
            base,
            "boot"
                | "init_boot"
                | "vendor_boot"
                | "vendor_kernel_boot"
                | "dtbo"
                | "recovery"
                | "vbmeta"
                | "vbmeta_system"
                | "vbmeta_vendor"
                | "super"
        ),
        "fastbootd" => matches!(
            base,
            "system"
                | "system_ext"
                | "vendor"
                | "product"
                | "odm"
                | "vendor_dlkm"
                | "odm_dlkm"
                | "system_dlkm"
                | "boot"
                | "init_boot"
                | "vendor_boot"
                | "vendor_kernel_boot"
                | "recovery"
        ),
        _ => false,
    }
}

pub fn strip_slot_suffix(partition: &str) -> &str {
    partition
        .strip_suffix("_a")
        .or_else(|| partition.strip_suffix("_b"))
        .unwrap_or(partition)
}

pub fn is_vbmeta_partition(partition: &str) -> bool {
    strip_slot_suffix(partition).starts_with("vbmeta")
}

/// Returns `true` if the file begins with the Android sparse image magic (`0xED26FF3A`).
pub fn is_android_sparse_image(path: &Path) -> Result<bool, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut magic = [0u8; 4];
    match f.read_exact(&mut magic) {
        Ok(()) => Ok(magic == SPARSE_HEADER_MAGIC),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

pub fn hash(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_manifest(m: &Manifest) -> Result<(), String> {
    if m.product.trim().is_empty()
        || m.build.trim().is_empty()
        || m.images.is_empty()
        || m.images.len() > 32
    {
        return Err("Product, build and 1–32 images are required".into());
    }
    let mut seen = HashSet::new();
    for i in &m.images {
        if !valid_partition(&i.partition, &m.mode) || !seen.insert(&i.partition) {
            return Err(format!(
                "Unsupported or duplicate partition: {}",
                i.partition
            ));
        }
        if i.sha256.len() != 64 || !i.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("Invalid SHA-256 for {}", i.partition));
        }
    }
    Ok(())
}

pub fn prepare(serial: &str, manifest_path: &Path) -> Result<Plan, String> {
    prepare_with_progress(None, serial, manifest_path, |_| {})
}

pub fn prepare_with_progress(
    fastboot_bin: Option<&str>,
    serial: &str,
    manifest_path: &Path,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<Plan, String> {
    let root = manifest_path.canonicalize().map_err(|e| e.to_string())?;
    let parent = root.parent().ok_or("Manifest has no folder")?;
    let raw = fs::read(&root).map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_slice(&raw).map_err(|e| format!("Manifest: {e}"))?;
    validate_manifest(&manifest)?;

    on_progress(ProgressUpdate::info(format!(
        "Probing Fastboot device {serial}…"
    )));
    let device = probe_with_bin(fastboot_bin, serial)?;
    if device.product != manifest.product || device.mode != manifest.mode {
        return Err(format!(
            "Device product '{}' / mode '{}' does not match manifest product '{}' / mode '{}'",
            device.product, device.mode, manifest.product, manifest.mode
        ));
    }

    let total = manifest.images.len();
    let mut files = Vec::with_capacity(total);
    let mut sizes = Vec::with_capacity(total);
    for (idx, i) in manifest.images.iter().enumerate() {
        on_progress(ProgressUpdate {
            step: idx + 1,
            total,
            percent: Some(((idx + 1) as f32 / total as f32) * 100.0),
            message: format!(
                "Verifying [{}/{}] {} ({})…",
                idx + 1,
                total,
                i.partition,
                i.file
            ),
            stock: None,
        });
        let relative = Path::new(&i.file);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("Image paths must be relative files inside the manifest folder".into());
        }
        let path = parent
            .join(relative)
            .canonicalize()
            .map_err(|e| format!("{}: {e}", i.file))?;
        let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
        if !path.starts_with(parent) || !path.is_file() || meta.len() == 0 {
            return Err(format!("Missing, empty or escaped image file: {}", i.file));
        }
        if hash(&path)? != i.sha256.to_ascii_lowercase() {
            return Err(format!("SHA-256 mismatch: {}", i.file));
        }
        check_partition_with_size(fastboot_bin, &device, &i.partition, &path, meta.len())?;
        files.push(path);
        sizes.push(meta.len());
    }
    Ok(Plan {
        device,
        manifest,
        files,
        sizes,
    })
}

fn check_partition_with_size(
    fastboot_bin: Option<&str>,
    device: &Device,
    partition: &str,
    file_path: &Path,
    file_size: u64,
) -> Result<(), String> {
    // Never let Fastboot implicitly pick an A/B slot.
    let base = strip_slot_suffix(partition);
    let has_slot_key = format!("has-slot:{base}");
    let has_slot = if !device.vars.is_empty() {
        device.vars.get(&has_slot_key).cloned().unwrap_or_default()
    } else {
        var_with_bin(fastboot_bin, &device.serial, &has_slot_key).unwrap_or_default()
    };
    if has_slot == "yes" && partition == base {
        return Err(format!("Choose an explicit _a or _b partition for {base}"));
    }

    let size_key = format!("partition-size:{partition}");
    let size_opt = if !device.vars.is_empty() {
        device.vars.get(&size_key).cloned()
    } else {
        var_with_bin(fastboot_bin, &device.serial, &size_key).ok()
    };
    if let Some(size_str) = size_opt {
        if let Some(part_size) = parse_hex_u64(&size_str) {
            if part_size == 0 {
                return Err(format!("Partition {partition} is missing or empty"));
            }
            if !is_android_sparse_image(file_path)? && file_size > part_size {
                return Err(format!(
                    "Raw image ({file_size} bytes) exceeds partition '{partition}' size ({part_size} bytes)"
                ));
            }
        }
    }
    Ok(())
}

fn build_flash_args<'a>(
    partition: &'a str,
    path_str: &'a str,
    options: &FlashOptions,
) -> Vec<&'a str> {
    let mut args = Vec::with_capacity(5);
    if is_vbmeta_partition(partition) {
        if options.disable_verity {
            args.push("--disable-verity");
        }
        if options.disable_verification {
            args.push("--disable-verification");
        }
    }
    args.push("flash");
    args.push(partition);
    args.push(path_str);
    args
}

pub fn execute(
    plan: Plan,
    log_path: &Path,
    mut progress: impl FnMut(String),
) -> Result<(), String> {
    execute_with_options(None, plan, &FlashOptions::default(), log_path, move |u| {
        progress(u.message);
    })
}

pub fn execute_with_options(
    fastboot_bin: Option<&str>,
    plan: Plan,
    options: &FlashOptions,
    log_path: &Path,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<(), String> {
    let mut log =
        File::create(log_path).map_err(|e| format!("Cannot create flash journal: {e}"))?;
    writeln!(
        log,
        "Device: {:?}\nBuild: {}\nOptions: {:?}",
        plan.device, plan.manifest.build, options
    )
    .map_err(|e| e.to_string())?;

    // Revalidate device identity and all input files before the first write.
    on_progress(ProgressUpdate::info(
        "Revalidating device state and image checksums before writing…",
    ));
    let current = probe_with_bin(fastboot_bin, &plan.device.serial)?;
    if current.serial != plan.device.serial
        || current.product != plan.device.product
        || current.mode != plan.device.mode
        || current.slot != plan.device.slot
    {
        return Err("Device identity, mode or slot changed; review again".into());
    }
    for ((i, path), &size) in plan
        .manifest
        .images
        .iter()
        .zip(&plan.files)
        .zip(&plan.sizes)
    {
        if hash(path)? != i.sha256.to_ascii_lowercase() {
            return Err("Image changed since review; no write started".into());
        }
        check_partition_with_size(fastboot_bin, &plan.device, &i.partition, path, size)?;
    }

    let fb = resolve_fastboot(fastboot_bin);
    let total = plan.files.len();
    for (index, (i, path)) in plan.manifest.images.iter().zip(&plan.files).enumerate() {
        let step = index + 1;
        let message = format!(
            "[{step}/{total}] Flashing {} ({}) — SHA-256 {}",
            i.partition,
            i.file,
            &i.sha256[..12]
        );
        writeln!(log, "START {message}")
            .and_then(|_| log.sync_all())
            .map_err(|e| e.to_string())?;
        on_progress(ProgressUpdate {
            step,
            total,
            percent: Some(((index as f32) / (total as f32)) * 100.0),
            message,
            stock: None,
        });

        let path_str = path.to_str().ok_or("Image path is not valid Unicode")?;
        let args = build_flash_args(&i.partition, path_str, options);
        let result = run_program_streaming(
            &fb,
            Some(&plan.device.serial),
            &args,
            Duration::from_secs(1800),
            true,
            |line| {
                on_progress(ProgressUpdate {
                    step,
                    total,
                    percent: Some(((index as f32 + 0.5) / (total as f32)) * 100.0),
                    message: format!("  {line}"),
                    stock: None,
                });
            },
        );
        match result {
            Ok(text) => {
                writeln!(log, "{text}")
                    .and_then(|_| log.sync_all())
                    .map_err(|e| e.to_string())?;
                on_progress(ProgressUpdate {
                    step,
                    total,
                    percent: Some(((step as f32) / (total as f32)) * 100.0),
                    message: format!("[{step}/{total}] Completed {}", i.partition),
                    stock: None,
                });
            }
            Err(e) => {
                let _ = writeln!(
                    log,
                    "STOPPED: {e}\nPartial flash possible. Do not reboot automatically."
                );
                let _ = log.sync_all();
                return Err(e);
            }
        }
    }
    writeln!(
        log,
        "COMPLETED. Device left in Fastboot; no automatic reboot."
    )
    .and_then(|_| log.sync_all())
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Flashes a single partition image explicitly with full safety checks and journaling.
pub fn flash_single_image(
    serial: &str,
    partition: &str,
    path: &Path,
    log_path: &Path,
    mut progress: impl FnMut(String),
) -> Result<(), String> {
    flash_single_image_with_options(
        None,
        serial,
        partition,
        path,
        &FlashOptions::default(),
        log_path,
        move |u| progress(u.message),
    )
}

pub fn flash_single_image_with_options(
    fastboot_bin: Option<&str>,
    serial: &str,
    partition: &str,
    path: &Path,
    options: &FlashOptions,
    log_path: &Path,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("Image file does not exist: {}", path.display()));
    }
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.len() == 0 {
        return Err(format!("Image file is empty: {}", path.display()));
    }

    let device = probe_with_bin(fastboot_bin, serial)?;
    if !valid_partition(partition, &device.mode) {
        return Err(format!(
            "Partition '{}' is not valid for mode '{}'. Dangerous or unknown partitions are blocked.",
            partition, device.mode
        ));
    }
    check_partition_with_size(fastboot_bin, &device, partition, path, metadata.len())?;

    on_progress(ProgressUpdate {
        step: 1,
        total: 2,
        percent: Some(15.0),
        message: "Computing SHA-256 checksum…".into(),
        stock: None,
    });
    let digest = hash(path)?;

    let mut log =
        File::create(log_path).map_err(|e| format!("Cannot create flash journal: {e}"))?;
    writeln!(
        log,
        "Device: {:?}\nPartition: {}\nFile: {}\nSHA-256: {}\nOptions: {:?}",
        device,
        partition,
        path.display(),
        digest,
        options
    )
    .map_err(|e| e.to_string())?;

    let message = format!(
        "Flashing {partition} ({} bytes, SHA-256: {})…",
        metadata.len(),
        &digest[..12]
    );
    writeln!(log, "START {message}")
        .and_then(|_| log.sync_all())
        .map_err(|e| e.to_string())?;
    on_progress(ProgressUpdate {
        step: 2,
        total: 2,
        percent: Some(40.0),
        message,
        stock: None,
    });

    let fb = resolve_fastboot(fastboot_bin);
    let path_str = path.to_str().ok_or("Image path is not valid Unicode")?;
    let args = build_flash_args(partition, path_str, options);
    let result = run_program_streaming(
        &fb,
        Some(&device.serial),
        &args,
        Duration::from_secs(1800),
        true,
        |line| {
            on_progress(ProgressUpdate {
                step: 2,
                total: 2,
                percent: Some(75.0),
                message: format!("  {line}"),
                stock: None,
            });
        },
    );
    match result {
        Ok(text) => {
            writeln!(log, "{text}")
                .and_then(|_| log.sync_all())
                .map_err(|e| e.to_string())?;
            on_progress(ProgressUpdate {
                step: 2,
                total: 2,
                percent: Some(100.0),
                message: format!("Partition {partition} flashed cleanly."),
                stock: None,
            });
        }
        Err(e) => {
            let _ = writeln!(
                log,
                "STOPPED: {e}\nPartial write possible. Do not reboot automatically."
            );
            let _ = log.sync_all();
            return Err(e);
        }
    }
    writeln!(
        log,
        "COMPLETED. Device left in Fastboot; no automatic reboot."
    )
    .and_then(|_| log.sync_all())
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Temporarily boots a `boot` or `recovery` image in RAM without flashing (`fastboot boot`).
pub fn boot_image(
    fastboot_bin: Option<&str>,
    serial: &str,
    path: &Path,
    log_path: &Path,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("Image file does not exist: {}", path.display()));
    }
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.len() == 0 {
        return Err(format!("Image file is empty: {}", path.display()));
    }
    let device = probe_with_bin(fastboot_bin, serial)?;
    if device.mode != "bootloader" {
        return Err(
            "Temporary boot (`fastboot boot`) requires bootloader mode, not fastbootd.".into(),
        );
    }

    on_progress(ProgressUpdate {
        step: 1,
        total: 2,
        percent: Some(25.0),
        message: "Computing SHA-256 checksum…".into(),
        stock: None,
    });
    let digest = hash(path)?;

    let mut log = File::create(log_path).map_err(|e| format!("Cannot create boot journal: {e}"))?;
    writeln!(
        log,
        "Action: fastboot boot (temporary, non-destructive)\nDevice: {:?}\nFile: {}\nSHA-256: {}",
        device,
        path.display(),
        digest
    )
    .map_err(|e| e.to_string())?;

    let fb = resolve_fastboot(fastboot_bin);
    let path_str = path.to_str().ok_or("Image path is not valid Unicode")?;
    on_progress(ProgressUpdate {
        step: 2,
        total: 2,
        percent: Some(60.0),
        message: format!("Booting {} (SHA-256: {})…", path.display(), &digest[..12]),
        stock: None,
    });
    let text = run_program_streaming(
        &fb,
        Some(&device.serial),
        &["boot", path_str],
        Duration::from_secs(120),
        true,
        |line| {
            on_progress(ProgressUpdate {
                step: 2,
                total: 2,
                percent: Some(80.0),
                message: format!("  {line}"),
                stock: None,
            });
        },
    )?;
    writeln!(log, "{text}\nCOMPLETED")
        .and_then(|_| log.sync_all())
        .map_err(|e| e.to_string())?;
    on_progress(ProgressUpdate {
        step: 2,
        total: 2,
        percent: Some(100.0),
        message: "Temporary boot command succeeded.".into(),
        stock: None,
    });
    Ok(())
}

/// Switches the active A/B slot (`a` or `b`) on a Fastboot device.
pub fn set_active_slot(
    fastboot_bin: Option<&str>,
    serial: &str,
    slot: &str,
) -> Result<String, String> {
    let normalized = slot.trim().to_ascii_lowercase();
    if !matches!(normalized.as_str(), "a" | "b") {
        return Err(format!("Invalid A/B slot '{slot}'. Expected 'a' or 'b'."));
    }
    let device = probe_with_bin(fastboot_bin, serial)?;
    if device.slot.is_empty() && device.slot_count.unwrap_or(0) < 2 {
        return Err("Device does not appear to support A/B slots.".into());
    }
    let arg = format!("--set-active={normalized}");
    run_with_bin(fastboot_bin, Some(serial), &[&arg], Duration::from_secs(15))
}

/// Parses sideload progress percentage from lines like `serving: 'rom.zip' (~47%)` or `Total xfer: ...`.
pub fn parse_sideload_percent(line: &str) -> Option<f32> {
    let start = line.rfind('(')?;
    let end = line[start..].find('%')? + start;
    let raw = line[start + 1..end].trim().trim_start_matches('~');
    let pct = raw.parse::<f32>().ok()?;
    if (0.0..=100.0).contains(&pct) {
        Some(pct)
    } else {
        None
    }
}

/// Flashes an OTA or custom ROM `.zip` package via `adb sideload` with SHA-256 journaling and live progress.
pub fn adb_sideload(
    adb_bin: Option<&str>,
    serial: &str,
    zip_path: &Path,
    log_path: &Path,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<(), String> {
    if !zip_path.is_file() {
        return Err(format!(
            "ZIP package does not exist: {}",
            zip_path.display()
        ));
    }
    let metadata = fs::metadata(zip_path).map_err(|e| e.to_string())?;
    if metadata.len() == 0 {
        return Err(format!("ZIP package is empty: {}", zip_path.display()));
    }
    let devices = sideload_devices_with_bin(adb_bin)?;
    match devices.iter().find(|d| d.serial == serial) {
        Some(d) if d.state == "sideload" => {}
        Some(d) => {
            return Err(format!(
                "Device {serial} is in '{}' state. Enable 'Apply update from ADB' / 'ADB Sideload' in Recovery first.",
                d.state
            ));
        }
        None => {
            return Err(format!(
                "Device {serial} is not connected in ADB sideload mode."
            ));
        }
    }

    on_progress(ProgressUpdate {
        step: 1,
        total: 2,
        percent: Some(5.0),
        message: "Computing SHA-256 checksum of ZIP package…".into(),
        stock: None,
    });
    let digest = hash(zip_path)?;

    let mut log =
        File::create(log_path).map_err(|e| format!("Cannot create sideload journal: {e}"))?;
    writeln!(
        log,
        "Action: adb sideload\nSerial: {serial}\nFile: {}\nSize: {} bytes\nSHA-256: {}",
        zip_path.display(),
        metadata.len(),
        digest
    )
    .map_err(|e| e.to_string())?;

    on_progress(ProgressUpdate {
        step: 2,
        total: 2,
        percent: Some(0.0),
        message: format!(
            "Starting ADB sideload ({} bytes, SHA-256: {})…",
            metadata.len(),
            &digest[..12]
        ),
        stock: None,
    });

    let adb = resolve_adb(adb_bin);
    let path_str = zip_path.to_str().ok_or("ZIP path is not valid Unicode")?;
    let result = run_program_streaming(
        &adb,
        Some(serial),
        &["sideload", path_str],
        Duration::from_secs(3600),
        false,
        |line| {
            let pct = parse_sideload_percent(line);
            on_progress(ProgressUpdate {
                step: 2,
                total: 2,
                percent: pct,
                message: line.to_string(),
                stock: None,
            });
        },
    );
    match result {
        Ok(text) => {
            writeln!(log, "{text}\nCOMPLETED")
                .and_then(|_| log.sync_all())
                .map_err(|e| e.to_string())?;
            on_progress(ProgressUpdate {
                step: 2,
                total: 2,
                percent: Some(100.0),
                message: "ADB sideload finished.".into(),
                stock: None,
            });
            Ok(())
        }
        Err(e) => {
            let _ = writeln!(log, "STOPPED: {e}");
            let _ = log.sync_all();
            Err(e)
        }
    }
}

/// Issues a fastboot reboot command (e.g., target = "" for system, "bootloader", "fastboot", "recovery").
pub fn reboot(serial: Option<&str>, target: &str) -> Result<String, String> {
    reboot_with_bin(None, serial, target)
}

pub fn reboot_with_bin(
    fastboot_bin: Option<&str>,
    serial: Option<&str>,
    target: &str,
) -> Result<String, String> {
    let mut args = vec!["reboot"];
    if !target.is_empty() {
        args.push(target);
    }
    run_with_bin(fastboot_bin, serial, &args, Duration::from_secs(10))
}

/// Automatically scans a folder for image files (.img), hashes them, and creates a manifest.json.
pub fn generate_manifest(
    folder: &Path,
    product: &str,
    build: &str,
    mode: &str,
) -> Result<(Manifest, PathBuf), String> {
    generate_manifest_with_options(folder, product, build, mode, None, |_| {})
}

/// Scans a folder for `.img` files, optionally appends an A/B `slot_suffix` (`"_a"` or `"_b"`),
/// computes SHA-256 digests with progress reporting, and writes `manifest.json`.
pub fn generate_manifest_with_options(
    folder: &Path,
    product: &str,
    build: &str,
    mode: &str,
    slot_suffix: Option<&str>,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<(Manifest, PathBuf), String> {
    if !folder.is_dir() {
        return Err(format!("Not a directory: {}", folder.display()));
    }
    if let Some(s) = slot_suffix {
        if !matches!(s, "_a" | "_b") {
            return Err(format!("Invalid slot suffix '{s}'. Expected '_a' or '_b'."));
        }
    }
    let entries = fs::read_dir(folder).map_err(|e| e.to_string())?;
    let mut filenames: Vec<String> = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext.to_string_lossy().eq_ignore_ascii_case("img") {
                    if let Some(file_name) = path.file_name() {
                        filenames.push(file_name.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    filenames.sort();
    if filenames.is_empty() {
        return Err("No .img files found in folder".into());
    }

    let matched: Vec<(String, String)> = filenames
        .into_iter()
        .filter_map(|name| {
            let stem = Path::new(&name)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let partition = apply_slot_suffix(&stem, slot_suffix);
            valid_partition(&partition, mode).then_some((partition, name))
        })
        .collect();

    if matched.is_empty() {
        return Err(format!(
            "No valid partition images matched for mode '{mode}'"
        ));
    }

    let total = matched.len();
    let mut images = Vec::with_capacity(total);
    for (idx, (partition, name)) in matched.into_iter().enumerate() {
        on_progress(ProgressUpdate {
            step: idx + 1,
            total,
            percent: Some(((idx + 1) as f32 / total as f32) * 100.0),
            message: format!("Hashing [{}/{total}] {name} -> {partition}…", idx + 1),
            stock: None,
        });
        let full_path = folder.join(&name);
        let sha256 = hash(&full_path)?;
        images.push(Image {
            partition,
            file: name,
            sha256,
        });
    }

    let manifest = Manifest {
        product: product.trim().to_string(),
        build: build.trim().to_string(),
        mode: mode.to_string(),
        images,
    };
    validate_manifest(&manifest)?;

    let manifest_path = folder.join("manifest.json");
    let json_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    fs::write(&manifest_path, json_bytes).map_err(|e| e.to_string())?;

    Ok((manifest, manifest_path))
}

fn apply_slot_suffix(stem: &str, slot_suffix: Option<&str>) -> String {
    let Some(suffix) = slot_suffix else {
        return stem.to_string();
    };
    // Do not append slot suffix to non-slotted `super` or already-slotted names.
    if stem == "super" || stem.ends_with("_a") || stem.ends_with("_b") {
        stem.to_string()
    } else {
        format!("{stem}{suffix}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockMethod {
    /// `fastboot oem lock` (Xiaomi, OnePlus, Motorola, many Qualcomm OEMs)
    OemLock,
    /// `fastboot flashing lock` (Google Pixel, AOSP, Nothing)
    FlashingLock,
}

impl LockMethod {
    pub fn args(self) -> &'static [&'static str] {
        match self {
            LockMethod::OemLock => &["oem", "lock"],
            LockMethod::FlashingLock => &["flashing", "lock"],
        }
    }

    pub fn display_cmd(self) -> &'static str {
        match self {
            LockMethod::OemLock => "fastboot oem lock",
            LockMethod::FlashingLock => "fastboot flashing lock",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StockFlashMode {
    /// `flash_all_lock` — Flashes full Stock ROM, wipes userdata, and locks bootloader.
    CleanAllAndLock,
    /// `flash_all` — Flashes full Stock ROM, wipes userdata, and keeps bootloader unlocked.
    CleanAll,
    /// `flash_all_except_storage` — Updates Stock ROM while preserving userdata and unlocked bootloader.
    KeepUserData,
}

impl StockFlashMode {
    pub const ALL: [Self; 3] = [Self::CleanAllAndLock, Self::CleanAll, Self::KeepUserData];

    pub fn script_name(self) -> &'static str {
        #[cfg(windows)]
        {
            match self {
                Self::CleanAllAndLock => "flash_all_lock.bat",
                Self::CleanAll => "flash_all.bat",
                Self::KeepUserData => "flash_all_except_storage.bat",
            }
        }
        #[cfg(not(windows))]
        {
            match self {
                Self::CleanAllAndLock => "flash_all_lock.sh",
                Self::CleanAll => "flash_all.sh",
                Self::KeepUserData => "flash_all_except_storage.sh",
            }
        }
    }

    pub fn locks_bootloader(self) -> bool {
        matches!(self, Self::CleanAllAndLock)
    }

    pub fn wipes_userdata(self) -> bool {
        matches!(self, Self::CleanAllAndLock | Self::CleanAll)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CleanAllAndLock => "Clean All & Lock (flash_all_lock)",
            Self::CleanAll => "Clean All — Keep Unlocked (flash_all)",
            Self::KeepUserData => "Keep User Data — Keep Unlocked (flash_all_except_storage)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StockStepKind {
    Flash {
        partition: String,
        image_rel: String,
        bytes: u64,
    },
    Erase {
        partition: String,
    },
    SetActive {
        slot: String,
    },
    Lock,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StockStep {
    pub kind: StockStepKind,
    pub label: String,
    pub conditional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StockLockInfo {
    pub rom_folder: PathBuf,
    pub script_path: PathBuf,
    pub mode: StockFlashMode,
    pub available_modes: Vec<StockFlashMode>,
    pub expected_product: Option<String>,
    pub rom_anti_version: Option<u32>,
    pub has_oem_lock: bool,
    pub flash_command_count: usize,
    pub steps: Vec<StockStep>,
    pub total_bytes: u64,
    pub missing_images: Vec<String>,
}

fn normalize_script_image_path(raw: &str) -> String {
    let mut s = raw.trim().trim_matches('"').trim_matches('\'').to_string();
    for prefix in [
        "%~dp0",
        "`dirname $0`/",
        "$(dirname \"$0\")/",
        "$(dirname $0)/",
        "$SCRIPT_DIR/",
        "${SCRIPT_DIR}/",
        "./",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }
    s.replace('\\', "/").trim_start_matches('/').to_string()
}

fn is_fastboot_token(token: &str) -> bool {
    let cleaned = token.trim_matches('"').trim_matches('\'');
    let base = cleaned
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(cleaned)
        .to_ascii_lowercase();
    matches!(base.as_str(), "fastboot" | "fastboot.exe" | "$fastboot" | "%fastboot%")
}

pub fn parse_stock_rom_script(
    rom_folder: &Path,
    content: &str,
) -> (Option<String>, bool, Vec<StockStep>, Vec<String>) {
    let mut expected_product = None;
    let mut has_oem_lock = false;
    let mut steps = Vec::new();
    let mut missing_images = Vec::new();
    let mut in_crc_block = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("::")
            || trimmed.to_ascii_lowercase().starts_with("rem ")
            || trimmed.starts_with('#')
        {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if expected_product.is_none() {
            if let Some(idx) = trimmed.find("product:") {
                let after = trimmed[idx + "product:".len()..]
                    .trim_start_matches([' ', '*'])
                    .trim();
                let token: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if !token.is_empty() && token != "getvar" {
                    expected_product = Some(token);
                }
            }
        }
        if lower.contains("getvar") && lower.contains("crc") {
            in_crc_block = true;
            continue;
        }
        if in_crc_block && (trimmed == ")" || trimmed == "fi") {
            in_crc_block = false;
            continue;
        }

        let conditional = in_crc_block || lower.contains("crclist");

        // Inspect command segments before shell chaining (`||`, `&&`, `;`).
        for segment in trimmed.split("||").flat_map(|s| s.split("&&")).flat_map(|s| s.split(';')) {
            let tokens: Vec<&str> = segment.split_whitespace().collect();
            let Some(fb_pos) = tokens.iter().position(|t| is_fastboot_token(t)) else {
                continue;
            };
            let mut idx = fb_pos + 1;
            while idx < tokens.len() {
                let tok = tokens[idx];
                if matches!(tok, "%*" | "$*" | "\"$*\"") {
                    idx += 1;
                    continue;
                }
                if tok == "-s" {
                    idx += 2;
                    continue;
                }
                if let Some(slot) = tok.strip_prefix("--set-active=") {
                    let slot = slot.trim_matches('"').to_string();
                    steps.push(StockStep {
                        label: format!("set_active {slot}"),
                        kind: StockStepKind::SetActive { slot },
                        conditional,
                    });
                    break;
                }
                if tok.starts_with('-') {
                    idx += 1;
                    continue;
                }

                match tok.to_ascii_lowercase().as_str() {
                    "flash" => {
                        if idx + 2 < tokens.len() {
                            let partition = tokens[idx + 1].to_string();
                            let image_rel = normalize_script_image_path(tokens[idx + 2]);
                            let full_path = rom_folder.join(&image_rel);
                            let bytes = fs::metadata(&full_path).map(|m| m.len()).unwrap_or(0);
                            if !full_path.is_file() && !conditional {
                                missing_images.push(image_rel.clone());
                            }
                            steps.push(StockStep {
                                label: format!("flash {partition}"),
                                kind: StockStepKind::Flash {
                                    partition,
                                    image_rel,
                                    bytes,
                                },
                                conditional,
                            });
                        }
                        break;
                    }
                    "erase" => {
                        if idx + 1 < tokens.len() {
                            let partition = tokens[idx + 1].to_string();
                            steps.push(StockStep {
                                label: format!("erase {partition}"),
                                kind: StockStepKind::Erase { partition },
                                conditional,
                            });
                        }
                        break;
                    }
                    "set_active" => {
                        if idx + 1 < tokens.len() {
                            let slot = tokens[idx + 1].to_string();
                            steps.push(StockStep {
                                label: format!("set_active {slot}"),
                                kind: StockStepKind::SetActive { slot },
                                conditional,
                            });
                        }
                        break;
                    }
                    "oem" if tokens
                        .get(idx + 1)
                        .is_some_and(|n| n.eq_ignore_ascii_case("lock")) =>
                    {
                        has_oem_lock = true;
                        steps.push(StockStep {
                            label: "oem lock".into(),
                            kind: StockStepKind::Lock,
                            conditional,
                        });
                        break;
                    }
                    "flashing"
                        if tokens
                            .get(idx + 1)
                            .is_some_and(|n| n.eq_ignore_ascii_case("lock")) =>
                    {
                        has_oem_lock = true;
                        steps.push(StockStep {
                            label: "flashing lock".into(),
                            kind: StockStepKind::Lock,
                            conditional,
                        });
                        break;
                    }
                    _ => break,
                }
            }
        }
    }

    (expected_product, has_oem_lock, steps, missing_images)
}

fn resolve_stock_rom_root(folder: &Path) -> Result<(PathBuf, Vec<StockFlashMode>), String> {
    if !folder.is_dir() {
        return Err(format!("Not a directory: {}", folder.display()));
    }
    let find_modes = |dir: &Path| -> Vec<StockFlashMode> {
        StockFlashMode::ALL
            .into_iter()
            .filter(|m| dir.join(m.script_name()).is_file())
            .collect()
    };

    let direct = find_modes(folder);
    let rom_folder = if !direct.is_empty() {
        folder.to_path_buf()
    } else if folder
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("images"))
    {
        let parent = folder
            .parent()
            .ok_or_else(|| format!("No Stock ROM scripts found in {}", folder.display()))?;
        if find_modes(parent).is_empty() {
            return Err(format!(
                "Could not find any stock flash script (flash_all_lock / flash_all / flash_all_except_storage) in {}",
                folder.display()
            ));
        }
        parent.to_path_buf()
    } else {
        return Err(format!(
            "Could not find any stock flash script (flash_all_lock / flash_all / flash_all_except_storage) in {}",
            folder.display()
        ));
    };

    let available_modes = find_modes(&rom_folder);
    Ok((rom_folder, available_modes))
}

/// Inspects an extracted Stock Fastboot ROM folder (selecting `flash_all_lock` first,
/// or the first available `flash_all*` script) and extracts metadata, steps, and image sizes.
pub fn inspect_stock_rom_folder(folder: &Path) -> Result<StockLockInfo, String> {
    let (rom_folder, available_modes) = resolve_stock_rom_root(folder)?;
    let default_mode = available_modes
        .iter()
        .copied()
        .find(|m| *m == StockFlashMode::CleanAllAndLock)
        .unwrap_or(available_modes[0]);
    inspect_stock_rom_script(&rom_folder, default_mode)
}

/// Inspects a specific Stock Fastboot ROM script mode inside `folder`.
pub fn inspect_stock_rom_script(
    folder: &Path,
    mode: StockFlashMode,
) -> Result<StockLockInfo, String> {
    let (rom_folder, available_modes) = resolve_stock_rom_root(folder)?;
    let script_name = mode.script_name();
    let script_path = rom_folder.join(script_name);
    if !script_path.is_file() {
        return Err(format!(
            "Could not find '{script_name}' in {}",
            rom_folder.display()
        ));
    }

    let images_dir = rom_folder.join("images");
    if !images_dir.is_dir() {
        return Err(format!(
            "Missing 'images' directory inside {}",
            rom_folder.display()
        ));
    }

    let content =
        fs::read_to_string(&script_path).map_err(|e| format!("Cannot read {script_name}: {e}"))?;
    let (expected_product, has_oem_lock, steps, missing_images) =
        parse_stock_rom_script(&rom_folder, &content);
    if mode.locks_bootloader() && !has_oem_lock {
        return Err(format!(
            "Script '{script_name}' does not contain an `oem lock` or `flashing lock` command."
        ));
    }
    if !missing_images.is_empty() {
        let preview = missing_images
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Missing {} required image file(s) referenced by {script_name}: {preview}",
            missing_images.len()
        ));
    }

    let flash_command_count = steps
        .iter()
        .filter(|s| matches!(s.kind, StockStepKind::Flash { .. }))
        .count();
    let total_bytes: u64 = steps
        .iter()
        .filter(|s| !s.conditional)
        .filter_map(|s| match s.kind {
            StockStepKind::Flash { bytes, .. } => Some(bytes),
            _ => None,
        })
        .sum();

    let rom_anti_version = fs::read_to_string(images_dir.join("anti_version.txt"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());

    Ok(StockLockInfo {
        rom_folder,
        script_path,
        mode,
        available_modes,
        expected_product,
        rom_anti_version,
        has_oem_lock,
        flash_command_count,
        steps,
        total_bytes,
        missing_images,
    })
}

/// Inspects an extracted Stock Fastboot ROM folder specifically for `flash_all_lock`.
pub fn inspect_stock_lock_folder(folder: &Path) -> Result<StockLockInfo, String> {
    inspect_stock_rom_script(folder, StockFlashMode::CleanAllAndLock)
}

fn strip_ab_suffix(part: &str) -> &str {
    part.strip_suffix("_ab")
        .or_else(|| part.strip_suffix("_a"))
        .or_else(|| part.strip_suffix("_b"))
        .unwrap_or(part)
}

pub fn partition_matches(step_part: &str, reported_part: &str) -> bool {
    if step_part.eq_ignore_ascii_case(reported_part) {
        return true;
    }
    if step_part.to_ascii_lowercase().ends_with("_ab") {
        return strip_ab_suffix(step_part).eq_ignore_ascii_case(strip_ab_suffix(reported_part));
    }
    false
}

fn extract_single_quoted_after<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let idx = line.find(prefix)?;
    let rest = &line[idx + prefix.len()..];
    let end = rest.find('\'')?;
    let val = rest[..end].trim();
    (!val.is_empty()).then_some(val)
}

pub fn parse_sparse_chunk_line(line: &str) -> Option<(String, u32, u32)> {
    let idx = line.find("Sending sparse '")?;
    let rest = &line[idx + "Sending sparse '".len()..];
    let end_part = rest.find('\'')?;
    let part = rest[..end_part].trim();
    if part.is_empty() {
        return None;
    }
    let after_part = rest[end_part + 1..].trim_start();
    let ratio_tok = after_part.split_whitespace().next()?;
    let (cur_s, tot_s) = ratio_tok.split_once('/')?;
    let cur = cur_s.parse::<u32>().ok()?;
    let tot = tot_s.parse::<u32>().ok()?;
    if cur == 0 || tot == 0 || cur > tot {
        return None;
    }
    Some((part.to_string(), cur, tot))
}

#[derive(Debug, PartialEq, Eq)]
enum FastbootLineAction {
    SendingSparse {
        partition: String,
        cur: u32,
        tot: u32,
    },
    Sending {
        partition: String,
    },
    Writing {
        partition: String,
    },
    Erasing {
        partition: String,
    },
    SetActive {
        slot: String,
    },
    Okay,
    Finished,
}

fn parse_fastboot_action_line(line: &str) -> Option<FastbootLineAction> {
    let trimmed = line.trim();
    if let Some((partition, cur, tot)) = parse_sparse_chunk_line(trimmed) {
        return Some(FastbootLineAction::SendingSparse {
            partition,
            cur,
            tot,
        });
    }
    if let Some(part) = extract_single_quoted_after(trimmed, "Sending '") {
        return Some(FastbootLineAction::Sending {
            partition: part.to_string(),
        });
    }
    if let Some(part) = extract_single_quoted_after(trimmed, "Writing '") {
        return Some(FastbootLineAction::Writing {
            partition: part.to_string(),
        });
    }
    if let Some(part) = extract_single_quoted_after(trimmed, "Erasing '") {
        return Some(FastbootLineAction::Erasing {
            partition: part.to_string(),
        });
    }
    if let Some(slot) = extract_single_quoted_after(trimmed, "Setting current slot to '") {
        return Some(FastbootLineAction::SetActive {
            slot: slot.to_string(),
        });
    }
    if trimmed.starts_with("OKAY [") {
        return Some(FastbootLineAction::Okay);
    }
    if trimmed.starts_with("Finished. Total time:") {
        return Some(FastbootLineAction::Finished);
    }
    None
}

pub struct StockProgressTracker {
    steps: Vec<StockStep>,
    step_done: Vec<bool>,
    next_idx: usize,
    active_idx: Option<usize>,
    current_partition: Option<String>,
    current_chunk: Option<(u32, u32)>,
    chunk_written: bool,
    total_bytes: u64,
    completed_bytes: u64,
    started: Instant,
}

impl StockProgressTracker {
    pub fn new(steps: Vec<StockStep>) -> Self {
        let len = steps.len();
        let total_bytes = steps
            .iter()
            .filter(|s| !s.conditional)
            .filter_map(|s| match s.kind {
                StockStepKind::Flash { bytes, .. } => Some(bytes),
                _ => None,
            })
            .sum();
        Self {
            steps,
            step_done: vec![false; len],
            next_idx: 0,
            active_idx: None,
            current_partition: None,
            current_chunk: None,
            chunk_written: false,
            total_bytes,
            completed_bytes: 0,
            started: Instant::now(),
        }
    }

    fn step_matches_flash(step: &StockStep, partition: &str) -> bool {
        matches!(
            &step.kind,
            StockStepKind::Flash { partition: p, .. } if partition_matches(p, partition)
        )
    }

    fn step_matches_erase(step: &StockStep, partition: &str) -> bool {
        matches!(
            &step.kind,
            StockStepKind::Erase { partition: p } if partition_matches(p, partition)
        )
    }

    fn step_matches_set_active(step: &StockStep, slot: &str) -> bool {
        matches!(
            &step.kind,
            StockStepKind::SetActive { slot: s } if s.eq_ignore_ascii_case(slot)
        )
    }

    fn find_step_from(&self, start: usize, pred: impl Fn(&StockStep) -> bool) -> Option<usize> {
        (start..self.steps.len()).find(|&i| !self.step_done[i] && pred(&self.steps[i]))
    }

    fn next_non_conditional_lock(&self) -> Option<usize> {
        let idx = (self.next_idx..self.steps.len())
            .find(|&i| !self.step_done[i] && !self.steps[i].conditional)?;
        matches!(self.steps[idx].kind, StockStepKind::Lock).then_some(idx)
    }

    fn complete_step(&mut self, idx: usize) {
        if idx >= self.steps.len() || self.step_done[idx] {
            return;
        }
        self.step_done[idx] = true;
        if let StockStepKind::Flash { bytes, .. } = self.steps[idx].kind {
            if !self.steps[idx].conditional {
                self.completed_bytes = self.completed_bytes.saturating_add(bytes);
            }
        }
        if idx + 1 > self.next_idx {
            self.next_idx = idx + 1;
        }
        self.current_partition = None;
        self.current_chunk = None;
        self.chunk_written = false;
    }

    fn activate_step(&mut self, idx: usize, partition_display: String, chunk: Option<(u32, u32)>) {
        if self.active_idx != Some(idx) {
            if let Some(prev) = self.active_idx.take() {
                self.complete_step(prev);
            }
            if idx > self.next_idx {
                self.next_idx = idx;
            }
            self.active_idx = Some(idx);
        }
        self.current_partition = Some(partition_display);
        self.current_chunk = chunk;
        self.chunk_written = false;
    }

    pub fn feed_line(&mut self, line: &str) -> ProgressUpdate {
        self.feed_line_at(line, self.started.elapsed())
    }

    pub fn feed_line_at(&mut self, line: &str, elapsed: Duration) -> ProgressUpdate {
        if let Some(action) = parse_fastboot_action_line(line) {
            match action {
                FastbootLineAction::SendingSparse {
                    partition,
                    cur,
                    tot,
                } => {
                    let target = self
                        .active_idx
                        .filter(|&i| Self::step_matches_flash(&self.steps[i], &partition))
                        .or_else(|| {
                            self.find_step_from(self.next_idx, |s| {
                                Self::step_matches_flash(s, &partition)
                            })
                        });
                    if let Some(idx) = target {
                        self.activate_step(idx, partition, Some((cur, tot)));
                    }
                }
                FastbootLineAction::Sending { partition } => {
                    let target = self
                        .active_idx
                        .filter(|&i| Self::step_matches_flash(&self.steps[i], &partition))
                        .or_else(|| {
                            self.find_step_from(self.next_idx, |s| {
                                Self::step_matches_flash(s, &partition)
                            })
                        });
                    if let Some(idx) = target {
                        self.activate_step(idx, partition, None);
                    }
                }
                FastbootLineAction::Writing { partition } => {
                    if let Some(idx) = self.active_idx {
                        if Self::step_matches_flash(&self.steps[idx], &partition) {
                            self.current_partition = Some(partition);
                            self.chunk_written = true;
                        }
                    }
                }
                FastbootLineAction::Erasing { partition } => {
                    let target = self
                        .active_idx
                        .filter(|&i| Self::step_matches_erase(&self.steps[i], &partition))
                        .or_else(|| {
                            self.find_step_from(self.next_idx, |s| {
                                Self::step_matches_erase(s, &partition)
                            })
                        });
                    if let Some(idx) = target {
                        self.activate_step(idx, format!("erase {partition}"), None);
                    }
                }
                FastbootLineAction::SetActive { slot } => {
                    let target = self
                        .active_idx
                        .filter(|&i| Self::step_matches_set_active(&self.steps[i], &slot))
                        .or_else(|| {
                            self.find_step_from(self.next_idx, |s| {
                                Self::step_matches_set_active(s, &slot)
                            })
                        });
                    if let Some(idx) = target {
                        self.activate_step(idx, format!("set_active {slot}"), None);
                    }
                }
                FastbootLineAction::Okay => {
                    if self.active_idx.is_none() {
                        if let Some(lock_idx) = self.next_non_conditional_lock() {
                            let label = self.steps[lock_idx].label.clone();
                            self.activate_step(lock_idx, label, None);
                        }
                    }
                }
                FastbootLineAction::Finished => {
                    if let Some(idx) = self.active_idx.take() {
                        self.complete_step(idx);
                    } else if let Some(lock_idx) = self.next_non_conditional_lock() {
                        self.complete_step(lock_idx);
                    }
                }
            }
        }

        self.build_update(line, elapsed)
    }

    pub fn snapshot(&self, elapsed: Duration) -> StockFlashProgress {
        let total_steps = self
            .steps
            .iter()
            .enumerate()
            .filter(|&(i, s)| !s.conditional || self.step_done[i] || self.active_idx == Some(i))
            .count();
        let completed_steps = self.step_done.iter().filter(|&&d| d).count();

        let partial_bytes: f64 = match self.active_idx {
            Some(idx) => match self.steps[idx].kind {
                StockStepKind::Flash { bytes, .. } if !self.steps[idx].conditional => {
                    if let Some((cur, tot)) = self.current_chunk {
                        if tot > 0 {
                            let phase = if self.chunk_written { 0.75 } else { 0.25 };
                            let frac = ((cur.saturating_sub(1) as f64 + phase) / tot as f64)
                                .clamp(0.0, 0.99);
                            bytes as f64 * frac
                        } else {
                            0.0
                        }
                    } else {
                        let phase = if self.chunk_written { 0.65 } else { 0.20 };
                        bytes as f64 * phase
                    }
                }
                _ => 0.0,
            },
            None => 0.0,
        };

        let effective_bytes =
            (self.completed_bytes as f64 + partial_bytes).min(self.total_bytes as f64);

        let eta_seconds = if elapsed.as_secs_f64() >= 5.0
            && effective_bytes > 0.0
            && effective_bytes < self.total_bytes as f64
        {
            let speed = effective_bytes / elapsed.as_secs_f64();
            if speed > 1.0 {
                Some(((self.total_bytes as f64 - effective_bytes) / speed).round() as u64)
            } else {
                None
            }
        } else {
            None
        };

        let remaining_summary = self
            .steps
            .iter()
            .enumerate()
            .filter(|&(i, s)| !self.step_done[i] && self.active_idx != Some(i) && !s.conditional)
            .map(|(_, s)| match &s.kind {
                StockStepKind::Flash {
                    partition, bytes, ..
                } => {
                    if *bytes >= 1024 * 1024 {
                        format!(
                            "{partition} ({:.0} MB)",
                            *bytes as f64 / (1024.0 * 1024.0)
                        )
                    } else {
                        partition.clone()
                    }
                }
                _ => s.label.clone(),
            })
            .collect();

        StockFlashProgress {
            current_partition: self.current_partition.clone(),
            current_chunk: self.current_chunk,
            completed_steps,
            total_steps,
            completed_bytes: effective_bytes.round() as u64,
            total_bytes: self.total_bytes,
            remaining_summary,
            eta_seconds,
        }
    }

    fn build_update(&self, line: &str, elapsed: Duration) -> ProgressUpdate {
        let snap = self.snapshot(elapsed);
        let percent = if snap.total_bytes > 0 {
            if snap.completed_steps >= snap.total_steps && snap.total_steps > 0 {
                99.0
            } else if snap.completed_bytes == 0 && self.active_idx.is_none() {
                1.0
            } else {
                ((snap.completed_bytes as f64 / snap.total_bytes as f64) * 98.0).clamp(1.0, 99.0)
                    as f32
            }
        } else if snap.total_steps > 0 {
            ((snap.completed_steps as f32 / snap.total_steps as f32) * 98.0).clamp(1.0, 99.0)
        } else {
            1.0
        };

        ProgressUpdate {
            step: snap.completed_steps,
            total: snap.total_steps,
            percent: Some(percent),
            message: line.to_string(),
            stock: Some(snap),
        }
    }
}

fn validate_safe_serial(serial: &str) -> Result<(), String> {
    if serial.is_empty()
        || !serial
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    {
        return Err(format!("Unsafe or invalid device serial: '{serial}'"));
    }
    Ok(())
}

/// Runs a verified Stock Fastboot ROM script (`flash_all_lock`, `flash_all`, or `flash_all_except_storage`)
/// with strict pre-flight checks, real-time chunk/partition/ETA tracking, and journaling.
pub fn run_stock_flash_script(
    fastboot_bin: Option<&str>,
    serial: &str,
    info: &StockLockInfo,
    log_path: &Path,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<(), String> {
    validate_safe_serial(serial)?;
    let script_str = info
        .script_path
        .to_str()
        .ok_or("Script path is not valid Unicode")?;
    if script_str
        .chars()
        .any(|c| matches!(c, '"' | '&' | '|' | '\r' | '\n' | ';' | '<' | '>'))
    {
        return Err("Script path contains unsupported shell metacharacters.".into());
    }

    let mut tracker = StockProgressTracker::new(info.steps.clone());
    let initial_snap = tracker.snapshot(Duration::ZERO);
    on_progress(ProgressUpdate {
        step: 0,
        total: initial_snap.total_steps.max(1),
        percent: Some(1.0),
        message: format!(
            "Probing Fastboot device {serial} before {}…",
            info.mode.label()
        ),
        stock: Some(initial_snap),
    });

    let device = probe_with_bin(fastboot_bin, serial)?;
    if device.mode != "bootloader" {
        return Err(
            "Stock Fastboot ROM script requires `bootloader` mode (not `fastbootd`). Reboot to Bootloader first."
                .into(),
        );
    }
    if device.battery_soc_ok == Some(false) {
        return Err(
            "Device reports low battery (`battery-soc-ok: no`). Charge battery before flashing."
                .into(),
        );
    }
    if let Some(expected) = &info.expected_product {
        if device.product != *expected {
            return Err(format!(
                "Product mismatch! Device is '{}' but ROM script expects '{}'. Blocked to prevent brick.",
                device.product, expected
            ));
        }
    }
    if let (Some(dev_anti), Some(rom_anti)) = (
        device
            .anti_rollback
            .as_deref()
            .and_then(|s| s.trim().parse::<u32>().ok()),
        info.rom_anti_version,
    ) {
        if dev_anti > rom_anti {
            return Err(format!(
                "Anti-rollback check FAILED: device anti={dev_anti} > ROM anti={rom_anti}. Blocked to prevent brick."
            ));
        }
    }

    let mut log =
        File::create(log_path).map_err(|e| format!("Cannot create flash journal: {e}"))?;
    writeln!(
        log,
        "Action: Stock Flash ({})\nDevice: {:?}\nScript: {}\nExpected Product: {:?}\nROM Anti: {:?}\nTotal Bytes: {}",
        info.mode.label(),
        device,
        info.script_path.display(),
        info.expected_product,
        info.rom_anti_version,
        info.total_bytes
    )
    .and_then(|_| log.sync_all())
    .map_err(|e| e.to_string())?;

    let mut cmd = {
        #[cfg(windows)]
        {
            let mut c = Command::new("cmd");
            c.args(["/C", info.mode.script_name(), "-s", serial]);
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("sh");
            c.args([script_str, "-s", serial]);
            c
        }
    };
    cmd.current_dir(&info.rom_folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Prepend custom fastboot directory to PATH if configured.
    if let Some(fb) = fastboot_bin.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(fb_dir) = Path::new(fb).parent().filter(|p| !p.as_os_str().is_empty()) {
            let current_path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![fb_dir.to_path_buf()];
            paths.extend(std::env::split_paths(&current_path));
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("PATH", joined);
            }
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot start stock flash script: {e}"))?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let (tx, rx) = unbounded::<String>();
    let a = spawn_pipe_reader(Box::new(out), tx.clone());
    let b = spawn_pipe_reader(Box::new(err), tx);

    let start = Instant::now();
    let timeout = Duration::from_secs(3600);
    let mut full_output = String::new();
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            let _ = writeln!(log, "{line}");
            full_output.push_str(&line);
            full_output.push('\n');
            on_progress(tracker.feed_line(&line));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = a.join();
                let _ = b.join();
                let _ = writeln!(log, "STOPPED: Timed out.");
                let _ = log.sync_all();
                return Err(
                    "Stock ROM Flash timed out. Do NOT reboot until verifying device state.".into(),
                );
            }
        }
    };
    let _ = a.join();
    let _ = b.join();
    while let Ok(line) = rx.try_recv() {
        let _ = writeln!(log, "{line}");
        full_output.push_str(&line);
        full_output.push('\n');
        on_progress(tracker.feed_line(&line));
    }
    let _ = log.sync_all();

    if !status.success() {
        return Err(format!(
            "Stock Flash script exited with {status}. Check journal log: {}",
            log_path.display()
        ));
    }

    let completion_msg = if info.mode.locks_bootloader() {
        "Stock ROM flashed and bootloader locked cleanly."
    } else {
        "Stock ROM flashed cleanly (bootloader kept unlocked)."
    };
    let _ = writeln!(log, "COMPLETED: {completion_msg}");
    let _ = log.sync_all();
    let mut final_snap = tracker.snapshot(start.elapsed());
    final_snap.current_partition = None;
    final_snap.current_chunk = None;
    final_snap.eta_seconds = Some(0);
    on_progress(ProgressUpdate {
        step: final_snap.total_steps,
        total: final_snap.total_steps,
        percent: Some(100.0),
        message: completion_msg.into(),
        stock: Some(final_snap),
    });
    Ok(())
}

/// Runs a verified Stock Fastboot ROM `flash_all_lock` script.
pub fn run_stock_flash_and_lock(
    fastboot_bin: Option<&str>,
    serial: &str,
    info: &StockLockInfo,
    log_path: &Path,
    on_progress: impl FnMut(ProgressUpdate),
) -> Result<(), String> {
    run_stock_flash_script(fastboot_bin, serial, info, log_path, on_progress)
}

/// Directly locks the bootloader (`fastboot oem lock` or `fastboot flashing lock`)
/// after verifying the device is in `bootloader` mode, currently unlocked, and battery is OK.
pub fn lock_bootloader(
    fastboot_bin: Option<&str>,
    serial: &str,
    method: LockMethod,
    log_path: &Path,
    mut on_progress: impl FnMut(ProgressUpdate),
) -> Result<(), String> {
    on_progress(ProgressUpdate {
        step: 1,
        total: 2,
        percent: Some(20.0),
        message: format!("Verifying bootloader mode & status on {serial}…"),
        stock: None,
    });
    let device = probe_with_bin(fastboot_bin, serial)?;
    if device.mode != "bootloader" {
        return Err(
            "Direct bootloader lock requires `bootloader` mode (not `fastbootd`). Reboot to Bootloader first."
                .into(),
        );
    }
    if device.battery_soc_ok == Some(false) {
        return Err("Device reports low battery. Charge before locking bootloader.".into());
    }

    let mut log = File::create(log_path).map_err(|e| format!("Cannot create lock journal: {e}"))?;
    writeln!(
        log,
        "Action: Direct Bootloader Lock ({})\nDevice: {:?}",
        method.display_cmd(),
        device
    )
    .and_then(|_| log.sync_all())
    .map_err(|e| e.to_string())?;

    let fb = resolve_fastboot(fastboot_bin);
    on_progress(ProgressUpdate {
        step: 2,
        total: 2,
        percent: Some(60.0),
        message: format!("Running `{}` on {serial}…", method.display_cmd()),
        stock: None,
    });
    let text = run_program_streaming(
        &fb,
        Some(serial),
        method.args(),
        Duration::from_secs(60),
        true,
        |line| {
            on_progress(ProgressUpdate {
                step: 2,
                total: 2,
                percent: Some(85.0),
                message: format!("  {line}"),
                stock: None,
            });
        },
    )?;
    writeln!(log, "{text}\nCOMPLETED")
        .and_then(|_| log.sync_all())
        .map_err(|e| e.to_string())?;
    on_progress(ProgressUpdate {
        step: 2,
        total: 2,
        percent: Some(100.0),
        message: format!(
            "`{}` completed. Check phone screen if confirmation is required.",
            method.display_cmd()
        ),
        stock: None,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fastboot_stderr() {
        assert_eq!(
            parse_var(
                "(bootloader) partition-size:boot_a: 0x6000000\nFinished",
                "partition-size:boot_a"
            )
            .unwrap(),
            "0x6000000"
        );
        assert!(parse_var("FAILED", "unlocked").is_err());
    }

    #[test]
    fn parses_hex_sizes() {
        assert_eq!(parse_hex_u64("0x6000000"), Some(0x6000000));
        assert_eq!(parse_hex_u64("0X1000"), Some(0x1000));
        assert_eq!(parse_hex_u64("805306368"), Some(805_306_368));
        assert_eq!(parse_hex_u64("c000000"), Some(0x0c00_0000));
    }

    #[test]
    fn rejects_dangerous_targets_and_wrong_mode() {
        for p in [
            "userdata",
            "metadata",
            "persist",
            "abl",
            "../boot",
            "boot --slot all",
        ] {
            assert!(!valid_partition(p, "bootloader"));
        }
        assert!(!valid_partition("system_a", "bootloader"));
        assert!(valid_partition("system_a", "fastbootd"));
        assert!(valid_partition("boot_b", "bootloader"));
    }

    #[test]
    fn rejects_duplicate_and_bad_digest() {
        let mut m = Manifest {
            product: "unicorn".into(),
            build: "test".into(),
            mode: "bootloader".into(),
            images: vec![Image {
                partition: "boot_a".into(),
                file: "boot.img".into(),
                sha256: "a".repeat(64),
            }],
        };
        assert!(validate_manifest(&m).is_ok());
        m.images.push(m.images[0].clone());
        assert!(validate_manifest(&m).is_err());
        m.images.pop();
        m.images[0].sha256 = "bad".into();
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn applies_slot_suffix_safely() {
        assert_eq!(apply_slot_suffix("boot", Some("_a")), "boot_a");
        assert_eq!(apply_slot_suffix("boot_b", Some("_a")), "boot_b");
        assert_eq!(apply_slot_suffix("super", Some("_a")), "super");
        assert_eq!(apply_slot_suffix("vbmeta", None), "vbmeta");
    }

    #[test]
    fn builds_vbmeta_flags_only_for_vbmeta() {
        let opts = FlashOptions {
            disable_verity: true,
            disable_verification: true,
        };
        assert_eq!(
            build_flash_args("vbmeta_a", "vbmeta.img", &opts),
            vec![
                "--disable-verity",
                "--disable-verification",
                "flash",
                "vbmeta_a",
                "vbmeta.img"
            ]
        );
        assert_eq!(
            build_flash_args("boot_a", "boot.img", &opts),
            vec!["flash", "boot_a", "boot.img"]
        );
    }

    #[test]
    fn parses_sideload_devices_and_progress() {
        let list = "List of devices attached\nABC123\tsideload\nDEF456\trecovery\nGHI789\tdevice\n";
        let devs = parse_sideload_devices(list);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].serial, "ABC123");
        assert_eq!(devs[0].state, "sideload");
        assert_eq!(devs[1].serial, "DEF456");
        assert_eq!(devs[1].state, "recovery");

        assert_eq!(
            parse_sideload_percent("serving: 'lineage.zip'  (~47%)"),
            Some(47.0)
        );
        assert_eq!(
            parse_sideload_percent("serving: 'rom.zip'  (100%)"),
            Some(100.0)
        );
        assert_eq!(parse_sideload_percent("Total xfer: 1.00x"), None);
    }

    #[test]
    fn detects_android_sparse_header() {
        let dir = std::env::temp_dir().join("adb-client-sparse-test");
        let _ = fs::create_dir_all(&dir);
        let sparse = dir.join("sparse.img");
        let raw = dir.join("raw.img");
        fs::write(&sparse, [0x3A, 0xFF, 0x26, 0xED, 0x01, 0x00]).unwrap();
        fs::write(&raw, b"ANDROID!rawbootimage").unwrap();
        assert!(is_android_sparse_image(&sparse).unwrap());
        assert!(!is_android_sparse_image(&raw).unwrap());
        let _ = fs::remove_file(&sparse);
        let _ = fs::remove_file(&raw);
    }

    #[test]
    fn parses_getvar_all_output() {
        let sample = "(bootloader) product:unicorn\n\
                      (bootloader) unlocked:yes\n\
                      (bootloader) is-userspace:no\n\
                      (bootloader) current-slot:a\n\
                      (bootloader) has-slot:boot:yes\n\
                      (bootloader) partition-size:boot_a: 0xc000000\n\
                      (bootloader) anti:1\n\
                      all: Done!!\n\
                      Finished. Total time: 0.015s\n";
        let vars = parse_all_vars(sample);
        assert_eq!(vars.get("product").map(String::as_str), Some("unicorn"));
        assert_eq!(vars.get("unlocked").map(String::as_str), Some("yes"));
        assert_eq!(vars.get("has-slot:boot").map(String::as_str), Some("yes"));
        assert_eq!(
            vars.get("partition-size:boot_a").map(String::as_str),
            Some("0xc000000")
        );
        assert_eq!(vars.get("anti").map(String::as_str), Some("1"));
    }

    #[test]
    fn parses_full_xiaomi_script_steps_and_sizes() {
        let dir = std::env::temp_dir().join("adb-client-stock-parse-test");
        let images = dir.join("images");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&images).unwrap();
        fs::write(images.join("super.img"), vec![0u8; 4096]).unwrap();
        fs::write(images.join("vendor_boot.img"), vec![0u8; 1024]).unwrap();
        fs::write(images.join("cust.img"), vec![0u8; 2048]).unwrap();
        fs::write(images.join("boot.img"), vec![0u8; 512]).unwrap();

        let script = r#"
fastboot %* getvar product 2>&1 | findstr /r /c:"^product: *unicorn" || exit /B 1
fastboot %* getvar crc 2>&1 | findstr /r /c:"^crc: 1" && if %errorlevel% equ 0 (
fastboot %* flash crclist %~dp0images\crclist.txt || @echo "Flash crclist error" && exit /B 1
fastboot %* flash sparsecrclist %~dp0images\sparsecrclist.txt || @echo "Flash sparsecrclist error" && exit /B 1
)
fastboot %* erase boot_ab || exit /B 1
fastboot %* flash super %~dp0images/super.img || exit 1
fastboot %* flash vendor_boot_ab %~dp0images/vendor_boot.img || exit 1
fastboot %* flash cust %~dp0images/cust.img || exit 1
fastboot %* flash boot_ab %~dp0images/boot.img || exit 1
fastboot %* set_active a || exit 1
fastboot %* oem lock || exit /B 1
"#;
        let (product, has_lock, steps, missing) = parse_stock_rom_script(&dir, script);
        assert_eq!(product.as_deref(), Some("unicorn"));
        assert!(has_lock);
        assert!(missing.is_empty());
        assert_eq!(steps.len(), 9);
        assert!(steps[0].conditional);
        assert!(steps[1].conditional);
        assert!(!steps[2].conditional);
        assert_eq!(steps[2].kind, StockStepKind::Erase { partition: "boot_ab".into() });
        assert_eq!(
            steps[3].kind,
            StockStepKind::Flash {
                partition: "super".into(),
                image_rel: "images/super.img".into(),
                bytes: 4096,
            }
        );
        assert_eq!(steps[7].kind, StockStepKind::SetActive { slot: "a".into() });
        assert_eq!(steps[8].kind, StockStepKind::Lock);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tracks_sparse_chunks_ab_slots_and_ignores_getvar() {
        let steps = vec![
            StockStep {
                kind: StockStepKind::Flash {
                    partition: "crclist".into(),
                    image_rel: "images/crclist.txt".into(),
                    bytes: 100,
                },
                label: "flash crclist".into(),
                conditional: true,
            },
            StockStep {
                kind: StockStepKind::Erase {
                    partition: "boot_ab".into(),
                },
                label: "erase boot_ab".into(),
                conditional: false,
            },
            StockStep {
                kind: StockStepKind::Flash {
                    partition: "super".into(),
                    image_rel: "images/super.img".into(),
                    bytes: 11_000_000,
                },
                label: "flash super".into(),
                conditional: false,
            },
            StockStep {
                kind: StockStepKind::Flash {
                    partition: "vendor_boot_ab".into(),
                    image_rel: "images/vendor_boot.img".into(),
                    bytes: 2_000_000,
                },
                label: "flash vendor_boot_ab".into(),
                conditional: false,
            },
            StockStep {
                kind: StockStepKind::Flash {
                    partition: "cust".into(),
                    image_rel: "images/cust.img".into(),
                    bytes: 1_048_576 * 988,
                },
                label: "flash cust".into(),
                conditional: false,
            },
            StockStep {
                kind: StockStepKind::SetActive { slot: "a".into() },
                label: "set_active a".into(),
                conditional: false,
            },
            StockStep {
                kind: StockStepKind::Lock,
                label: "oem lock".into(),
                conditional: false,
            },
        ];

        let mut tracker = StockProgressTracker::new(steps);

        // Initial `getvar` lines should NOT advance any steps.
        let u = tracker.feed_line_at("product: unicorn", Duration::from_secs(1));
        assert_eq!(u.stock.as_ref().unwrap().completed_steps, 0);
        let u = tracker.feed_line_at("Finished. Total time: 0.004s", Duration::from_secs(1));
        assert_eq!(u.stock.as_ref().unwrap().completed_steps, 0);

        // Conditional `crclist` skipped; `erase boot_ab` activates & finishes cleanly.
        tracker.feed_line_at("Erasing 'boot_ab'", Duration::from_secs(2));
        let u = tracker.feed_line_at("Finished. Total time: 0.120s", Duration::from_secs(2));
        assert_eq!(u.stock.as_ref().unwrap().completed_steps, 1);

        // Sparse chunk 3/11 of `super` reports chunk + fractional progress + ETA.
        tracker.feed_line_at("Sending sparse 'super' 1/11 (786428 KB)", Duration::from_secs(10));
        let u = tracker.feed_line_at("Sending sparse 'super' 3/11 (786428 KB)", Duration::from_secs(100));
        let s = u.stock.unwrap();
        assert_eq!(s.current_partition.as_deref(), Some("super"));
        assert_eq!(s.current_chunk, Some((3, 11)));
        assert!(s.completed_bytes > 2_000_000 && s.completed_bytes < 3_000_000);
        assert!(s.eta_seconds.is_some());
        assert!(s.remaining_summary.iter().any(|r| r.contains("cust (988 MB)")));

        tracker.feed_line_at("Sending sparse 'super' 11/11 (12345 KB)", Duration::from_secs(415));
        let u = tracker.feed_line_at("Finished. Total time: 419.0s", Duration::from_secs(419));
        assert_eq!(u.stock.unwrap().completed_steps, 2);

        // `_ab` dual-slot writes `_a` then `_b` before completing on `Finished. Total time:`.
        let u = tracker.feed_line_at("Sending 'vendor_boot_a' (98304 KB)", Duration::from_secs(420));
        assert_eq!(u.stock.unwrap().completed_steps, 2);
        let u = tracker.feed_line_at("Sending 'vendor_boot_b' (98304 KB)", Duration::from_secs(422));
        assert_eq!(u.stock.as_ref().unwrap().completed_steps, 2);
        assert_eq!(u.stock.as_ref().unwrap().current_partition.as_deref(), Some("vendor_boot_b"));
        let u = tracker.feed_line_at("Finished. Total time: 4.5s", Duration::from_secs(425));
        assert_eq!(u.stock.unwrap().completed_steps, 3);
    }

    #[test]
    fn inspects_folder_with_multiple_stock_modes() {
        let dir = std::env::temp_dir().join("adb-client-stock-modes-test");
        let images = dir.join("images");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&images).unwrap();
        fs::write(images.join("boot.img"), vec![0u8; 2048]).unwrap();
        fs::write(images.join("anti_version.txt"), "1\n").unwrap();

        let lock_bat = "fastboot %* getvar product 2>&1 | findstr /r /c:\"^product: *unicorn\" || exit /B 1\n\
                        fastboot %* flash boot_ab %~dp0images/boot.img || exit 1\n\
                        fastboot %* oem lock || exit /B 1\n";
        let all_bat = "fastboot %* getvar product 2>&1 | findstr /r /c:\"^product: *unicorn\" || exit /B 1\n\
                       fastboot %* flash boot_ab %~dp0images/boot.img || exit 1\n";
        let keep_bat = "fastboot %* flash boot_ab %~dp0images/boot.img || exit 1\n";

        #[cfg(windows)]
        {
            fs::write(dir.join("flash_all_lock.bat"), lock_bat).unwrap();
            fs::write(dir.join("flash_all.bat"), all_bat).unwrap();
            fs::write(dir.join("flash_all_except_storage.bat"), keep_bat).unwrap();
        }
        #[cfg(not(windows))]
        {
            fs::write(dir.join("flash_all_lock.sh"), lock_bat).unwrap();
            fs::write(dir.join("flash_all.sh"), all_bat).unwrap();
            fs::write(dir.join("flash_all_except_storage.sh"), keep_bat).unwrap();
        }

        let info = inspect_stock_rom_folder(&dir).unwrap();
        assert_eq!(info.mode, StockFlashMode::CleanAllAndLock);
        assert_eq!(info.available_modes.len(), 3);
        assert_eq!(info.expected_product.as_deref(), Some("unicorn"));
        assert_eq!(info.rom_anti_version, Some(1));
        assert_eq!(info.total_bytes, 2048);

        let keep_info = inspect_stock_rom_script(&images, StockFlashMode::KeepUserData).unwrap();
        assert_eq!(keep_info.mode, StockFlashMode::KeepUserData);
        assert!(!keep_info.has_oem_lock);
        assert_eq!(keep_info.total_bytes, 2048);

        let _ = fs::remove_dir_all(&dir);
    }
}
