use std::path::PathBuf;
use std::time::Duration;

use adb_client::apk::{
    self, ApkFileInfo, ApkInstallOptions, ApkInstallReport, ApkKind, DeviceInstallReadiness,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;

use crate::i18n;
use crate::theme::colors;
use crate::ui_elements;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApkItemStatus {
    Queued,
    Installing,
    Success { elapsed_ms: u64, split_count: usize },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct ApkQueueEntry {
    pub info: ApkFileInfo,
    pub status: ApkItemStatus,
}

enum ApkEvent {
    ReadinessChecked(String, Result<DeviceInstallReadiness, String>),
    PickedPaths(Vec<PathBuf>),
    ItemStarted(usize),
    ItemLog(String),
    ItemFinished {
        index: usize,
        result: Result<ApkInstallReport, String>,
    },
    BatchDone {
        ok: usize,
        fail: usize,
    },
}

pub struct ApkInstallerDialog {
    pub open: bool,
    adb_bin: String,
    selected_serial: Option<String>,
    readiness: Option<Result<DeviceInstallReadiness, String>>,
    checking_readiness: bool,
    options: ApkInstallOptions,
    queue: Vec<ApkQueueEntry>,
    is_installing: bool,
    pending_pickers: usize,
    current_index: Option<usize>,
    completed_count: usize,
    total_batch: usize,
    status: String,
    error: bool,
    log: String,
    tx: Sender<ApkEvent>,
    rx: Receiver<ApkEvent>,
}

impl Default for ApkInstallerDialog {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self {
            open: false,
            adb_bin: apk::resolve_adb(None),
            selected_serial: None,
            readiness: None,
            checking_readiness: false,
            options: ApkInstallOptions::default(),
            queue: Vec::new(),
            is_installing: false,
            pending_pickers: 0,
            current_index: None,
            completed_count: 0,
            total_batch: 0,
            status: "Sẵn sàng. Kéo & thả hoặc chọn file .apk / .apks / .xapk / .apkm.".into(),
            error: false,
            log: String::new(),
            tx,
            rx,
        }
    }
}

impl ApkInstallerDialog {
    pub fn bind_device(&mut self, serial: Option<&str>) {
        let next = serial.map(str::to_string);
        if self.selected_serial != next {
            self.selected_serial = next;
            self.readiness = None;
            if let Some(s) = self.selected_serial.clone() {
                self.refresh_readiness_for(s);
            }
        }
    }

    pub fn sync_adb_path(&mut self, adb: &str) {
        if !self.is_installing && !adb.trim().is_empty() {
            self.adb_bin = adb.trim().to_string();
        }
    }

    pub fn is_busy(&self) -> bool {
        self.is_installing || self.pending_pickers > 0 || self.checking_readiness
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    fn adb_override(&self) -> Option<String> {
        let s = self.adb_bin.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }

    pub fn refresh_readiness(&mut self) {
        if let Some(s) = self.selected_serial.clone() {
            self.refresh_readiness_for(s);
        }
    }

    fn refresh_readiness_for(&mut self, serial: String) {
        self.checking_readiness = true;
        let adb_bin = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = apk::check_device_readiness(adb_bin.as_deref(), &serial);
            let _ = tx.send(ApkEvent::ReadinessChecked(serial, res));
        });
    }

    pub fn enqueue_paths(&mut self, paths: Vec<PathBuf>) -> usize {
        let mut added = 0usize;
        let mut last_err: Option<String> = None;

        for path in paths {
            if path.is_dir() {
                match apk::collect_apks_from_dir(&path) {
                    Ok(items) => {
                        for info in items {
                            if self.push_unique_item(info) {
                                added += 1;
                            }
                        }
                    }
                    Err(e) => last_err = Some(e),
                }
            } else {
                match apk::inspect_apk_file(&path) {
                    Ok(info) => {
                        if self.push_unique_item(info) {
                            added += 1;
                        }
                    }
                    Err(e) => last_err = Some(e),
                }
            }
        }

        if added > 0 {
            self.status = format!("Đã thêm {added} gói vào hàng đợi ({} tổng).", self.queue.len());
            self.error = false;
        } else if let Some(err) = last_err {
            self.status = err;
            self.error = true;
        }
        added
    }

    fn push_unique_item(&mut self, info: ApkFileInfo) -> bool {
        if let Some(existing) = self.queue.iter_mut().find(|e| e.info.path == info.path) {
            if matches!(existing.status, ApkItemStatus::Failed(_)) && !self.is_installing {
                existing.status = ApkItemStatus::Queued;
                return true;
            }
            return false;
        }
        self.queue.push(ApkQueueEntry {
            info,
            status: ApkItemStatus::Queued,
        });
        true
    }

    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.rx.try_recv() {
            changed = true;
            match event {
                ApkEvent::ReadinessChecked(serial, res) => {
                    self.checking_readiness = false;
                    if self.selected_serial.as_deref() == Some(serial.as_str()) {
                        self.readiness = Some(res);
                    }
                }
                ApkEvent::PickedPaths(paths) => {
                    self.pending_pickers = self.pending_pickers.saturating_sub(1);
                    if !paths.is_empty() {
                        self.enqueue_paths(paths);
                    }
                }
                ApkEvent::ItemStarted(idx) => {
                    self.current_index = Some(idx);
                    if let Some(entry) = self.queue.get_mut(idx) {
                        entry.status = ApkItemStatus::Installing;
                        self.status = format!("Đang cài {}…", entry.info.file_name);
                        self.error = false;
                        self.log.push_str(&format!("\n▶ {}\n", entry.info.file_name));
                    }
                }
                ApkEvent::ItemLog(line) => {
                    self.log.push_str(&line);
                    self.log.push('\n');
                }
                ApkEvent::ItemFinished { index, result } => {
                    self.completed_count += 1;
                    if let Some(entry) = self.queue.get_mut(index) {
                        match result {
                            Ok(report) => {
                                entry.status = ApkItemStatus::Success {
                                    elapsed_ms: report.elapsed_ms,
                                    split_count: report.split_count,
                                };
                                self.log.push_str(&format!(
                                    "✓ Thành công ({:.1}s, {} APK)\n",
                                    report.elapsed_ms as f64 / 1000.0,
                                    report.split_count
                                ));
                            }
                            Err(err) => {
                                entry.status = ApkItemStatus::Failed(err.clone());
                                self.log.push_str(&format!("✗ Lỗi: {err}\n"));
                            }
                        }
                    }
                }
                ApkEvent::BatchDone { ok, fail } => {
                    self.is_installing = false;
                    self.current_index = None;
                    if fail == 0 {
                        self.status = format!("Hoàn tất: đã cài thành công {ok} gói.");
                        self.error = false;
                    } else {
                        self.status = format!("Hoàn tất: {ok} thành công, {fail} lỗi.");
                        self.error = true;
                    }
                }
            }
        }
        changed
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }

        let is_hovering_files =
            !self.is_installing && ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !self.is_installing {
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            let paths: Vec<PathBuf> = dropped.into_iter().filter_map(|f| f.path).collect();
            if !paths.is_empty() {
                self.enqueue_paths(paths);
            }
        }

        let mut open = self.open;
        egui::Window::new(i18n::t("apk.window_title"))
            .open(&mut open)
            .default_size([760.0, 620.0])
            .min_width(500.0)
            .resizable(true)
            .vscroll(true)
            .show(ctx, |ui| {
                self.render_full_ui(ui, ctx, is_hovering_files);
            });

        if !open && self.is_installing {
            self.open = true;
            self.status = "Đang cài APK — chờ hoàn tất trước khi đóng cửa sổ.".into();
            self.error = true;
        } else {
            self.open = open;
        }
    }

    pub fn render_embedded_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let is_hovering_files =
            !self.is_installing && ctx.input(|i| !i.raw.hovered_files.is_empty());
        ui.horizontal_wrapped(|ui| {
            self.render_device_badge_row(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Mở cửa sổ APK đầy đủ…").clicked() {
                    self.open = true;
                }
            });
        });

        self.render_miui_warning(ui);
        ui.add_space(4.0);
        self.render_drop_and_pick_bar(ui, is_hovering_files);
        ui.add_space(4.0);
        self.render_options_row(ui);

        if !self.queue.is_empty() {
            ui.add_space(4.0);
            self.render_queue_list(ui, 150.0);
        }

        ui.add_space(4.0);
        self.render_action_and_progress(ui, ctx);
    }

    fn render_full_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hovering_files: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui_elements::section_header(
                    ui,
                    "Thiết bị mục tiêu",
                    "Cài trực tiếp file .apk hoặc bundle .apks / .xapk / .apkm từ máy tính qua ADB",
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            self.selected_serial.is_some() && !self.is_busy(),
                            egui::Button::new("Kiểm tra lại"),
                        )
                        .clicked()
                    {
                        self.refresh_readiness();
                    }
                    if self.checking_readiness {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(4.0);
            self.render_device_badge_row(ui);
            self.render_miui_warning(ui);
        });

        ui.add_space(8.0);

        ui_elements::tool_card(ui, |ui| {
            self.render_drop_and_pick_bar(ui, hovering_files);
            ui.add_space(6.0);
            self.render_options_row(ui);
        });

        ui.add_space(8.0);

        ui_elements::tool_card(ui, |ui| {
            self.render_queue_list(ui, 210.0);
        });

        ui.add_space(8.0);

        ui_elements::tool_card(ui, |ui| {
            self.render_action_and_progress(ui, ctx);
            ui.add_space(4.0);
            egui::CollapsingHeader::new("Nhật ký cài đặt (ADB Log)")
                .id_salt("apk_installer_log")
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.small_button(i18n::t("common.copy_log")).clicked() {
                            ctx.copy_text(self.log.clone());
                        }
                        if ui.small_button(i18n::t("common.clear_log")).clicked() {
                            self.log.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("apk_log_scroll")
                        .max_height(130.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.monospace(&self.log);
                        });
                });
        });
    }

    fn render_device_badge_row(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| match (&self.selected_serial, &self.readiness) {
            (None, _) => {
                ui_elements::status_pill(ui, "Chưa chọn thiết bị ADB", colors::ACCENT_AMBER);
            }
            (Some(serial), None) => {
                ui_elements::status_pill(ui, &format!("● {serial}"), colors::ACCENT_BLUE);
            }
            (Some(_), Some(Ok(r))) => {
                ui_elements::status_pill(
                    ui,
                    &format!("● {} ({})", r.model, r.serial),
                    colors::ACCENT_GREEN,
                );
                if !r.android_version.is_empty() {
                    ui_elements::status_pill(
                        ui,
                        &format!("Android {}", r.android_version),
                        colors::ACCENT_BLUE,
                    );
                }
                if !r.abi.is_empty() {
                    ui_elements::status_pill(ui, &r.abi, colors::TEXT_MUTED);
                }
            }
            (Some(serial), Some(Err(e))) => {
                ui_elements::status_pill(ui, &format!("● {serial}"), colors::ACCENT_RED);
                ui.small(egui::RichText::new(e).color(colors::ERROR));
            }
        });
    }

    fn render_miui_warning(&self, ui: &mut egui::Ui) {
        if let Some(Ok(r)) = &self.readiness {
            if r.miui_adb_install_disabled {
                ui.add_space(4.0);
                ui_elements::tool_subcard(ui, Some(colors::ACCENT_AMBER), |ui| {
                    ui.colored_label(
                        colors::ACCENT_AMBER,
                        "⚠ Xiaomi/HyperOS đang tắt 'Install via USB' (persist.security.adbinstall = 0). Hãy bật mục này trong Developer Options trước khi cài APK.",
                    );
                });
            }
        }
    }

    fn render_drop_and_pick_bar(&mut self, ui: &mut egui::Ui, hovering_files: bool) {
        let border = hovering_files.then_some(colors::ACCENT_BLUE);
        ui_elements::tool_subcard(ui, border, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Kéo & thả file .apk / .apks / .xapk / .apkm hoặc thư mục chứa APK vào đây",
                    )
                    .color(if hovering_files {
                        colors::ACCENT_BLUE
                    } else {
                        colors::TEXT_MUTED
                    }),
                );
            });
            ui.add_space(4.0);
            ui.add_enabled_ui(!self.is_installing, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui_elements::accent_button(ui, "+ Chọn file APK…", colors::ACCENT_BLUE)
                        .clicked()
                    {
                        self.pick_files();
                    }
                    if ui.button("+ Thêm thư mục APK…").clicked() {
                        self.pick_folder();
                    }
                    if self.pending_pickers > 0 {
                        ui.spinner();
                    }
                });
            });
        });
    }

    fn render_options_row(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(!self.is_installing, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.options.replace_existing, "Ghi đè / Cập nhật (-r)");
                ui.checkbox(&mut self.options.grant_permissions, "Tự cấp quyền (-g)");
                ui.checkbox(&mut self.options.allow_downgrade, "Hạ phiên bản (-d)");
                ui.checkbox(&mut self.options.allow_test_packages, i18n::t("apk.flag_test"));
            });
        });
    }

    fn render_queue_list(&mut self, ui: &mut egui::Ui, max_height: f32) {
        let total_bytes: u64 = self.queue.iter().map(|e| e.info.bytes).sum();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Hàng đợi: {} gói ({})",
                    self.queue.len(),
                    format_bytes(total_bytes)
                ))
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!self.is_installing, |ui| {
                    if ui.small_button("Xóa tất cả").clicked() {
                        self.queue.clear();
                    }
                    if ui.small_button("Xóa đã xong").clicked() {
                        self.queue
                            .retain(|e| !matches!(e.status, ApkItemStatus::Success { .. }));
                    }
                    let has_failed = self
                        .queue
                        .iter()
                        .any(|e| matches!(e.status, ApkItemStatus::Failed(_)));
                    if has_failed && ui.small_button("Thử lại gói lỗi").clicked() {
                        for entry in &mut self.queue {
                            if matches!(entry.status, ApkItemStatus::Failed(_)) {
                                entry.status = ApkItemStatus::Queued;
                            }
                        }
                    }
                });
            });
        });

        if self.queue.is_empty() {
            ui.small(
                egui::RichText::new("Chưa có file nào trong hàng đợi.")
                    .color(colors::TEXT_MUTED),
            );
            return;
        }

        ui.add_space(4.0);
        let mut remove_idx: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("apk_queue_scroll")
            .max_height(max_height)
            .show(ui, |ui| {
                for (idx, entry) in self.queue.iter().enumerate() {
                    ui_elements::tool_subcard(ui, None, |ui| {
                        ui.horizontal(|ui| {
                            let kind_label = match entry.info.kind {
                                ApkKind::SingleApk => "APK",
                                ApkKind::SplitBundle => i18n::t("apk.kind_bundle"),
                            };
                            let kind_color = match entry.info.kind {
                                ApkKind::SingleApk => colors::ACCENT_BLUE,
                                ApkKind::SplitBundle => colors::ACCENT_AMBER,
                            };
                            ui_elements::status_pill(ui, kind_label, kind_color);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&entry.info.file_name).strong(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(entry.info.path.display().to_string());
                            ui.small(
                                egui::RichText::new(format_bytes(entry.info.bytes))
                                    .color(colors::TEXT_MUTED),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !self.is_installing && ui.small_button("×").clicked() {
                                        remove_idx = Some(idx);
                                    }
                                    match &entry.status {
                                        ApkItemStatus::Queued => {
                                            ui_elements::status_pill(
                                                ui,
                                                "Chờ cài",
                                                colors::TEXT_MUTED,
                                            );
                                        }
                                        ApkItemStatus::Installing => {
                                            ui_elements::status_pill(
                                                ui,
                                                "Đang cài…",
                                                colors::ACCENT_BLUE,
                                            );
                                            ui.spinner();
                                        }
                                        ApkItemStatus::Success {
                                            elapsed_ms,
                                            split_count,
                                        } => {
                                            let secs = format!("{:.1}", *elapsed_ms as f64 / 1000.0);
                                            let label = if *split_count > 1 {
                                                let count = split_count.to_string();
                                                i18n::tf(
                                                    "apk.status_ok_split",
                                                    &[("secs", &secs), ("count", &count)],
                                                )
                                            } else {
                                                i18n::tf("apk.status_ok", &[("secs", &secs)])
                                            };
                                            ui_elements::status_pill(
                                                ui,
                                                &label,
                                                colors::ACCENT_GREEN,
                                            );
                                        }
                                        ApkItemStatus::Failed(err) => {
                                            ui.scope(|ui| {
                                                ui_elements::status_pill(
                                                    ui,
                                                    i18n::t("apk.status_fail"),
                                                    colors::ACCENT_RED,
                                                );
                                            })
                                            .response
                                            .on_hover_text(err);
                                        }
                                    }
                                },
                            );
                        });
                        if let ApkItemStatus::Failed(err) = &entry.status {
                            ui.small(egui::RichText::new(err).color(colors::ERROR));
                        }
                    });
                }
            });

        if let Some(idx) = remove_idx {
            self.queue.remove(idx);
        }
    }

    fn render_action_and_progress(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pending_count = self
            .queue
            .iter()
            .filter(|e| matches!(e.status, ApkItemStatus::Queued | ApkItemStatus::Failed(_)))
            .count();

        ui.horizontal_wrapped(|ui| {
            let can_install =
                !self.is_installing && self.selected_serial.is_some() && pending_count > 0;
            let btn_label = if self.is_installing {
                format!(
                    "Đang cài ({}/{})…",
                    self.completed_count.min(self.total_batch),
                    self.total_batch
                )
            } else {
                format!("Cài đặt ngay ({pending_count} file)")
            };

            if ui
                .add_enabled_ui(can_install, |ui| {
                    ui_elements::accent_button(ui, &btn_label, colors::ACCENT_GREEN)
                })
                .inner
                .clicked()
            {
                self.start_install_batch();
            }

            if self.is_installing && self.total_batch > 0 {
                let frac = (self.completed_count as f32 / self.total_batch as f32).clamp(0.0, 1.0);
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(220.0)
                        .show_percentage()
                        .animate(true),
                );
            }
        });

        ui.add_space(4.0);
        ui.colored_label(
            if self.error {
                colors::ERROR
            } else {
                colors::LOG_INFO
            },
            &self.status,
        );

        if self.is_installing || self.pending_pickers > 0 || self.checking_readiness {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn start_install_batch(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let mut targets = Vec::new();
        for (idx, entry) in self.queue.iter_mut().enumerate() {
            if matches!(entry.status, ApkItemStatus::Queued | ApkItemStatus::Failed(_)) {
                entry.status = ApkItemStatus::Queued;
                targets.push((idx, entry.info.path.clone()));
            }
        }
        if targets.is_empty() {
            return;
        }

        self.is_installing = true;
        self.completed_count = 0;
        self.total_batch = targets.len();
        self.current_index = None;
        self.error = false;
        self.status = format!("Bắt đầu cài {} gói lên {serial}…", targets.len());

        let adb_bin = self.adb_override();
        let options = self.options.clone();
        let abi = self
            .readiness
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|r| r.abi.clone());
        let tx = self.tx.clone();

        std::thread::spawn(move || {
            let (mut ok, mut fail) = (0usize, 0usize);
            for (idx, path) in targets {
                let _ = tx.send(ApkEvent::ItemStarted(idx));
                let tx_log = tx.clone();
                let res = apk::install_apk(
                    adb_bin.as_deref(),
                    &serial,
                    &path,
                    &options,
                    abi.as_deref(),
                    move |line| {
                        let _ = tx_log.send(ApkEvent::ItemLog(line));
                    },
                );
                if res.is_ok() {
                    ok += 1;
                } else {
                    fail += 1;
                }
                let _ = tx.send(ApkEvent::ItemFinished {
                    index: idx,
                    result: res,
                });
            }
            let _ = tx.send(ApkEvent::BatchDone { ok, fail });
        });
    }

    fn pick_files(&mut self) {
        self.pending_pickers += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let paths = native_pick_apk_files();
            let _ = tx.send(ApkEvent::PickedPaths(paths));
        });
    }

    fn pick_folder(&mut self) {
        self.pending_pickers += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let paths = native_pick_apk_folder().into_iter().collect();
            let _ = tx.send(ApkEvent::PickedPaths(paths));
        });
    }
}

