use crate::i18n;
use crate::ui_elements;
use adb_client::transfer::{
    self, RemoteFolder, TransferDirection, TransferEvent, TransferJob, TransferRequest,
};
use crossbeam_channel::Receiver;
use eframe::egui;
use std::path::PathBuf;
use std::time::Instant;

struct Listing {
    serial: String,
    path: String,
    result: Result<Vec<RemoteFolder>, String>,
}

pub struct BackupDialog {
    serial: Option<String>,
    remote: String,
    local: String,
    folders: Vec<RemoteFolder>,
    local_folders: Vec<PathBuf>,
    listing: Option<Receiver<Listing>>,
    job: Option<TransferJob>,
    request: Option<TransferRequest>,
    started: Option<Instant>,
    compression: bool,
    status: String,
    output: String,
    error: bool,
    local_missing: bool,
    manifest: Option<PathBuf>,
}

impl Default for BackupDialog {
    fn default() -> Self {
        Self {
            serial: None,
            remote: "/sdcard".into(),
            local: dirs::home_dir()
                .unwrap_or_default()
                .join("Android Backups")
                .display()
                .to_string(),
            folders: Vec::new(),
            local_folders: Vec::new(),
            listing: None,
            job: None,
            request: None,
            started: None,
            compression: true,
            status: i18n::t("backup.default_status").into(),
            output: String::new(),
            error: false,
            local_missing: false,
            manifest: None,
        }
    }
}

impl BackupDialog {
    pub fn bind_device(&mut self, serial: Option<&str>) {
        if self.serial.as_deref() != serial {
            if let Some(job) = &self.job {
                job.cancel();
            }
            self.serial = serial.map(str::to_owned);
            self.folders.clear();
            self.listing = None;
            self.remote = "/sdcard".into();
            self.refresh_remote();
        }
    }

    fn refresh_remote(&mut self) {
        let Some(serial) = self.serial.clone() else {
            return;
        };
        let path = self.remote.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.folders.clear();
        self.listing = Some(rx);
        std::thread::spawn(move || {
            let result = transfer::list_remote_folders(&serial, &path);
            let _ = tx.send(Listing {
                serial,
                path,
                result,
            });
        });
    }

    fn refresh_local(&mut self) {
        let path = PathBuf::from(&self.local);
        if !path.is_absolute() {
            self.local_folders.clear();
            return;
        }
        if !path.exists() {
            self.local_folders.clear();
            let p_str = path.display().to_string();
            self.status = i18n::tf("backup.folder_not_exist", &[("path", &p_str)]);
            self.error = false;
            self.local_missing = true;
            return;
        }
        match transfer::local_folders(&path) {
            Ok(folders) => {
                self.local_folders = folders;
                if self.local_missing || (self.error && self.status.contains("os error")) {
                    self.error = false;
                    self.local_missing = false;
                    self.status = i18n::t("backup.default_status").into();
                }
            }
            Err(error) => {
                self.local_folders.clear();
                self.local_missing = false;
                self.fail(error);
            }
        }
    }

