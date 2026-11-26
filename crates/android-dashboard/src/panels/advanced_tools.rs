use crate::theme::colors;
use crate::ui_elements;
use crossbeam_channel::Receiver;
use eframe::egui;
use std::{
    fs::{self, File},
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn default_fastboot() -> String {
    adb_client::flash::resolve_fastboot(None)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Config {
    adb: String,
    scrcpy: String,
    #[serde(default = "default_fastboot")]
    fastboot: String,
}
impl Default for Config {
    fn default() -> Self {
        let sdk = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("Android/Sdk/platform-tools/adb.exe"));
        Self {
            adb: sdk
                .filter(|p| p.is_file())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "adb".into()),
            scrcpy: "scrcpy".into(),
            fastboot: default_fastboot(),
        }
    }
}
fn config_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("android-dashboard/advanced-tools.json")
}
pub(super) fn reports_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("android-dashboard/reports")
}
pub(super) fn command(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c
}
struct ResultMessage {
    serial: String,
    text: String,
}
pub struct AdvancedTools {
    zalo: super::zalo_cleanup::ZaloCleanup,
    pub open: bool,
    config: Config,
    package: String,
    output: String,
    result: Option<Receiver<ResultMessage>>,
    mirror: Option<Child>,
    mirror_device: String,
}
impl Default for AdvancedTools {
    fn default() -> Self {
        let config = fs::read(config_file())
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        Self {
            zalo: Default::default(),
            open: false,
            config,
            package: String::new(),
            output: String::new(),
            result: None,
            mirror: None,
            mirror_device: String::new(),
        }
    }
}
fn valid_package(p: &str) -> bool {
    p.contains('.')
        && p.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
}
// Redirect both streams to disk so large dumps cannot fill a pipe and block the child.
pub(super) fn run_report(
    adb: &str,
    serial: &str,
    args: &[String],
    timeout: Duration,
    path: &PathBuf,
) -> Result<String, String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    let stderr = file.try_clone().map_err(|e| e.to_string())?;
    let mut child = command(adb)
        .args(["-s", serial])
        .args(args)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("Cannot start ADB: {e}"))?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(100)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Command stopped (timeout or process error: {other:?}). Partial output: {}",
                    path.display()
                ));
            }
        }
    };
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(512 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    if !status.success() {
        return Err(format!(
            "ADB failed ({status}):\n{text}\nLog: {}",
            path.display()
        ));
    }
    Ok(format!("Saved: {}\n\n{text}", path.display()))
}
impl AdvancedTools {
    pub fn tool_paths(&self) -> (&str, &str) {
        (&self.config.adb, &self.config.fastboot)
    }