#[cfg(windows)]
fn native_pick_apk_files() -> Vec<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let filter = i18n::t("apk.pick_files_filter");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
        $d = New-Object System.Windows.Forms.OpenFileDialog; \
        $d.Multiselect = $true; \
        $d.Filter = '{filter}'; \
        if ($d.ShowDialog() -eq 'OK') {{ $d.FileNames | ForEach-Object {{ [Console]::Out.WriteLine($_) }} }}"
    );

    let Ok(out) = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(not(windows))]
fn native_pick_apk_files() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(windows)]
fn native_pick_apk_folder() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let desc = i18n::t("apk.pick_folder_desc");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
        $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
        $d.Description = '{desc}'; \
        if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.WriteLine($d.SelectedPath) }}"
    );

    let out = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

#[cfg(not(windows))]
fn native_pick_apk_folder() -> Option<PathBuf> {
    None
}

fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn enqueue_deduplicates_scans_folders_and_resets_failed() {
        let dir = std::env::temp_dir().join("android-dashboard-apk-queue-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("app_a.apk");
        let b = dir.join("app_b.xapk");
        fs::write(&a, b"123").unwrap();
        fs::write(&b, b"456").unwrap();

        let mut dlg = ApkInstallerDialog::default();
        assert_eq!(dlg.enqueue_paths(vec![a.clone(), a.clone()]), 1);
        assert_eq!(dlg.queue_len(), 1);

        // Scan directory adds only `b` (dedupes `a`).
        assert_eq!(dlg.enqueue_paths(vec![dir.clone()]), 1);
        assert_eq!(dlg.queue_len(), 2);

        // Mark `a` as Failed, then re-enqueuing `a` resets it to Queued.
        dlg.queue[0].status = ApkItemStatus::Failed("err".into());
        assert_eq!(dlg.enqueue_paths(vec![a.clone()]), 1);
        assert_eq!(dlg.queue[0].status, ApkItemStatus::Queued);

        let _ = fs::remove_dir_all(&dir);
    }
}