    fn fail(&mut self, message: String) {
        self.status = message;
        self.error = true;
    }

    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(result) = self.listing.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.listing = None;
            if self.serial.as_deref() == Some(&result.serial) && self.remote == result.path {
                match result.result {
                    Ok(folders) => self.folders = folders,
                    Err(e) => self.fail(e),
                }
            }
            changed = true;
        }
        let events: Vec<_> = self
            .job
            .as_ref()
            .map(|j| j.rx.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            changed = true;
            match event {
                TransferEvent::Output(text) => {
                    self.output.push_str(&text.replace('\r', "\n"));
                    if self.output.len() > 32000 {
                        self.output = self
                            .output
                            .chars()
                            .rev()
                            .take(16000)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                    }
                }
                TransferEvent::Finished(result) => {
                    self.job = None;
                    self.error = result.is_err();
                    let success = result.is_ok();
                    self.status = result.unwrap_or_else(|e| e);
                    if let (Some(path), Some(request)) = (&self.manifest, &self.request) {
                        let record = serde_json::json!({"status": if success {"completed"} else {"incomplete"},
                            "direction": format!("{:?}", request.direction), "device": request.serial,
                            "phone": request.remote, "pc": request.local, "compression": request.compression,
                            "finished_at": chrono::Local::now().to_rfc3339(), "result": self.status});
                        if let Err(e) = std::fs::write(
                            path,
                            serde_json::to_vec_pretty(&record).unwrap_or_default(),
                        ) {
                            let err_s = e.to_string();
                            self.status.push_str(&i18n::tf(
                                "backup.manifest_save_fail",
                                &[("err", &err_s)],
                            ));
                        }
                    }
                    self.refresh_local();
                }
            }
        }
        changed
    }

    fn start(&mut self, direction: TransferDirection) {
        let Some(serial) = self.serial.clone() else {
            return;
        };
        if let Err(e) = transfer::validate_remote(&self.remote) {
            self.fail(e);
            return;
        }
        let local = PathBuf::from(&self.local);
        if !local.is_absolute() {
            self.fail(i18n::t("backup.err_abs_pc").into());
            return;
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
        let token = format!("{stamp}-{}", std::process::id());
        let mut request = TransferRequest {
            serial,
            direction,
            remote: self.remote.clone(),
            local: local.clone(),
            compression: self.compression,
        };
        if direction == TransferDirection::Pull {
            if let Err(e) = std::fs::create_dir_all(&local) {
                let p = local.display().to_string();
                let err_s = e.to_string();
                self.fail(i18n::tf(
                    "backup.err_create_dest",
                    &[("path", &p), ("err", &err_s)],
                ));
                return;
            }
            let snapshot = local.join(format!("backup-{token}"));
            if let Err(e) = std::fs::create_dir(&snapshot) {
                let p = snapshot.display().to_string();
                let err_s = e.to_string();
                self.fail(i18n::tf(
                    "backup.err_create_sub",
                    &[("path", &p), ("err", &err_s)],
                ));
                return;
            }
            self.manifest = Some(snapshot.join("transfer-manifest.json"));
            request.local = snapshot;
        } else {
            if !local.is_dir() {
                self.fail(i18n::t("backup.err_pick_source").into());
                return;
            }
            request.remote = format!("{}/from-pc-{token}", self.remote.trim_end_matches('/'));
            self.manifest = ai_insight::history_path()
                .and_then(|p| p.parent().map(|p| p.join(format!("transfer-{token}.json"))));
            if let Some(parent) = self.manifest.as_ref().and_then(|p| p.parent()) {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    self.fail(e.to_string());
                    return;
                }
            }
        }
        // Persist an incomplete marker before starting so interruption never looks successful.
        if let Some(path) = &self.manifest {
            let marker = serde_json::json!({"status":"incomplete", "device":request.serial, "phone":request.remote, "pc":request.local, "started_at":chrono::Local::now().to_rfc3339()});
            if let Err(e) = std::fs::write(path, marker.to_string()) {
                let err_s = e.to_string();
                self.fail(i18n::tf("backup.err_manifest", &[("err", &err_s)]));
                return;
            }
        }
        let dest_display = if direction == TransferDirection::Pull {
            request.local.display().to_string()
        } else {
            request.remote.clone()
        };
        self.status = i18n::tf(
            "backup.copying",
            &[("serial", &request.serial), ("dest", &dest_display)],
        );
        self.output.clear();
        self.error = false;
        self.local_missing = false;
        self.started = Some(Instant::now());
        self.job = Some(TransferJob::start(request.clone()));
        self.request = Some(request);
    }

    /// Renders the backup / transfer UI inside the Tools tab.
    pub fn show_in(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(ui, i18n::t("backup.title"), "");
            ui.add_space(4.0);
            {
                let dev = self
                    .serial
                    .as_deref()
                    .unwrap_or_else(|| i18n::t("backup.no_device"));
                ui.label(i18n::tf("backup.header", &[("device", dev)]));
                ui.label(i18n::t("backup.subtitle"));
                let busy = self.job.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    ui.columns(2, |columns| {
                        let ui = &mut columns[0];
                        ui.heading(i18n::t("backup.phone_folder"));
                        ui.add(egui::TextEdit::singleline(&mut self.remote).desired_width(f32::INFINITY));
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(i18n::t("backup.up")).clicked() {
                                self.remote = self
                                    .remote
                                    .trim_end_matches('/')
                                    .rsplit_once('/')
                                    .map(|(p, _)| if p.is_empty() { "/" } else { p })
                                    .unwrap_or("/")
                                    .to_string();
                                self.refresh_remote();
                            }
                            if ui
                                .add_enabled(
                                    self.serial.is_some() && self.listing.is_none(),
                                    egui::Button::new(i18n::t("backup.browse_refresh")),
                                )
                                .clicked()
                            {
                                self.refresh_remote();
                            }
                        });
                        if self.listing.is_some() {
                            ui.spinner();
                        }
                        let mut navigate = None;
                        egui::ScrollArea::both()
                            .id_salt("phone_folders")
                            .max_height(210.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for folder in &self.folders {
                                    let label = i18n::tf(
                                        "backup.folder_item",
                                        &[("name", &folder.name)],
                                    );
                                    if ui
                                        .selectable_label(false, label)
                                        .on_hover_text(&folder.path)
                                        .clicked()
                                    {
                                        navigate = Some(folder.path.clone());
                                    }
                                }
                            });
                        if let Some(path) = navigate {
                            self.remote = path;
                            self.refresh_remote();
                        }
                        let ui = &mut columns[1];
                        ui.heading(i18n::t("backup.pc_folder"));
                        ui.add(egui::TextEdit::singleline(&mut self.local).desired_width(f32::INFINITY));
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(i18n::t("backup.up")).clicked() {
                                if let Some(parent) = PathBuf::from(&self.local).parent() {
                                    self.local = parent.display().to_string();
                                    self.refresh_local();
                                }
                            }
                            let mut drive = None;
                            ui.menu_button(i18n::t("backup.drives"), |ui| {
                                for root in pc_roots() {
                                    if ui.button(root.display().to_string()).clicked() {
                                        drive = Some(root);
                                        ui.close_menu();
                                    }
                                }
                            });
                            if let Some(root) = drive {
                                self.local = root.display().to_string();
                                self.refresh_local();
                            }
                            if ui.button(i18n::t("backup.browse_refresh")).clicked() {
                                self.refresh_local();
                            }
                            let local_p = PathBuf::from(&self.local);
                            if local_p.is_absolute() && !local_p.exists() {
                                if ui.button(i18n::t("backup.create_folder")).clicked() {
                                    match std::fs::create_dir_all(&local_p) {
                                        Ok(()) => {
                                            let p = local_p.display().to_string();
                                            self.status =
                                                i18n::tf("backup.created_folder", &[("path", &p)]);
                                            self.error = false;
                                            self.local_missing = false;
                                            self.refresh_local();
                                        }
                                        Err(e) => {
                                            let err_s = e.to_string();
                                            self.fail(i18n::tf(
                                                "backup.err_create_folder",
                                                &[("err", &err_s)],
                                            ));
                                        }
                                    }
                                }
                            }
                        });
                        let mut navigate = None;
                        egui::ScrollArea::both()
                            .id_salt("pc_folders")
                            .max_height(210.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for folder in &self.local_folders {
                                    let name = folder
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy();
                                    let label =
                                        i18n::tf("backup.folder_item", &[("name", &name)]);
                                    if ui
                                        .selectable_label(false, label)
                                        .on_hover_text(folder.display().to_string())
                                        .clicked()
                                    {
                                        navigate = Some(folder.display().to_string());
                                    }
                                }
                            });
                        if let Some(path) = navigate {
                            self.local = path;
                            self.refresh_local();
                        }
                    });
                    ui.checkbox(&mut self.compression, i18n::t("backup.compression"));
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                self.serial.is_some(),
                                egui::Button::new(i18n::t("backup.pull_btn")),
                            )
                            .clicked()
                        {
                            self.start(TransferDirection::Pull);
                        }
                        if ui
                            .add_enabled(
                                self.serial.is_some(),
                                egui::Button::new(i18n::t("backup.push_btn")),
                            )
                            .clicked()
                        {
                            self.start(TransferDirection::Push);
                        }
                    });
                    ui.small(i18n::t("backup.note"));
                });
                ui.separator();
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let secs = format!(
                            "{:.0}",
                            self.started
                                .map(|s| s.elapsed().as_secs_f64())
                                .unwrap_or_default()
                        );
                        ui.label(i18n::tf("backup.transferring", &[("secs", &secs)]));
                        if ui.button(i18n::t("backup.cancel")).clicked() {
                            if let Some(job) = &self.job {
                                job.cancel();
                            }
                        }
                    });
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                ui.colored_label(
                    if self.error {
                        egui::Color32::LIGHT_RED
                    } else {
                        egui::Color32::LIGHT_BLUE
                    },
                    &self.status,
                );
                if let Some(path) = &self.manifest {
                    let p = path.display().to_string();
                    ui.small(i18n::tf("backup.record", &[("path", &p)]));
                }
                egui::ScrollArea::vertical()
                    .id_salt("transfer_output")
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.monospace(&self.output);
                    });
            }
        });
    }
}

/// Enumerate drive letters without probing potentially disconnected network drives.
#[cfg(windows)]
fn pc_roots() -> Vec<PathBuf> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    // This Win32 call takes no pointers and returns a bit mask of mounted drive letters.
    let mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        .collect()
}

#[cfg(not(windows))]
fn pc_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}
