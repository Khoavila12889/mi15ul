use crate::i18n;
use crate::tabs::MainTab;
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
        .map_err(|e| {
            let err_s = e.to_string();
            i18n::tf("advanced.cannot_start_adb", &[("err", &err_s)])
        })?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(100)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                let err_s = format!("{other:?}");
                let p_s = path.display().to_string();
                return Err(i18n::tf(
                    "advanced.cmd_stopped",
                    &[("err", &err_s), ("path", &p_s)],
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
        let st_s = status.to_string();
        let p_s = path.display().to_string();
        return Err(i18n::tf(
            "advanced.adb_failed",
            &[("status", &st_s), ("text", &text), ("path", &p_s)],
        ));
    }
    let p_s = path.display().to_string();
    Ok(i18n::tf(
        "advanced.saved_report",
        &[("path", &p_s), ("text", &text)],
    ))
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
        self.output = i18n::tf("advanced.running", &[("name", &name), ("serial", &serial)]);
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
                text: result.unwrap_or_else(|e| i18n::tf("common.error_prefix", &[("err", &e)])),
            });
        });
    }
    /// Renders the remaining advanced tools inside the Tools main tab.
    /// Returns a main tab to switch to when the user picks a feature that
    /// now lives in its own tab (HyperOS).
    pub fn show_in(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        serial: Option<&str>,
        flash: &mut super::flash::FlashDialog,
        apk: &mut super::apk_installer::ApkInstallerDialog,
        hyperos: &mut super::hyperos::HyperOsDialog,
    ) -> Option<MainTab> {
        if let Some(rx) = &self.result {
            match rx.try_recv() {
                Ok(r) => {
                    self.output = i18n::tf(
                        "advanced.device_line",
                        &[("serial", &r.serial), ("text", &r.text)],
                    );
                    self.result = None;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.output = i18n::t("advanced.worker_disconnected").into();
                    self.result = None;
                }
                _ => {
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
            }
        }
        if let Some(child) = &mut self.mirror {
            if let Ok(Some(status)) = child.try_wait() {
                let st_s = status.to_string();
                self.output = i18n::tf("advanced.scrcpy_ended", &[("status", &st_s)]);
                self.mirror = None;
            }
        }
        let mut goto: Option<MainTab> = None;
        {
            let ui = &mut *ui;
            {
                ui_elements::tool_card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui_elements::section_header(
                            ui,
                            i18n::t("advanced.workspace_title"),
                            i18n::t("advanced.workspace_sub"),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(s) = serial {
                                ui_elements::status_pill(
                                    ui,
                                    &format!("● {s}"),
                                    colors::ACCENT_GREEN,
                                );
                            } else {
                                ui_elements::status_pill(
                                    ui,
                                    i18n::t("advanced.no_device_pill"),
                                    colors::ACCENT_AMBER,
                                );
                            }
                        });
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
                            i18n::t("advanced.hyperos_title"),
                            i18n::t("advanced.hyperos_sub"),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui_elements::accent_button(
                                ui,
                                i18n::t("advanced.hyperos_open"),
                                colors::ACCENT_BLUE,
                            )
                            .clicked()
                            {
                                goto = Some(MainTab::HyperOs);
                            }
                        });
                    });
                });
                ui.add_space(6.0);
                ui_elements::tool_card(ui, |ui| {
                    ui_elements::section_header(
                        ui,
                        i18n::t("advanced.scrcpy_title"),
                        i18n::t("advanced.scrcpy_sub"),
                    );
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled_ui(serial.is_some() && self.mirror.is_none(), |ui| {
                                ui_elements::accent_button(
                                    ui,
                                    i18n::t("advanced.scrcpy_open"),
                                    colors::ACCENT_BLUE,
                                )
                            })
                            .inner
                            .clicked()
                        {
                            let s = serial.unwrap();
                            let result = (|| -> Result<Child, std::io::Error> {
                                fs::create_dir_all(reports_dir())?;
                                let log = File::create(reports_dir().join("scrcpy.log"))?;
                                let err = log.try_clone()?;
                                command(&self.config.scrcpy)
                                    .env("ADB", &self.config.adb)
                                    .args(["--serial", s, "--max-size", "1280", "--max-fps", "30"])
                                    .stdin(Stdio::null())
                                    .stdout(log)
                                    .stderr(err)
                                    .spawn()
                            })();
                            match result {
                                Ok(c) => {
                                    self.mirror = Some(c);
                                    self.mirror_device = s.into();
                                    self.output = i18n::tf(
                                        "advanced.scrcpy_started",
                                        &[("serial", s)],
                                    );
                                }
                                Err(e) => {
                                    let err_s = e.to_string();
                                    self.output = i18n::tf(
                                        "advanced.scrcpy_err",
                                        &[("err", &err_s)],
                                    );
                                }
                            }
                        }
                        if ui
                            .add_enabled_ui(self.mirror.is_some(), |ui| {
                                ui_elements::accent_button(
                                    ui,
                                    i18n::t("advanced.scrcpy_stop"),
                                    colors::ACCENT_RED,
                                )
                            })
                            .inner
                            .clicked()
                        {
                            if let Some(mut child) = self.mirror.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                        }
                        ui.hyperlink_to(
                            i18n::t("advanced.scrcpy_link"),
                            "https://github.com/Genymobile/scrcpy",
                        );
                    });
                    if self.mirror.is_some() {
                        ui.add_space(2.0);
                        ui_elements::status_pill(
                            ui,
                            &i18n::tf(
                                "advanced.scrcpy_mirroring",
                                &[("serial", &self.mirror_device)],
                            ),
                            colors::ACCENT_GREEN,
                        );
                    }
                });
                ui.add_space(6.0);
                ui_elements::tool_card(ui, |ui| {
                    ui_elements::section_header(
                        ui,
                        i18n::t("advanced.diag_title"),
                        i18n::t("advanced.diag_sub"),
                    );
                    ui.add_space(4.0);
                    ui.add_enabled_ui(serial.is_some() && self.result.is_none(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (label, name, cmd) in [
                                (i18n::t("advanced.diag_cpu"), "cpu", "top -b -n 1 -m 20"),
                                (i18n::t("advanced.diag_mem"), "memory", "dumpsys meminfo"),
                                (i18n::t("advanced.diag_battery"), "battery", "dumpsys battery"),
                                (i18n::t("advanced.diag_storage"), "storage", "df -h /data /sdcard"),
                                (i18n::t("advanced.diag_apps"), "packages", "pm list packages -i"),
                            ] {
                                if ui.button(label).clicked() {
                                    self.start(
                                        serial.unwrap(),
                                        name,
                                        vec!["shell".into(), cmd.into()],
                                        30,
                                    );
                                }
                            }
                        });
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(i18n::t("advanced.package_label"));
                            ui.text_edit_singleline(&mut self.package);
                            if ui
                                .add_enabled(
                                    valid_package(&self.package),
                                    egui::Button::new(i18n::t("advanced.inspect_pkg")),
                                )
                                .clicked()
                            {
                                self.start(
                                    serial.unwrap(),
                                    "package",
                                    vec![
                                        "shell".into(),
                                        "dumpsys".into(),
                                        "package".into(),
                                        self.package.clone(),
                                    ],
                                    30,
                                );
                            }
                        });
                    });
                    if self.result.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(i18n::t("advanced.collecting"));
                        });
                    }
                });
                ui.add_space(6.0);
                ui_elements::tool_card(ui, |ui| {
                    ui.collapsing(i18n::t("advanced.paths_header"), |ui| {
                        ui.label(i18n::t("advanced.path_adb"));
                        ui.text_edit_singleline(&mut self.config.adb);
                        ui.label(i18n::t("advanced.path_fastboot"));
                        ui.text_edit_singleline(&mut self.config.fastboot);
                        ui.label(i18n::t("advanced.path_scrcpy"));
                        ui.text_edit_singleline(&mut self.config.scrcpy);
                        ui.add_space(4.0);
                        if ui_elements::accent_button(
                            ui,
                            i18n::t("advanced.save_paths"),
                            colors::ACCENT_BLUE,
                        )
                        .clicked()
                        {
                            flash.sync_tool_paths(&self.config.adb, &self.config.fastboot);
                            apk.sync_adb_path(&self.config.adb);
                            hyperos.sync_adb_path(&self.config.adb);
                            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                                let path = config_file();
                                fs::create_dir_all(path.parent().unwrap())?;
                                fs::write(path, serde_json::to_vec_pretty(&self.config)?)?;
                                Ok(())
                            })();
                            self.output = match result {
                                Ok(()) => i18n::t("advanced.paths_saved").into(),
                                Err(e) => e.to_string(),
                            };
                        }
                    });
                    ui.collapsing(i18n::t("advanced.repos_header"), |ui| {
                        for (name, description, url) in [
                            ("JADX", i18n::t("advanced.repo_jadx"), "https://github.com/skylot/jadx"),
                            ("Perfetto", i18n::t("advanced.repo_perfetto"), "https://github.com/google/perfetto"),
                            ("UAD-ng", i18n::t("advanced.repo_uad"), "https://github.com/Universal-Debloater-Alliance/universal-android-debloater-next-generation"),
                            ("MobSF", i18n::t("advanced.repo_mobsf"), "https://github.com/MobSF/Mobile-Security-Framework-MobSF"),
                            ("Androguard", i18n::t("advanced.repo_androguard"), "https://github.com/androguard/androguard"),
                            ("Canta", i18n::t("advanced.repo_canta"), "https://github.com/samolego/Canta"),
                        ] {
                            ui.hyperlink_to(name, url);
                            ui.small(description);
                            ui.add_space(6.0);
                        }
                    });
                });
                ui.add_space(6.0);
                ui_elements::tool_card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(i18n::t("advanced.latest_result")).strong());
                        if ui.button(i18n::t("advanced.copy_result")).clicked() {
                            ctx.copy_text(self.output.clone());
                        }
                        let dir_s = reports_dir().display().to_string();
                        ui.small(i18n::tf("advanced.reports_dir", &[("path", &dir_s)]));
                    });
                    egui::ScrollArea::both()
                        .id_salt("advanced_tools_result")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.output)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(8),
                            );
                        });
                });
            }
        }
        goto
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