    fn start(&mut self, serial: &str, name: &str, args: Vec<String>, seconds: u64) {
        let adb = self.config.adb.clone();
        let serial = serial.to_owned();
        let name = name.to_owned();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.result = Some(rx);
        self.output = format!("Running {name} on {serial}…");
        std::thread::spawn(move || {
            let result = (|| {
                fs::create_dir_all(reports_dir()).map_err(|e| e.to_string())?;
                let path = reports_dir().join(format!(
                    "{}-{name}-{}.txt",
                    chrono::Local::now().format("%Y%m%d-%H%M%S-%f"),
                    std::process::id()
                ));
                run_report(&adb, &serial, &args, Duration::from_secs(seconds), &path)
            })();
            let _ = tx.send(ResultMessage {
                serial,
                text: result.unwrap_or_else(|e| format!("Error: {e}")),
            });
        });
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        serial: Option<&str>,
        flash: &mut super::flash::FlashDialog,
        apk: &mut super::apk_installer::ApkInstallerDialog,
        hyperos: &mut super::hyperos::HyperOsDialog,
    ) {
        if let Some(rx) = &self.result {
            match rx.try_recv() {
                Ok(r) => {
                    self.output = format!("Device: {}\n{}", r.serial, r.text);
                    self.result = None;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.output = "Tool worker disconnected.".into();
                    self.result = None;
                }
                _ => {
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
            }
        }
        if let Some(child) = &mut self.mirror {
            if let Ok(Some(status)) = child.try_wait() {
                self.output = format!("scrcpy ended ({status}). See scrcpy log in reports folder.");
                self.mirror = None;
            }
        }
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Advanced Tools").open(&mut open).default_width(820.0).default_height(680.0).vscroll(true).show(ctx, |ui| {
            ui_elements::tool_card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui_elements::section_header(
                        ui,
                        "Android Workspace",
                        "Công cụ điều khiển, cài APK, Flash ROM & chẩn đoán thiết bị cục bộ",
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(s) = serial {
                            ui_elements::status_pill(ui, &format!("● {s}"), colors::ACCENT_GREEN);
                        } else {
                            ui_elements::status_pill(ui, "○ Chưa chọn thiết bị", colors::ACCENT_AMBER);
                        }
                    });
                });
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                egui::CollapsingHeader::new("Quick APK Installer (PC → Android)")
                    .default_open(apk.is_busy() || apk.queue_len() > 0)
                    .show(ui, |ui| {
                        apk.render_embedded_card(ui, ctx);
                    });
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                egui::CollapsingHeader::new("Stock ROM Flash & Bootloader Lock (Fastboot)")
                    .default_open(flash.is_busy())
                    .show(ui, |ui| {
                        flash.render_embedded_stock_module(ui, ctx);
                    });
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                self.zalo.show(ui, serial, &self.config.adb);
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui_elements::section_header(
                        ui,
                        "HyperOS China Fix & Việt Hóa",
                        "Fix thông báo chậm (MILLET/FCM), Việt hóa Locale + MTZ, Safe Debloat User 0 & Khóa bảo mật Bank/VNeID",
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui_elements::accent_button(
                            ui,
                            "Mở HyperOS China Fix & Việt Hóa…",
                            colors::ACCENT_BLUE,
                        )
                        .clicked()
                        {
                            hyperos.open = true;
                        }
                    });
                });
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                ui_elements::section_header(
                    ui,
                    "Screen Control (scrcpy)",
                    "Phản chiếu & điều khiển màn hình điện thoại trên PC",
                );
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled_ui(serial.is_some() && self.mirror.is_none(), |ui| {
                            ui_elements::accent_button(ui, "Open scrcpy", colors::ACCENT_BLUE)
                        })
                        .inner
                        .clicked()
                    {
                        let s = serial.unwrap();
                        let result = (|| -> Result<Child, std::io::Error> {
                            fs::create_dir_all(reports_dir())?;
                            let log = File::create(reports_dir().join("scrcpy.log"))?;
                            let err = log.try_clone()?;
                            command(&self.config.scrcpy).env("ADB", &self.config.adb).args(["--serial", s, "--max-size", "1280", "--max-fps", "30"])
                                .stdin(Stdio::null()).stdout(log).stderr(err).spawn()
                        })();
                        match result { Ok(c) => { self.mirror = Some(c); self.mirror_device = s.into(); self.output = format!("scrcpy started for {s}."); }, Err(e) => self.output = format!("Cannot open scrcpy: {e}. Install it from the official repository and set its path above.") }
                    }
                    if ui
                        .add_enabled_ui(self.mirror.is_some(), |ui| {
                            ui_elements::accent_button(ui, "Stop scrcpy", colors::ACCENT_RED)
                        })
                        .inner
                        .clicked()
                    {
                        if let Some(mut child) = self.mirror.take() { let _ = child.kill(); let _ = child.wait(); }
                    }
                    ui.hyperlink_to("scrcpy repository / download", "https://github.com/Genymobile/scrcpy");
                });
                if self.mirror.is_some() {
                    ui.add_space(2.0);
                    ui_elements::status_pill(
                        ui,
                        &format!("Mirroring: {}", self.mirror_device),
                        colors::ACCENT_GREEN,
                    );
                }
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                ui_elements::section_header(
                    ui,
                    "Diagnostics & Development",
                    "Thu thập thông số hệ thống / quyền ứng dụng một lần qua ADB",
                );
                ui.add_space(4.0);
                ui.add_enabled_ui(serial.is_some() && self.result.is_none(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (label, name, cmd) in [
                            ("CPU / processes", "cpu", "top -b -n 1 -m 20"),
                            ("Memory", "memory", "dumpsys meminfo"),
                            ("Battery", "battery", "dumpsys battery"),
                            ("Storage", "storage", "df -h /data /sdcard"),
                            ("Installed apps", "packages", "pm list packages -i"),
                        ] {
                            if ui.button(label).clicked() { self.start(serial.unwrap(), name, vec!["shell".into(), cmd.into()], 30); }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Package"); ui.text_edit_singleline(&mut self.package);
                        if ui.add_enabled(valid_package(&self.package), egui::Button::new("Inspect permissions / app")).clicked() {
                            self.start(serial.unwrap(), "package", vec!["shell".into(), "dumpsys".into(), "package".into(), self.package.clone()], 30);
                        }
                    });
                });
                if self.result.is_some() { ui.horizontal(|ui| { ui.spinner(); ui.label("Collecting report… (up to 30 seconds)"); }); }
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                ui.collapsing("Tool paths", |ui| {
                    ui.label("ADB executable"); ui.text_edit_singleline(&mut self.config.adb);
                    ui.label("fastboot executable"); ui.text_edit_singleline(&mut self.config.fastboot);
                    ui.label("scrcpy executable (install separately, then enter full path)"); ui.text_edit_singleline(&mut self.config.scrcpy);
                    ui.add_space(4.0);
                    if ui_elements::accent_button(ui, "Save paths", colors::ACCENT_BLUE).clicked() {
                        flash.sync_tool_paths(&self.config.adb, &self.config.fastboot);
                        apk.sync_adb_path(&self.config.adb);
                        hyperos.sync_adb_path(&self.config.adb);
                        let result = (|| -> Result<(), Box<dyn std::error::Error>> { let path = config_file(); fs::create_dir_all(path.parent().unwrap())?; fs::write(path, serde_json::to_vec_pretty(&self.config)?)?; Ok(()) })();
                        self.output = match result { Ok(()) => "Tool paths saved.".into(), Err(e) => e.to_string() };
                    }
                });
                ui.collapsing("Repository library — install / integrate separately", |ui| {
                    for (name, description, url) in [
                        ("JADX", "Inspect APK / DEX Java code and resources", "https://github.com/skylot/jadx"),
                        ("Perfetto", "Trace CPU scheduling, performance and latency", "https://github.com/google/perfetto"),
                        ("UAD-ng", "Community package descriptions and debloat workflow", "https://github.com/Universal-Debloater-Alliance/universal-android-debloater-next-generation"),
                        ("MobSF", "Static / dynamic mobile app security analysis", "https://github.com/MobSF/Mobile-Security-Framework-MobSF"),
                        ("Androguard", "Programmatic APK / DEX analysis", "https://github.com/androguard/androguard"),
                        ("Canta", "On-device debloat using Shizuku", "https://github.com/samolego/Canta"),
                    ] { ui.hyperlink_to(name, url); ui.small(description); ui.add_space(6.0); }
                });
            });
            ui.add_space(6.0);
            ui_elements::tool_card(ui, |ui| {
                ui.horizontal_wrapped(|ui| { ui.label(egui::RichText::new("Latest result").strong()); if ui.button("Copy result").clicked() { ctx.copy_text(self.output.clone()); } ui.small(format!("Reports: {}", reports_dir().display())); });
                egui::ScrollArea::both().id_salt("advanced_tools_result").max_height(240.0).show(ui, |ui| { ui.add(egui::TextEdit::multiline(&mut self.output).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(8)); });
            });
        });
        self.open = open;
    }
}
impl Drop for AdvancedTools {
    fn drop(&mut self) {
        if let Some(mut child) = self.mirror.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_input_cannot_inject_remote_shell_commands() {
        assert!(valid_package("com.android.settings"));
        for p in [
            "",
            "com..app",
            "com.app;reboot",
            "com.app other",
            "com.app\nreboot",
            "$(reboot).app",
        ] {
            assert!(!valid_package(p), "{p}");
        }
    }

    #[test]
    fn deserializes_legacy_config_without_fastboot() {
        let legacy = r#"{"adb":"C:\\adb.exe","scrcpy":"scrcpy"}"#;
        let cfg: Config = serde_json::from_str(legacy).expect("legacy config should parse");
        assert_eq!(cfg.adb, r"C:\adb.exe");
        assert_eq!(cfg.scrcpy, "scrcpy");
        assert!(!cfg.fastboot.is_empty());
    }
}
