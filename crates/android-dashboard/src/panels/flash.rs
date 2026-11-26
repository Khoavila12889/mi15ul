use crate::audit;
use crate::i18n;
use crate::theme::colors;
use crate::ui_elements;
use adb_client::Adb;
use adb_client::flash::{
    self, Device, FlashOptions, LockMethod, Manifest, Plan, ProgressUpdate, SideloadDevice,
    StockFlashMode, StockFlashProgress, StockLockInfo, StockStepKind,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SAMPLE_MANIFEST_JSON: &str = r#"{
  "product": "your_device_codename",
  "build": "custom-build-2026",
  "mode": "bootloader",
  "images": [
    {
      "partition": "boot_a",
      "file": "boot.img",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
    },
    {
      "partition": "vbmeta_a",
      "file": "vbmeta.img",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
    }
  ]
}"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlashTab {
    FullRom,
    SinglePartition,
    AdbSideload,
    ManifestGenerator,
    LockBootloader,
    Guide,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PickTarget {
    ManifestJson,
    SingleImage,
    SideloadZip,
    GeneratorFolder,
    StockLockFolder,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotTarget {
    None,
    SlotA,
    SlotB,
}

impl SlotTarget {
    fn as_suffix(self) -> Option<&'static str> {
        match self {
            SlotTarget::None => None,
            SlotTarget::SlotA => Some("_a"),
            SlotTarget::SlotB => Some("_b"),
        }
    }
}

struct FastbootRefresh {
    devices: Vec<String>,
    probe: Option<Result<Device, String>>,
}

enum FlashEvent {
    FastbootRefreshed(Result<FastbootRefresh, String>),
    SideloadRefreshed(Result<Vec<SideloadDevice>, String>),
    DeviceProbed(Result<Device, String>),
    PlanPrepared(Result<Plan, String>),
    SingleInspected(Result<(u64, String, bool), String>),
    SideloadInspected(Result<(u64, String), String>),
    ManifestGenerated(Result<(Manifest, PathBuf), String>),
    StockLockInspected(Result<StockLockInfo, String>),
    PickedPath {
        target: PickTarget,
        path: Option<String>,
    },
    Progress(ProgressUpdate),
    QuickActionDone(Result<String, String>),
    WriteFinished(Result<String, String>),
}

pub struct FlashDialog {
    pub open: bool,
    tab: FlashTab,
    // Tool binaries
    fastboot_bin: String,
    adb_bin: String,
    // Connected devices
    fastboot_devices: Vec<String>,
    selected_fastboot: Option<String>,
    selected_adb: Option<String>,
    device_info: Option<Result<Device, String>>,
    sideload_devices: Vec<SideloadDevice>,
    selected_sideload: Option<String>,
    // Tab 1: Full ROM Manifest
    manifest_path: String,
    plan_verified: Option<Plan>,
    full_options: FlashOptions,
    confirm_full_flash: bool,
    // Tab 2: Single Partition / Temporary Boot
    partition: String,
    single_image_path: String,
    single_options: FlashOptions,
    single_inspection: Option<Result<(u64, String, bool), String>>,
    confirm_single_flash: bool,
    // Tab 3: ADB Sideload
    sideload_zip_path: String,
    sideload_inspection: Option<Result<(u64, String), String>>,
    confirm_sideload: bool,
    // Tab 4: Manifest Generator
    gen_folder: String,
    gen_product: String,
    gen_build: String,
    gen_mode: String,
    gen_slot: SlotTarget,
    gen_result: Option<Result<(Manifest, PathBuf), String>>,
    // Tab 5: Stock ROM & Lock Bootloader
    lock_rom_folder: String,
    stock_mode: StockFlashMode,
    lock_stock_info: Option<Result<StockLockInfo, String>>,
    confirm_stock_lock: bool,
    lock_confirm_text: String,
    direct_lock_method: LockMethod,
    confirm_direct_lock: bool,
    direct_lock_text: String,
    // Async channel & execution state
    tx: Sender<FlashEvent>,
    rx: Receiver<FlashEvent>,
    is_flashing: bool,
    pending_bg_tasks: usize,
    started: Option<Instant>,
    progress_percent: Option<f32>,
    progress_label: String,
    stock_progress: Option<StockFlashProgress>,
    status: String,
    output: String,
    error: bool,
    log_path: Option<PathBuf>,
}

fn reports_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("android-dashboard/flash-reports")
}

impl Default for FlashDialog {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self {
            open: false,
            tab: FlashTab::FullRom,
            fastboot_bin: flash::resolve_fastboot(None),
            adb_bin: flash::resolve_adb(None),
            fastboot_devices: Vec::new(),
            selected_fastboot: None,
            selected_adb: None,
            device_info: None,
            sideload_devices: Vec::new(),
            selected_sideload: None,
            manifest_path: String::new(),
            plan_verified: None,
            full_options: FlashOptions::default(),
            confirm_full_flash: false,
            partition: "boot".into(),
            single_image_path: String::new(),
            single_options: FlashOptions::default(),
            single_inspection: None,
            confirm_single_flash: false,
            sideload_zip_path: String::new(),
            sideload_inspection: None,
            confirm_sideload: false,
            gen_folder: String::new(),
            gen_product: String::new(),
            gen_build: "custom-build-1".into(),
            gen_mode: "bootloader".into(),
            gen_slot: SlotTarget::None,
            gen_result: None,
            lock_rom_folder: String::new(),
            stock_mode: StockFlashMode::CleanAllAndLock,
            lock_stock_info: None,
            confirm_stock_lock: false,
            lock_confirm_text: String::new(),
            direct_lock_method: LockMethod::OemLock,
            confirm_direct_lock: false,
            direct_lock_text: String::new(),
            tx,
            rx,
            is_flashing: false,
            pending_bg_tasks: 0,
            started: None,
            progress_percent: None,
            progress_label: String::new(),
            stock_progress: None,
            status: i18n::t("flash.ready_status").into(),
            output: String::new(),
            error: false,
            log_path: None,
        }
    }
}

impl FlashDialog {
    pub fn bind_adb_device(&mut self, serial: Option<&str>) {
        if self.selected_adb.as_deref() != serial {
            self.selected_adb = serial.map(str::to_owned);
        }
    }

    fn fastboot_override(&self) -> Option<String> {
        let trimmed = self.fastboot_bin.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn adb_override(&self) -> Option<String> {
        let trimmed = self.adb_bin.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn invalidate_full_plan(&mut self) {
        self.plan_verified = None;
        self.confirm_full_flash = false;
    }

    fn invalidate_single(&mut self, clear_inspection: bool) {
        if clear_inspection {
            self.single_inspection = None;
        }
        self.confirm_single_flash = false;
    }

    fn invalidate_sideload(&mut self, clear_inspection: bool) {
        if clear_inspection {
            self.sideload_inspection = None;
        }
        self.confirm_sideload = false;
    }

    /// Asynchronously refreshes both Fastboot devices and ADB sideload/recovery devices.
    pub fn refresh_fastboot_devices(&mut self) {
        if self.is_flashing {
            return;
        }
        self.status = i18n::t("flash.scanning").into();
        self.error = false;
        self.pending_bg_tasks += 2;

        let tx_fb = self.tx.clone();
        let fb_bin = self.fastboot_override();
        let current_sel = self.selected_fastboot.clone();
        std::thread::spawn(move || {
            let res = flash::devices_with_bin(fb_bin.as_deref()).map(|devices| {
                let target = if let Some(sel) = current_sel.filter(|s| devices.contains(s)) {
                    Some(sel)
                } else {
                    devices.first().cloned()
                };
                let probe = target
                    .as_deref()
                    .map(|serial| flash::probe_info_with_bin(fb_bin.as_deref(), serial));
                FastbootRefresh { devices, probe }
            });
            let _ = tx_fb.send(FlashEvent::FastbootRefreshed(res));
        });

        let tx_sl = self.tx.clone();
        let adb_bin = self.adb_override();
        std::thread::spawn(move || {
            let res = flash::sideload_devices_with_bin(adb_bin.as_deref());
            let _ = tx_sl.send(FlashEvent::SideloadRefreshed(res));
        });
    }

    pub fn probe_device(&mut self, serial: &str) {
        if self.is_flashing {
            return;
        }
        self.pending_bg_tasks += 1;
        let tx = self.tx.clone();
        let fb_bin = self.fastboot_override();
        let serial = serial.to_string();
        std::thread::spawn(move || {
            let res = flash::probe_info_with_bin(fb_bin.as_deref(), &serial);
            let _ = tx.send(FlashEvent::DeviceProbed(res));
        });
    }

    fn apply_device_probe(&mut self, res: Result<Device, String>) {
        if let Ok(dev) = &res {
            if self.gen_product.is_empty() {
                self.gen_product = dev.product.clone();
            }
            if dev.slot == "a" && self.gen_slot == SlotTarget::None {
                self.gen_slot = SlotTarget::SlotA;
            } else if dev.slot == "b" && self.gen_slot == SlotTarget::None {
                self.gen_slot = SlotTarget::SlotB;
            }
        }
        self.device_info = Some(res);
    }

    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.rx.try_recv() {
            changed = true;
            match event {
                FlashEvent::FastbootRefreshed(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    match res {
                        Ok(FastbootRefresh { devices, probe }) => {
                            let prev = self.selected_fastboot.clone();
                            if !devices
                                .contains(self.selected_fastboot.as_ref().unwrap_or(&String::new()))
                            {
                                self.selected_fastboot = devices.first().cloned();
                            }
                            if self.selected_fastboot != prev {
                                self.invalidate_full_plan();
                                self.invalidate_single(false);
                            }
                            self.fastboot_devices = devices;
                            if let Some(p) = probe {
                                self.apply_device_probe(p);
                            } else {
                                self.device_info = None;
                            }
                            if !self.is_flashing {
                                let fb_s = self.fastboot_devices.len().to_string();
                                let sl_s = self.sideload_devices.len().to_string();
                                self.status = i18n::tf(
                                    "flash.scan_summary",
                                    &[("fb", &fb_s), ("sl", &sl_s)],
                                );
                                self.error = false;
                            }
                        }
                        Err(e) => {
                            self.fastboot_devices.clear();
                            self.selected_fastboot = None;
                            self.device_info = Some(Err(e.clone()));
                            self.status = i18n::tf("flash.scan_failed", &[("err", &e)]);
                            self.error = true;
                        }
                    }
                }
                FlashEvent::SideloadRefreshed(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    match res {
                        Ok(devices) => {
                            let still_present = self
                                .selected_sideload
                                .as_ref()
                                .is_some_and(|s| devices.iter().any(|d| &d.serial == s));
                            if !still_present {
                                self.selected_sideload = devices.first().map(|d| d.serial.clone());
                            }
                            self.sideload_devices = devices;
                        }
                        Err(_) => {
                            self.sideload_devices.clear();
                            self.selected_sideload = None;
                        }
                    }
                }
                FlashEvent::DeviceProbed(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    self.apply_device_probe(res);
                }
                FlashEvent::PlanPrepared(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    self.progress_percent = None;
                    self.progress_label.clear();
                    match res {
                        Ok(plan) => {
                            let count_s = plan.manifest().images.len().to_string();
                            let mb_s = format!("{:.2}", plan.total_bytes() as f64 / (1024.0 * 1024.0));
                            self.status = i18n::tf(
                                "flash.plan_verified",
                                &[
                                    ("product", &plan.manifest().product),
                                    ("count", &count_s),
                                    ("mb", &mb_s),
                                ],
                            );
                            self.error = false;
                            self.plan_verified = Some(plan);
                            self.confirm_full_flash = false;
                        }
                        Err(e) => {
                            self.status = i18n::tf("flash.plan_failed", &[("err", &e)]);
                            self.error = true;
                            self.plan_verified = None;
                        }
                    }
                }
                FlashEvent::SingleInspected(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    self.single_inspection = Some(res);
                }
                FlashEvent::SideloadInspected(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    self.sideload_inspection = Some(res);
                }
                FlashEvent::ManifestGenerated(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    self.progress_percent = None;
                    self.progress_label.clear();
                    match res {
                        Ok((manifest, path)) => {
                            self.manifest_path = path.display().to_string();
                            self.invalidate_full_plan();
                            let count_s = manifest.images.len().to_string();
                            let path_s = path.display().to_string();
                            self.status = i18n::tf(
                                "flash.gen_done",
                                &[("count", &count_s), ("path", &path_s)],
                            );
                            self.error = false;
                            self.gen_result = Some(Ok((manifest, path)));
                        }
                        Err(e) => {
                            self.status = i18n::tf("flash.gen_err", &[("err", &e)]);
                            self.error = true;
                            self.gen_result = Some(Err(e));
                        }
                    }
                }
                FlashEvent::StockLockInspected(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    match &res {
                        Ok(info) => {
                            self.stock_mode = info.mode;
                            let steps_s = info.flash_command_count.to_string();
                            let size_s = format_bytes_human(info.total_bytes);
                            let prod_s = info
                                .expected_product
                                .as_deref()
                                .unwrap_or_else(|| i18n::t("flash.stock_unspecified"));
                            self.status = i18n::tf(
                                "flash.stock_detected",
                                &[
                                    ("mode", i18n::stock_mode_label(info.mode)),
                                    ("steps", &steps_s),
                                    ("size", &size_s),
                                    ("product", prod_s),
                                ],
                            );
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = i18n::tf("flash.stock_inspect_failed", &[("err", e)]);
                            self.error = true;
                        }
                    }
                    self.lock_stock_info = Some(res);
                    self.confirm_stock_lock = false;
                    self.lock_confirm_text.clear();
                }
                FlashEvent::PickedPath { target, path } => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    if let Some(p) = path.filter(|s| !s.trim().is_empty()) {
                        match target {
                            PickTarget::ManifestJson => {
                                self.manifest_path = p;
                                self.invalidate_full_plan();
                            }
                            PickTarget::SingleImage => {
                                self.single_image_path = p;
                                self.invalidate_single(true);
                            }
                            PickTarget::SideloadZip => {
                                self.sideload_zip_path = p;
                                self.invalidate_sideload(true);
                            }
                            PickTarget::GeneratorFolder => {
                                self.gen_folder = p;
                            }
                            PickTarget::StockLockFolder => {
                                self.lock_rom_folder = p;
                                self.lock_stock_info = None;
                                self.confirm_stock_lock = false;
                                self.lock_confirm_text.clear();
                            }
                        }
                    }
                }
                FlashEvent::Progress(upd) => {
                    if let Some(pct) = upd.percent {
                        self.progress_percent = Some(pct);
                    }
                    self.stock_progress = upd.stock;
                    self.progress_label = if upd.total > 0 {
                        format!("[{}/{}] {}", upd.step, upd.total, upd.message)
                    } else {
                        upd.message.clone()
                    };
                    self.status = upd.message.clone();
                    self.output.push_str(&upd.message);
                    self.output.push('\n');
                }
                FlashEvent::QuickActionDone(res) => {
                    self.pending_bg_tasks = self.pending_bg_tasks.saturating_sub(1);
                    match res {
                        Ok(msg) => {
                            self.status = msg.clone();
                            self.error = false;
                            self.output.push_str(&msg);
                            self.output.push('\n');
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.error = true;
                            self.output.push_str(&format!("LỖI: {err}\n"));
                        }
                    }
                }
                FlashEvent::WriteFinished(res) => {
                    self.is_flashing = false;
                    self.started = None;
                    self.confirm_full_flash = false;
                    self.confirm_single_flash = false;
                    self.confirm_sideload = false;
                    self.confirm_stock_lock = false;
                    self.lock_confirm_text.clear();
                    self.confirm_direct_lock = false;
                    self.direct_lock_text.clear();
                    match res {
                        Ok(msg) => {
                            self.progress_percent = Some(100.0);
                            self.status = i18n::tf("flash.completed", &[("msg", &msg)]);
                            self.error = false;
                            self.output.push_str(&format!("\n=== THÀNH CÔNG: {msg} ===\n"));
                        }
                        Err(err) => {
                            self.progress_percent = None;
                            self.status = i18n::tf("flash.failed", &[("err", &err)]);
                            self.error = true;
                            self.output.push_str(&format!("\n=== LỖI: {err} ===\n"));
                        }
                    }
                }
            }
        }
        changed
    }

    pub fn open_tab(&mut self, tab: FlashTab) {
        self.open = true;
        self.tab = tab;
        if !self.is_flashing {
            self.refresh_fastboot_devices();
        }
    }

    pub fn sync_tool_paths(&mut self, adb: &str, fastboot: &str) {
        if self.is_flashing {
            return;
        }
        if !adb.trim().is_empty() {
            self.adb_bin = adb.trim().to_string();
        }
        if !fastboot.trim().is_empty() {
            self.fastboot_bin = fastboot.trim().to_string();
        }
    }

    pub fn is_busy(&self) -> bool {
        self.is_flashing || self.pending_bg_tasks > 0
    }

    pub fn show(&mut self, ctx: &egui::Context, adb_serial: Option<&str>) {
        self.bind_adb_device(adb_serial);
        if self.tick() {
            ctx.request_repaint();
        }

        if !self.open {
            return;
        }

        self.handle_dropped_files(ctx);

        let mut open = self.open;
        egui::Window::new(i18n::t("flash.window_title"))
            .open(&mut open)
            .default_size([920.0, 740.0])
            .min_width(540.0)
            .resizable(true)
            .vscroll(true)
            .show(ctx, |ui| {
                self.render_ui(ui, ctx);
            });

        if !open && self.is_flashing {
            self.open = true;
            self.status = i18n::t("flash.busy_close").into();
            self.error = true;
        } else {
            self.open = open;
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        if self.is_flashing {
            return;
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            let Some(path) = file.path else {
                continue;
            };
            let display = path.display().to_string();
            if path.is_dir() {
                if flash::inspect_stock_rom_folder(&path).is_ok() {
                    self.lock_rom_folder = display;
                    self.lock_stock_info = None;
                    self.confirm_stock_lock = false;
                    self.lock_confirm_text.clear();
                    self.tab = FlashTab::LockBootloader;
                    self.start_inspect_stock_lock();
                } else {
                    self.gen_folder = display;
                    self.tab = FlashTab::ManifestGenerator;
                    self.status = i18n::t("flash.drop_gen").into();
                    self.error = false;
                }
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                match ext.to_ascii_lowercase().as_str() {
                    "json" => {
                        self.manifest_path = display;
                        self.invalidate_full_plan();
                        self.tab = FlashTab::FullRom;
                        self.status = i18n::t("flash.drop_manifest").into();
                        self.error = false;
                    }
                    "img" => {
                        self.single_image_path = display;
                        self.invalidate_single(true);
                        self.tab = FlashTab::SinglePartition;
                        self.status = i18n::t("flash.drop_img").into();
                        self.error = false;
                    }
                    "zip" => {
                        self.sideload_zip_path = display;
                        self.invalidate_sideload(true);
                        self.tab = FlashTab::AdbSideload;
                        self.status = i18n::t("flash.drop_zip").into();
                        self.error = false;
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn render_embedded_stock_module(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let is_busy = self.is_busy();

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!is_busy, egui::Button::new(i18n::t("flash.emb_refresh")))
                .clicked()
            {
                self.refresh_fastboot_devices();
            }
            if let Some(adb) = self.selected_adb.clone() {
                if ui
                    .add_enabled(!is_busy, egui::Button::new(i18n::t("flash.emb_reboot_bl")))
                    .clicked()
                {
                    self.run_adb_reboot(adb, Some("bootloader"));
                }
            }
            if ui.button(i18n::t("flash.emb_open_full")).clicked() {
                self.open_tab(FlashTab::LockBootloader);
            }
            if self.pending_bg_tasks > 0 && !self.is_flashing {
                ui.spinner();
            }
        });

        self.render_fastboot_connection_bar(ui, is_busy);

        if self.is_flashing || self.progress_percent.is_some() || self.stock_progress.is_some() {
            self.render_live_progress_block(ui, ctx);
        }

        ui.colored_label(
            if self.error {
                egui::Color32::LIGHT_RED
            } else {
                egui::Color32::LIGHT_BLUE
            },
            &self.status,
        );

        ui.add_space(4.0);
        ui.add_enabled_ui(!is_busy, |ui| {
            self.render_stock_rom_selector_and_run(ui);
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.small(i18n::t("flash.emb_other"));
            if ui.small_button(i18n::t("flash.tab_full")).clicked() {
                self.open_tab(FlashTab::FullRom);
            }
            if ui.small_button(i18n::t("flash.tab_single")).clicked() {
                self.open_tab(FlashTab::SinglePartition);
            }
            if ui.small_button(i18n::t("flash.tab_sideload")).clicked() {
                self.open_tab(FlashTab::AdbSideload);
            }
            if ui.small_button(i18n::t("flash.tab_guide")).clicked() {
                self.open_tab(FlashTab::Guide);
            }
        });
    }

    fn render_fastboot_connection_bar(&mut self, ui: &mut egui::Ui, is_busy: bool) {
        ui.horizontal_wrapped(|ui| {
            if self.fastboot_devices.is_empty() {
                ui.label(
                    egui::RichText::new(i18n::t("flash.bar_none"))
                        .color(egui::Color32::YELLOW),
                );
            } else {
                ui.label("Fastboot:");
                let mut probe_target = None;
                for d in &self.fastboot_devices {
                    let is_sel = self.selected_fastboot.as_ref() == Some(d);
                    if ui
                        .add_enabled(!is_busy, egui::SelectableLabel::new(is_sel, d))
                        .clicked()
                    {
                        self.selected_fastboot = Some(d.clone());
                        probe_target = Some(d.clone());
                    }
                }
                if let Some(serial) = probe_target {
                    self.invalidate_full_plan();
                    self.invalidate_single(false);
                    self.probe_device(&serial);
                }
            }
        });

        if let Some(res) = &self.device_info {
            match res {
                Ok(dev) => {
                    ui_elements::tool_card(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui_elements::status_pill(
                                ui,
                                &i18n::tf("flash.bar_product", &[("product", &dev.product)]),
                                colors::ACCENT_BLUE,
                            );
                            ui_elements::status_pill(
                                ui,
                                &i18n::tf("flash.bar_mode", &[("mode", &dev.mode)]),
                                colors::TEXT_MUTED,
                            );
                            let slot_s = if dev.slot.is_empty() { "N/A" } else { &dev.slot };
                            ui_elements::status_pill(
                                ui,
                                &i18n::tf("flash.bar_slot", &[("slot", slot_s)]),
                                colors::TEXT_MUTED,
                            );
                            if dev.unlocked {
                                ui_elements::status_pill(
                                    ui,
                                    i18n::t("flash.bar_unlocked"),
                                    colors::ACCENT_GREEN,
                                );
                            } else {
                                ui_elements::status_pill(
                                    ui,
                                    i18n::t("flash.bar_locked"),
                                    colors::ACCENT_RED,
                                );
                            }
                        });
                        let mut extras = Vec::new();
                        if let Some(v) = &dev.battery_voltage {
                            extras.push(i18n::tf("flash.bar_battery", &[("val", v)]));
                        }
                        if let Some(ok) = dev.battery_soc_ok {
                            let val = if ok {
                                i18n::t("flash.bar_battery_yes")
                            } else {
                                i18n::t("flash.bar_battery_low")
                            };
                            extras.push(i18n::tf("flash.bar_battery_ok", &[("val", val)]));
                        }
                        if let Some(sz) = dev.max_download_size {
                            let mb = format!("{:.0}", sz as f64 / (1024.0 * 1024.0));
                            extras.push(i18n::tf("flash.bar_max_dl", &[("mb", &mb)]));
                        }
                        if let Some(anti) = &dev.anti_rollback {
                            extras.push(i18n::tf("flash.bar_arb", &[("val", anti)]));
                        }
                        if let Some(bl) = &dev.version_bootloader {
                            extras.push(i18n::tf("flash.bar_bl_ver", &[("val", bl)]));
                        }
                        if !extras.is_empty() {
                            ui.add_space(2.0);
                            ui.small(
                                egui::RichText::new(extras.join("  |  ")).color(colors::TEXT_MUTED),
                            );
                        }
                        if dev.battery_soc_ok == Some(false) {
                            ui.colored_label(colors::ERROR, i18n::t("flash.bar_low_warn"));
                        }
                    });
                }
                Err(err) => {
                    ui.colored_label(
                        colors::ERROR,
                        i18n::tf("flash.bar_probe_err", &[("err", err)]),
                    );
                }
            }
        }
    }

    fn render_live_progress_block(&self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if self.is_flashing {
                ui.spinner();
                let secs = format!(
                    "{:.0}",
                    self.started
                        .map(|s| s.elapsed().as_secs_f64())
                        .unwrap_or_default()
                );
                ui.label(i18n::tf("flash.prog_running", &[("secs", &secs)]));
            }
            if let Some(pct) = self.progress_percent {
                ui.add(
                    egui::ProgressBar::new((pct / 100.0).clamp(0.0, 1.0))
                        .show_percentage()
                        .animate(self.is_flashing),
                );
            }
        });
        if !self.progress_label.is_empty() && self.is_flashing {
            ui.small(&self.progress_label);
        }
        if let Some(sp) = &self.stock_progress {
            ui_elements::tool_card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let active_text = match (&sp.current_partition, sp.current_chunk) {
                        (Some(part), Some((cur, tot))) => {
                            let c = cur.to_string();
                            let t = tot.to_string();
                            i18n::tf(
                                "flash.prog_active_chunk",
                                &[("part", part), ("cur", &c), ("tot", &t)],
                            )
                        }
                        (Some(part), None) => i18n::tf("flash.prog_active", &[("part", part)]),
                        (None, _) if sp.completed_steps >= sp.total_steps && sp.total_steps > 0 => {
                            i18n::t("flash.prog_done").into()
                        }
                        (None, _) => i18n::t("flash.prog_preparing").into(),
                    };
                    ui_elements::status_pill(ui, &active_text, colors::ACCENT_GREEN);
                    let done_s = sp.completed_steps.to_string();
                    let tot_s = sp.total_steps.to_string();
                    ui.label(i18n::tf(
                        "flash.prog_steps",
                        &[("done", &done_s), ("total", &tot_s)],
                    ));
                    if sp.total_bytes > 0 {
                        ui.label(format!(
                            "| {} / {}",
                            format_bytes_human(sp.completed_bytes),
                            format_bytes_human(sp.total_bytes)
                        ));
                    }
                    if let Some(eta) = sp.eta_seconds {
                        if eta > 0 {
                            let eta_s = format_duration_short(eta);
                            ui_elements::status_pill(
                                ui,
                                &i18n::tf("flash.prog_eta", &[("eta", &eta_s)]),
                                colors::ACCENT_BLUE,
                            );
                        }
                    }
                });
                if !sp.remaining_summary.is_empty() {
                    let shown: Vec<&str> = sp
                        .remaining_summary
                        .iter()
                        .take(10)
                        .map(String::as_str)
                        .collect();
                    let extra = sp.remaining_summary.len().saturating_sub(10);
                    let suffix = if extra > 0 {
                        let ex_s = extra.to_string();
                        i18n::tf("flash.prog_remaining_more", &[("extra", &ex_s)])
                    } else {
                        String::new()
                    };
                    let count_s = sp.remaining_summary.len().to_string();
                    let list_s = shown.join(", ");
                    ui.add_space(2.0);
                    ui.small(
                        egui::RichText::new(i18n::tf(
                            "flash.prog_remaining",
                            &[("count", &count_s), ("list", &list_s), ("suffix", &suffix)],
                        ))
                        .color(colors::TEXT_MUTED),
                    );
                }
            });
        }
        if self.is_flashing || self.pending_bg_tasks > 0 {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let is_busy = self.is_flashing || self.pending_bg_tasks > 0;

        // 1. Header & Device Connection
        ui.horizontal_wrapped(|ui| {
            ui.heading(i18n::t("flash.header_devices"));
            if ui
                .add_enabled(!is_busy, egui::Button::new(i18n::t("flash.btn_refresh")))
                .clicked()
            {
                self.refresh_fastboot_devices();
            }
            if self.pending_bg_tasks > 0 && !self.is_flashing {
                ui.spinner();
            }
        });

        self.render_fastboot_connection_bar(ui, is_busy);

        ui.add_space(4.0);

        // 2. Quick Reboot, A/B Slot Switch & Tool Paths
        ui.collapsing(i18n::t("flash.collapse_reboot"), |ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(adb) = self.selected_adb.clone() {
                        ui.label(i18n::tf("flash.from_adb", &[("adb", &adb)]));
                        if ui.button(i18n::t("flash.adb_reboot_bl")).clicked() {
                            self.run_adb_reboot(adb.clone(), Some("bootloader"));
                        }
                        if ui.button(i18n::t("flash.adb_reboot_fbd")).clicked() {
                            self.run_adb_reboot(adb.clone(), Some("fastboot"));
                        }
                        if ui.button(i18n::t("flash.adb_reboot_rec")).clicked() {
                            self.run_adb_reboot(adb, Some("recovery"));
                        }
                    } else {
                        ui.small(i18n::t("flash.no_adb_selected"));
                    }
                });

                if let Some(fb) = self.selected_fastboot.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(i18n::tf("flash.from_fb", &[("fb", &fb)]));
                        if ui.button(i18n::t("flash.fb_reboot_sys")).clicked() {
                            self.run_fastboot_reboot(fb.clone(), "");
                        }
                        if ui.button(i18n::t("flash.fb_reboot_bl")).clicked() {
                            self.run_fastboot_reboot(fb.clone(), "bootloader");
                        }
                        if ui.button(i18n::t("flash.fb_reboot_fbd")).clicked() {
                            self.run_fastboot_reboot(fb.clone(), "fastboot");
                        }
                        if ui.button(i18n::t("flash.fb_reboot_rec")).clicked() {
                            self.run_fastboot_reboot(fb.clone(), "recovery");
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        ui.label(i18n::t("flash.ab_slot"));
                        if ui.button(i18n::t("flash.set_slot_a")).clicked() {
                            self.run_set_active_slot(fb.clone(), "a");
                        }
                        if ui.button(i18n::t("flash.set_slot_b")).clicked() {
                            self.run_set_active_slot(fb, "b");
                        }
                    });
                }

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(i18n::t("flash.bin_fastboot"));
                    ui.add(egui::TextEdit::singleline(&mut self.fastboot_bin).desired_width(320.0));
                    ui.label(i18n::t("flash.bin_adb"));
                    ui.add(egui::TextEdit::singleline(&mut self.adb_bin).desired_width(240.0));
                });
            });
        });

        ui.separator();

        // 3. Mode Tabs
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tab, FlashTab::FullRom, i18n::t("flash.tab_full"));
            ui.selectable_value(
                &mut self.tab,
                FlashTab::SinglePartition,
                i18n::t("flash.tab_single"),
            );
            ui.selectable_value(
                &mut self.tab,
                FlashTab::AdbSideload,
                i18n::t("flash.tab_sideload"),
            );
            ui.selectable_value(
                &mut self.tab,
                FlashTab::ManifestGenerator,
                i18n::t("flash.tab_gen"),
            );
            ui.selectable_value(
                &mut self.tab,
                FlashTab::LockBootloader,
                i18n::t("flash.tab_stock_lock"),
            );
            ui.selectable_value(&mut self.tab, FlashTab::Guide, i18n::t("flash.tab_guide"));
        });
        ui.small(i18n::t("flash.drop_tip"));

        ui.separator();

        // 4. Tab Contents
        if self.tab == FlashTab::Guide {
            self.render_guide_tab(ui, ctx);
        } else {
            ui.add_enabled_ui(!is_busy, |ui| match self.tab {
                FlashTab::FullRom => self.render_full_rom_tab(ui),
                FlashTab::SinglePartition => self.render_single_partition_tab(ui),
                FlashTab::AdbSideload => self.render_sideload_tab(ui),
                FlashTab::ManifestGenerator => self.render_generator_tab(ui),
                FlashTab::LockBootloader => self.render_lock_bootloader_tab(ui),
                FlashTab::Guide => {}
            });
        }

        ui.separator();

        // 5. Execution State, Progress Bar & Log Console
        if self.is_flashing || self.progress_percent.is_some() || self.stock_progress.is_some() {
            self.render_live_progress_block(ui, ctx);
        }

        ui.colored_label(
            if self.error {
                egui::Color32::LIGHT_RED
            } else {
                egui::Color32::LIGHT_BLUE
            },
            &self.status,
        );

        ui.horizontal_wrapped(|ui| {
            if let Some(log) = &self.log_path {
                let p = log.display().to_string();
                ui.small(i18n::tf("flash.journal_log", &[("path", &p)]));
            }
            if ui.small_button(i18n::t("flash.open_logs_folder")).clicked() {
                open_folder_in_os(&reports_dir());
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n::t("flash.console_output"));
            if ui.button(i18n::t("flash.clear_console")).clicked() {
                self.output.clear();
            }
            if ui.button(i18n::t("flash.copy_console")).clicked() {
                ctx.copy_text(self.output.clone());
            }
        });

        egui::ScrollArea::vertical()
            .id_salt("flash_console_output")
            .max_height(180.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.monospace(&self.output);
            });
    }

    fn render_full_rom_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(i18n::t("flash.full_desc"));
        ui.horizontal(|ui| {
            ui.label(i18n::t("flash.full_manifest_path"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.manifest_path)
                        .desired_width(ui.available_width() - 90.0),
                )
                .changed()
            {
                self.invalidate_full_plan();
            }
            if ui.button(i18n::t("common.browse")).clicked() {
                self.pick_path(PickTarget::ManifestJson);
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.full_options.disable_verity,
                "vbmeta: --disable-verity",
            );
            ui.checkbox(
                &mut self.full_options.disable_verification,
                "vbmeta: --disable-verification",
            );
        });

        ui.horizontal(|ui| {
            let can_verify =
                self.selected_fastboot.is_some() && !self.manifest_path.trim().is_empty();
            if ui
                .add_enabled(can_verify, egui::Button::new(i18n::t("flash.full_verify_btn")))
                .clicked()
            {
                self.start_prepare_plan();
            }
        });

        if let Some(plan) = self.plan_verified.clone() {
            ui.group(|ui| {
                let mb_s = format!("{:.2}", plan.total_bytes() as f64 / (1024.0 * 1024.0));
                ui.label(
                    egui::RichText::new(i18n::tf(
                        "flash.full_plan_box",
                        &[
                            ("product", &plan.manifest().product),
                            ("build", &plan.manifest().build),
                            ("mode", &plan.manifest().mode),
                            ("mb", &mb_s),
                        ],
                    ))
                    .strong(),
                );
                for (idx, (img, size)) in
                    plan.manifest().images.iter().zip(plan.sizes()).enumerate()
                {
                    let mb = *size as f64 / (1024.0 * 1024.0);
                    ui.label(format!(
                        "[{}] {} ← {} ({mb:.2} MB) | SHA-256: {}…",
                        idx + 1,
                        img.partition,
                        img.file,
                        &img.sha256[..12]
                    ));
                }
            });

            ui.add_space(4.0);
            ui.checkbox(&mut self.confirm_full_flash, i18n::t("flash.full_confirm"));

            let can_flash = self.confirm_full_flash && self.selected_fastboot.is_some();
            if ui
                .add_enabled_ui(can_flash, |ui| {
                    ui_elements::accent_button(
                        ui,
                        i18n::t("flash.full_start_btn"),
                        colors::ACCENT_BLUE,
                    )
                })
                .inner
                .clicked()
            {
                self.start_full_flash(plan);
            }
        }
    }

    fn render_single_partition_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(i18n::t("flash.single_desc"));

        let active_slot = self
            .device_info
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|d| d.slot.as_str())
            .unwrap_or("");

        ui.horizontal_wrapped(|ui| {
            ui.label(i18n::t("flash.single_presets"));
            let presets: &[&str] = if active_slot == "a" {
                &[
                    "boot_a",
                    "init_boot_a",
                    "vendor_boot_a",
                    "dtbo_a",
                    "recovery_a",
                    "vbmeta_a",
                    "super",
                ]
            } else if active_slot == "b" {
                &[
                    "boot_b",
                    "init_boot_b",
                    "vendor_boot_b",
                    "dtbo_b",
                    "recovery_b",
                    "vbmeta_b",
                    "super",
                ]
            } else {
                &[
                    "boot",
                    "boot_a",
                    "boot_b",
                    "init_boot",
                    "vendor_boot",
                    "recovery",
                    "vbmeta",
                    "super",
                ]
            };
            for p in presets {
                if ui.button(*p).clicked() {
                    self.partition = (*p).into();
                    self.confirm_single_flash = false;
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n::t("flash.single_target"));
            if ui
                .add(egui::TextEdit::singleline(&mut self.partition).desired_width(180.0))
                .changed()
            {
                self.confirm_single_flash = false;
            }
            if flash::is_vbmeta_partition(self.partition.trim()) {
                ui.checkbox(&mut self.single_options.disable_verity, "--disable-verity");
                ui.checkbox(
                    &mut self.single_options.disable_verification,
                    "--disable-verification",
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n::t("flash.single_img_path"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.single_image_path)
                        .desired_width(ui.available_width() - 90.0),
                )
                .changed()
            {
                self.invalidate_single(true);
            }
            if ui.button(i18n::t("common.browse")).clicked() {
                self.pick_path(PickTarget::SingleImage);
            }
        });

        ui.horizontal(|ui| {
            let path_set = !self.single_image_path.trim().is_empty();
            if ui
                .add_enabled(
                    path_set,
                    egui::Button::new(i18n::t("flash.single_inspect_btn")),
                )
                .clicked()
            {
                self.start_inspect_single();
            }
        });

        if let Some(res) = &self.single_inspection {
            match res {
                Ok((size, hash, is_sparse)) => {
                    ui.group(|ui| {
                        let mb_s = format!("{:.2}", *size as f64 / (1024.0 * 1024.0));
                        let bytes_s = size.to_string();
                        let fmt_s = if *is_sparse {
                            i18n::t("flash.single_sparse")
                        } else {
                            i18n::t("flash.single_raw")
                        };
                        ui.label(i18n::tf(
                            "flash.single_size_fmt",
                            &[("mb", &mb_s), ("bytes", &bytes_s), ("fmt", fmt_s)],
                        ));
                        ui.label(format!("SHA-256: {hash}"));
                    });
                }
                Err(e) => {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        i18n::tf("flash.single_inspect_err", &[("err", e)]),
                    );
                }
            }
        }

        ui.add_space(4.0);
        let dev_name = self
            .selected_fastboot
            .as_deref()
            .unwrap_or_else(|| i18n::t("flash.single_device_fallback"));
        ui.checkbox(
            &mut self.confirm_single_flash,
            i18n::tf(
                "flash.single_confirm",
                &[("part", &self.partition), ("dev", dev_name)],
            ),
        );

        let can_flash = self.confirm_single_flash
            && self.selected_fastboot.is_some()
            && !self.single_image_path.trim().is_empty()
            && !self.partition.trim().is_empty();

        let can_temp_boot =
            self.selected_fastboot.is_some() && !self.single_image_path.trim().is_empty();

        ui.horizontal(|ui| {
            if ui
                .add_enabled_ui(can_flash, |ui| {
                    ui_elements::accent_button(
                        ui,
                        &i18n::tf("flash.single_flash_btn", &[("part", &self.partition)]),
                        colors::ACCENT_BLUE,
                    )
                })
                .inner
                .clicked()
            {
                self.start_single_flash();
            }

            if ui
                .add_enabled(
                    can_temp_boot,
                    egui::Button::new(i18n::t("flash.single_temp_boot")),
                )
                .on_hover_text(i18n::t("flash.single_temp_hover"))
                .clicked()
            {
                self.start_temp_boot();
            }
        });
    }

    fn render_sideload_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(i18n::t("flash.sl_desc"));

        ui.horizontal_wrapped(|ui| {
            if self.sideload_devices.is_empty() {
                ui.colored_label(egui::Color32::YELLOW, i18n::t("flash.sl_none"));
            } else {
                ui.label(i18n::t("flash.sl_target"));
                for d in &self.sideload_devices {
                    let is_sel = self.selected_sideload.as_deref() == Some(d.serial.as_str());
                    let label = format!("{} ({})", d.serial, d.state);
                    if ui.selectable_label(is_sel, label).clicked() {
                        self.selected_sideload = Some(d.serial.clone());
                        self.confirm_sideload = false;
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n::t("flash.sl_zip_path"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.sideload_zip_path)
                        .desired_width(ui.available_width() - 90.0),
                )
                .changed()
            {
                self.invalidate_sideload(true);
            }
            if ui.button(i18n::t("common.browse")).clicked() {
                self.pick_path(PickTarget::SideloadZip);
            }
        });

        ui.horizontal(|ui| {
            let path_set = !self.sideload_zip_path.trim().is_empty();
            if ui
                .add_enabled(path_set, egui::Button::new(i18n::t("flash.sl_inspect_btn")))
                .clicked()
            {
                self.start_inspect_sideload();
            }
        });

        if let Some(res) = &self.sideload_inspection {
            match res {
                Ok((size, hash)) => {
                    ui.group(|ui| {
                        let mb_s = format!("{:.2}", *size as f64 / (1024.0 * 1024.0));
                        let bytes_s = size.to_string();
                        ui.label(i18n::tf(
                            "flash.sl_size",
                            &[("mb", &mb_s), ("bytes", &bytes_s)],
                        ));
                        ui.label(format!("SHA-256: {hash}"));
                    });
                }
                Err(e) => {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        i18n::tf("flash.single_inspect_err", &[("err", e)]),
                    );
                }
            }
        }

        ui.add_space(4.0);
        let dev_name = self
            .selected_sideload
            .as_deref()
            .unwrap_or_else(|| i18n::t("flash.single_device_fallback"));
        ui.checkbox(
            &mut self.confirm_sideload,
            i18n::tf("flash.sl_confirm", &[("dev", dev_name)]),
        );

        let is_in_sideload_state = self.selected_sideload.as_deref().is_some_and(|serial| {
            self.sideload_devices
                .iter()
                .any(|d| d.serial == serial && d.state == "sideload")
        });

        let can_sideload = self.confirm_sideload
            && is_in_sideload_state
            && !self.sideload_zip_path.trim().is_empty();

        if !is_in_sideload_state && self.selected_sideload.is_some() {
            ui.small(i18n::t("flash.sl_recovery_hint"));
        }

        if ui
            .add_enabled_ui(can_sideload, |ui| {
                ui_elements::accent_button(
                    ui,
                    i18n::t("flash.sl_start_btn"),
                    colors::ACCENT_GREEN,
                )
            })
            .inner
            .clicked()
        {
            self.start_adb_sideload();
        }
    }

    fn render_generator_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(i18n::t("flash.gen_desc"));

        ui.horizontal(|ui| {
            ui.label(i18n::t("flash.gen_folder"));
            ui.add(
                egui::TextEdit::singleline(&mut self.gen_folder)
                    .desired_width(ui.available_width() - 90.0),
            );
            if ui.button(i18n::t("common.browse")).clicked() {
                self.pick_path(PickTarget::GeneratorFolder);
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(i18n::t("flash.gen_product"));
            ui.add(egui::TextEdit::singleline(&mut self.gen_product).desired_width(180.0));
            if let Some(Ok(dev)) = &self.device_info {
                if ui
                    .button(i18n::tf("flash.gen_use_product", &[("product", &dev.product)]))
                    .clicked()
                {
                    self.gen_product = dev.product.clone();
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label(i18n::t("flash.gen_build"));
            ui.add(egui::TextEdit::singleline(&mut self.gen_build).desired_width(180.0));
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(i18n::t("flash.gen_mode"));
            ui.radio_value(
                &mut self.gen_mode,
                "bootloader".into(),
                i18n::t("flash.gen_mode_bl"),
            );
            ui.radio_value(
                &mut self.gen_mode,
                "fastbootd".into(),
                i18n::t("flash.gen_mode_fbd"),
            );
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(i18n::t("flash.gen_slot"));
            ui.radio_value(
                &mut self.gen_slot,
                SlotTarget::None,
                i18n::t("flash.gen_slot_none"),
            );
            ui.radio_value(&mut self.gen_slot, SlotTarget::SlotA, i18n::t("flash.gen_slot_a"));
            ui.radio_value(&mut self.gen_slot, SlotTarget::SlotB, i18n::t("flash.gen_slot_b"));
        });

        let can_generate = !self.gen_folder.trim().is_empty()
            && !self.gen_product.trim().is_empty()
            && !self.gen_build.trim().is_empty();

        if ui
            .add_enabled(can_generate, egui::Button::new(i18n::t("flash.gen_btn")))
            .clicked()
        {
            self.start_generate_manifest();
        }

        if let Some(res) = &self.gen_result {
            match res {
                Ok((m, p)) => {
                    ui.group(|ui| {
                        let path_s = p.display().to_string();
                        ui.label(
                            egui::RichText::new(i18n::tf("flash.gen_ok", &[("path", &path_s)]))
                                .color(egui::Color32::GREEN),
                        );
                        ui.label(i18n::tf(
                            "flash.gen_meta",
                            &[
                                ("product", &m.product),
                                ("build", &m.build),
                                ("mode", &m.mode),
                            ],
                        ));
                        for img in &m.images {
                            ui.label(format!("• {} ← {}", img.partition, img.file));
                        }
                        if ui.button(i18n::t("flash.gen_switch")).clicked() {
                            self.tab = FlashTab::FullRom;
                        }
                    });
                }
                Err(err) => {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        i18n::tf("common.error_prefix", &[("err", err)]),
                    );
                }
            }
        }
    }

    fn render_stock_rom_selector_and_run(&mut self, ui: &mut egui::Ui) {
        let dev_ok = self
            .device_info
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .cloned();
        let is_bootloader_unlocked = dev_ok
            .as_ref()
            .is_some_and(|d| d.mode == "bootloader" && d.unlocked);

        if let Some(dev) = &dev_ok {
            if dev.mode != "bootloader" {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    i18n::tf("flash.stock_wrong_mode", &[("mode", &dev.mode)]),
                );
            } else if !dev.unlocked {
                ui.colored_label(
                    egui::Color32::GREEN,
                    i18n::t("flash.stock_already_locked"),
                );
            }
        }

        ui.group(|ui| {
            ui.label(egui::RichText::new(i18n::t("flash.stock_title")).strong());
            ui.small(i18n::t("flash.stock_sub"));

            ui.horizontal(|ui| {
                ui.label(i18n::t("flash.stock_folder"));
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.lock_rom_folder)
                            .desired_width(ui.available_width() - 90.0),
                    )
                    .changed()
                {
                    self.lock_stock_info = None;
                    self.confirm_stock_lock = false;
                    self.lock_confirm_text.clear();
                }
                if ui.button(i18n::t("common.browse")).clicked() {
                    self.pick_path(PickTarget::StockLockFolder);
                }
            });

            let can_inspect = !self.lock_rom_folder.trim().is_empty();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_inspect,
                        egui::Button::new(i18n::t("flash.stock_inspect_btn")),
                    )
                    .clicked()
                {
                    self.start_inspect_stock_lock();
                }
            });

            if let Some(res) = self.lock_stock_info.clone() {
                match res {
                    Ok(info) => {
                        ui.add_space(2.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(i18n::t("flash.stock_script_mode"));
                            for mode in StockFlashMode::ALL {
                                let available = info.available_modes.contains(&mode);
                                let mut selected_mode = self.stock_mode;
                                let color = match mode {
                                    StockFlashMode::CleanAllAndLock => egui::Color32::LIGHT_RED,
                                    StockFlashMode::CleanAll => egui::Color32::YELLOW,
                                    StockFlashMode::KeepUserData => egui::Color32::LIGHT_GREEN,
                                };
                                let label =
                                    egui::RichText::new(i18n::stock_mode_label(mode)).color(color);
                                ui.add_enabled_ui(available, |ui| {
                                    if ui
                                        .radio_value(&mut selected_mode, mode, label)
                                        .changed()
                                    {
                                        self.stock_mode = selected_mode;
                                        self.start_inspect_stock_rom(selected_mode);
                                    }
                                });
                            }
                        });

                        let mut compat_ok = true;
                        ui.group(|ui| {
                            let path_s = info.script_path.display().to_string();
                            let steps_s = info.flash_command_count.to_string();
                            let size_s = format_bytes_human(info.total_bytes);
                            ui.label(
                                egui::RichText::new(i18n::tf(
                                    "flash.stock_script_info",
                                    &[("path", &path_s), ("steps", &steps_s), ("size", &size_s)],
                                ))
                                .color(egui::Color32::GREEN),
                            );
                            if let Some(expected) = &info.expected_product {
                                let dev_prod = dev_ok.as_ref().map(|d| d.product.as_str());
                                if dev_prod == Some(expected.as_str()) {
                                    ui.colored_label(
                                        egui::Color32::GREEN,
                                        i18n::tf(
                                            "flash.stock_prod_match",
                                            &[("expected", expected)],
                                        ),
                                    );
                                } else if let Some(actual) = dev_prod {
                                    compat_ok = false;
                                    ui.colored_label(
                                        egui::Color32::LIGHT_RED,
                                        i18n::tf(
                                            "flash.stock_prod_mismatch",
                                            &[("actual", actual), ("expected", expected)],
                                        ),
                                    );
                                } else {
                                    ui.label(i18n::tf(
                                        "flash.stock_prod_expected",
                                        &[("expected", expected)],
                                    ));
                                }
                            }
                            if let Some(rom_anti) = info.rom_anti_version {
                                let dev_anti = dev_ok
                                    .as_ref()
                                    .and_then(|d| d.anti_rollback.as_deref())
                                    .and_then(|s| s.trim().parse::<u32>().ok());
                                if let Some(da) = dev_anti {
                                    let d_s = da.to_string();
                                    let r_s = rom_anti.to_string();
                                    if da > rom_anti {
                                        compat_ok = false;
                                        ui.colored_label(
                                            egui::Color32::LIGHT_RED,
                                            i18n::tf(
                                                "flash.stock_arb_block",
                                                &[("dev", &d_s), ("rom", &r_s)],
                                            ),
                                        );
                                    } else {
                                        ui.colored_label(
                                            egui::Color32::GREEN,
                                            i18n::tf(
                                                "flash.stock_arb_pass",
                                                &[("dev", &d_s), ("rom", &r_s)],
                                            ),
                                        );
                                    }
                                } else {
                                    let r_s = rom_anti.to_string();
                                    ui.label(i18n::tf("flash.stock_arb_rom", &[("rom", &r_s)]));
                                }
                            }

                            let steps_cnt = info.steps.len().to_string();
                            egui::CollapsingHeader::new(i18n::tf(
                                "flash.stock_steps_header",
                                &[("count", &steps_cnt)],
                            ))
                            .id_salt("stock_parsed_steps_list")
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("stock_steps_scroll")
                                    .max_height(150.0)
                                    .show(ui, |ui| {
                                        for (idx, step) in info.steps.iter().enumerate() {
                                            let cond = if step.conditional {
                                                i18n::t("flash.stock_cond")
                                            } else {
                                                ""
                                            };
                                            match &step.kind {
                                                StockStepKind::Flash {
                                                    partition,
                                                    image_rel,
                                                    bytes,
                                                } => {
                                                    ui.small(format!(
                                                        "{}. flash {} ← {} ({}){}",
                                                        idx + 1,
                                                        partition,
                                                        image_rel,
                                                        format_bytes_human(*bytes),
                                                        cond
                                                    ));
                                                }
                                                _ => {
                                                    ui.small(format!(
                                                        "{}. {}{}",
                                                        idx + 1,
                                                        step.label,
                                                        cond
                                                    ));
                                                }
                                            }
                                        }
                                    });
                            });
                        });

                        let confirm_caption = match info.mode {
                            StockFlashMode::CleanAllAndLock => i18n::t("flash.stock_confirm_lock"),
                            StockFlashMode::CleanAll => i18n::t("flash.stock_confirm_clean"),
                            StockFlashMode::KeepUserData => i18n::t("flash.stock_confirm_keep"),
                        };
                        ui.checkbox(&mut self.confirm_stock_lock, confirm_caption);

                        let required_word = match info.mode {
                            StockFlashMode::CleanAllAndLock => Some("LOCK"),
                            StockFlashMode::CleanAll => Some("FLASH"),
                            StockFlashMode::KeepUserData => None,
                        };
                        let typed_ok = if let Some(word) = required_word {
                            let mut ok = false;
                            ui.horizontal(|ui| {
                                ui.label(i18n::tf("flash.stock_type_confirm", &[("word", word)]));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.lock_confirm_text)
                                        .desired_width(120.0),
                                );
                                ok = self.lock_confirm_text.trim().eq_ignore_ascii_case(word);
                            });
                            ok
                        } else {
                            true
                        };

                        let button_text = match info.mode {
                            StockFlashMode::CleanAllAndLock => i18n::t("flash.stock_btn_lock"),
                            StockFlashMode::CleanAll => i18n::t("flash.stock_btn_clean"),
                            StockFlashMode::KeepUserData => i18n::t("flash.stock_btn_keep"),
                        };

                        let can_run = is_bootloader_unlocked
                            && compat_ok
                            && self.confirm_stock_lock
                            && typed_ok;
                        let btn_color = if info.mode.locks_bootloader() {
                            colors::ACCENT_RED
                        } else if info.mode.wipes_userdata() {
                            colors::ACCENT_AMBER
                        } else {
                            colors::ACCENT_GREEN
                        };
                        if ui
                            .add_enabled_ui(can_run, |ui| {
                                ui_elements::accent_button(ui, button_text, btn_color)
                            })
                            .inner
                            .clicked()
                        {
                            self.start_stock_flash_and_lock(info);
                        }
                    }
                    Err(e) => {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            i18n::tf("flash.single_inspect_err", &[("err", &e)]),
                        );
                    }
                }
            }
        });
    }

    fn render_lock_bootloader_tab(&mut self, ui: &mut egui::Ui) {
        ui_elements::tool_subcard(ui, Some(colors::ACCENT_RED), |ui| {
            ui.label(
                egui::RichText::new(i18n::t("flash.lock_warn_title"))
                    .color(colors::ERROR)
                    .strong(),
            );
            ui.label(i18n::t("flash.lock_warn_1"));
            ui.label(i18n::t("flash.lock_warn_2"));
            ui.label(i18n::t("flash.lock_warn_3"));
            ui.label(i18n::t("flash.lock_warn_4"));
        });

        ui.add_space(6.0);

        self.render_stock_rom_selector_and_run(ui);

        ui.add_space(6.0);

        let dev_ok = self.device_info.as_ref().and_then(|r| r.as_ref().ok());
        let is_bootloader_unlocked = dev_ok.is_some_and(|d| d.mode == "bootloader" && d.unlocked);

        // Mode B: Direct Fastboot Lock (only if 100% stock is already installed)
        ui.collapsing(i18n::t("flash.lock_opt2"), |ui| {
            ui.small(i18n::t("flash.lock_opt2_note"));
            ui.horizontal_wrapped(|ui| {
                ui.label(i18n::t("flash.lock_method"));
                ui.radio_value(
                    &mut self.direct_lock_method,
                    LockMethod::OemLock,
                    i18n::t("flash.lock_oem"),
                );
                ui.radio_value(
                    &mut self.direct_lock_method,
                    LockMethod::FlashingLock,
                    i18n::t("flash.lock_flashing"),
                );
            });
            ui.checkbox(
                &mut self.confirm_direct_lock,
                i18n::t("flash.lock_direct_confirm"),
            );
            ui.horizontal(|ui| {
                ui.label(i18n::tf("flash.stock_type_confirm", &[("word", "LOCK")]));
                ui.add(
                    egui::TextEdit::singleline(&mut self.direct_lock_text)
                        .desired_width(120.0),
                );
            });
            let typed_ok = self
                .direct_lock_text
                .trim()
                .eq_ignore_ascii_case("LOCK");
            let can_direct =
                is_bootloader_unlocked && self.confirm_direct_lock && typed_ok;
            if ui
                .add_enabled_ui(can_direct, |ui| {
                    ui_elements::accent_button(
                        ui,
                        &i18n::tf(
                            "flash.lock_run_cmd",
                            &[("cmd", self.direct_lock_method.display_cmd())],
                        ),
                        colors::ACCENT_RED,
                    )
                })
                .inner
                .clicked()
            {
                self.start_direct_lock();
            }
        });
    }

    fn render_guide_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading(i18n::t("flash.guide_title"));
        ui.add_space(4.0);

        // Quick jump buttons
        ui.horizontal_wrapped(|ui| {
            ui.label(i18n::t("flash.guide_jump"));
            if ui.small_button(i18n::t("flash.tab_full")).clicked() {
                self.tab = FlashTab::FullRom;
            }
            if ui.small_button(i18n::t("flash.tab_single")).clicked() {
                self.tab = FlashTab::SinglePartition;
            }
            if ui.small_button(i18n::t("flash.tab_sideload")).clicked() {
                self.tab = FlashTab::AdbSideload;
            }
            if ui.small_button(i18n::t("flash.tab_gen")).clicked() {
                self.tab = FlashTab::ManifestGenerator;
            }
            if ui.small_button(i18n::t("flash.tab_stock_lock")).clicked() {
                self.tab = FlashTab::LockBootloader;
            }
            if ui.small_button(i18n::t("flash.guide_copy_sample")).clicked() {
                ctx.copy_text(SAMPLE_MANIFEST_JSON.to_string());
                self.status = i18n::t("flash.guide_copied").into();
                self.error = false;
            }
        });

        ui.add_space(6.0);

        // Safety checklist
        ui.group(|ui| {
            ui.label(
                egui::RichText::new(i18n::t("flash.guide_checklist_title"))
                    .color(egui::Color32::YELLOW)
                    .strong(),
            );
            ui.label("• Pin điện thoại nên >= 50% (xem dòng Pin ổn định ở thanh thông tin thiết bị).");
            ui.label("• Sao lưu dữ liệu quan trọng trước (có thể dùng nút Sao lưu / Truyền tệp… trên Dashboard).");
            ui.label("• Bootloader bắt buộc ở trạng thái Đã mở khóa. Công cụ KHÔNG tự unlock bootloader và KHÔNG chạy lệnh xóa phân vùng dữ liệu ngoài ý muốn.");
            ui.label("• Dùng cáp USB tốt, cắm trực tiếp vào cổng USB máy tính; tuyệt đối không rút cáp khi đang ghi.");
            ui.label("• Mọi thao tác flash/sideload đều được ghi nhật ký — bấm 'Mở thư mục Log' để xem lại.");
        });

        ui.add_space(4.0);

        egui::CollapsingHeader::new("1. ROM đầy đủ (Manifest) — Flash trọn bộ ROM từ manifest.json")
            .default_open(true)
            .show(ui, |ui| {
                ui.label("1. Đưa máy vào chế độ Bootloader hoặc Fastbootd (dùng mục 'Khởi động lại, Slot A/B & Đường dẫn công cụ' hoặc phím cứng) rồi bấm 'Làm mới thiết bị'.");
                ui.label("2. Nếu thư mục ROM chỉ có các file .img và chưa có manifest.json, sang tab 'Tạo Manifest' để tạo trước.");
                ui.label("3. Chọn (Duyệt…) hoặc kéo-thả file manifest.json vào cửa sổ.");
                ui.label("4. Bấm 'Kiểm tra & Lập kế hoạch': tool sẽ đối chiếu mã máy, chế độ, dung lượng phân vùng và toàn bộ mã SHA-256.");
                ui.label("5. Nếu flash Custom ROM/GSI có vbmeta, bật thêm '--disable-verity' và '--disable-verification' nếu cần.");
                ui.label("6. Tick xác nhận và bấm 'Bắt đầu Flash ROM đầy đủ'. Khi hoàn tất, máy giữ nguyên ở Fastboot để bạn kiểm tra log rồi tự khởi động lại.");
            });

        egui::CollapsingHeader::new("2. Phân vùng lẻ / Boot tạm — Flash lẻ 1 phân vùng hoặc Boot tạm (RAM)")
            .show(ui, |ui| {
                ui.label("• Phù hợp khi cài Magisk (boot/init_boot), Custom Recovery (recovery/vendor_boot), dtbo, vbmeta hoặc super.");
                ui.label("• Trên máy A/B, chọn rõ phân vùng có hậu tố _a hoặc _b (ví dụ boot_a, init_boot_b). Tool tự gợi ý theo slot đang chạy.");
                ui.label("• Bấm 'Kiểm tra ảnh & Tính SHA-256' để kiểm tra dung lượng, định dạng Raw/Sparse và mã SHA-256.");
                ui.label("• Nút 'Khởi động tạm (fastboot boot)': chạy thử boot.img hoặc recovery.img trực tiếp trên RAM mà KHÔNG ghi đè flash (rất an toàn để test).");
            });

        egui::CollapsingHeader::new("3. ADB Sideload (ZIP) — Cài Custom ROM / OTA qua Recovery")
            .show(ui, |ui| {
                ui.label("1. Khởi động máy vào Recovery (TWRP, OrangeFox, Lineage Recovery hoặc Stock Recovery hỗ trợ sideload).");
                ui.label("2. Trên điện thoại, chọn 'Apply update' → 'Apply from ADB' (hoặc 'ADB Sideload').");
                ui.label("3. Bấm 'Làm mới thiết bị' — thiết bị sẽ hiện ở tab ADB Sideload với trạng thái '(sideload)'.");
                ui.label("4. Chọn hoặc kéo-thả file .zip, có thể bấm 'Kiểm tra ZIP & Tính SHA-256' để đối chiếu checksum.");
                ui.label("5. Tick xác nhận và bấm 'Bắt đầu ADB Sideload'. Theo dõi phần trăm tiến độ trực tiếp trên thanh tiến trình.");
            });

        egui::CollapsingHeader::new("4. Tạo Manifest — Tự tạo manifest.json từ thư mục .img")
            .show(ui, |ui| {
                ui.label("• Chọn hoặc kéo-thả thư mục chứa các file .img (tên file trùng tên phân vùng, ví dụ boot.img, dtbo.img, vbmeta.img, super.img).");
                ui.label("• Nhập Mã máy (hoặc bấm nút 'Lấy từ máy ...' đang cắm), nhập Mã bản dựng và chọn Chế độ Fastboot.");
                ui.label("• Với máy A/B, chọn 'Hậu tố Slot A/B' là 'Slot _a' hoặc 'Slot _b' (tool tự giữ nguyên tên 'super' và các file đã có _a/_b).");
                ui.label("• Bấm 'Tạo manifest.json từ thư mục' → sau khi xong, bấm 'Chuyển sang tab ROM đầy đủ để Flash'.");
            });

        egui::CollapsingHeader::new("5. ROM Gốc & Khóa Bootloader (flash_all_lock / flash_all / flash_all_except_storage)")
            .show(ui, |ui| {
                ui.label("• Chọn hoặc kéo-thả thư mục ROM Fastboot Gốc (chứa flash_all_lock.bat / flash_all.bat / flash_all_except_storage.bat và thư mục images/).");
                ui.label("• 3 chế độ: Xóa sạch & Khóa Bootloader, Xóa sạch giữ Mở khóa, và Cập nhật giữ dữ liệu người dùng.");
                ui.label("• Trước khi chạy, công cụ kiểm tra Mã máy, Anti-Rollback, trạng thái Đã mở khóa, chế độ bootloader và pin; trong lúc chạy sẽ hiển thị chunk sparse (vd super 3/11), phân vùng đang ghi, danh sách còn lại và thời gian dự kiến.");
                ui.label("• LƯU Ý KHÓA BOOTLOADER: Thoát Mi Account (tắt Tìm thiết bị) & Google Account, dùng đúng ROM Gốc cùng vùng máy (VD: máy nội địa China dùng ROM Gốc China), cắm cổng USB 2.0 ổn định trên PC AMD.");
                ui.label("• Tùy chọn 2 (Nâng cao - Lệnh khóa trực tiếp): chỉ dùng khi máy đã chạy 100% ROM Gốc nguyên bản đúng vùng.");
            });

        egui::CollapsingHeader::new("6. Phân biệt Bootloader vs Fastbootd & Quản lý Slot A/B")
            .show(ui, |ui| {
                ui.label("• Chế độ bootloader: Flash phân vùng phần cứng / khởi động — boot, init_boot, vendor_boot, vendor_kernel_boot, dtbo, recovery, vbmeta*, super.");
                ui.label("• Chế độ fastbootd (userspace): Flash phân vùng động nằm trong super — system, system_ext, vendor, product, odm, *_dlkm.");
                ui.label("• Các phân vùng nguy hiểm (userdata, metadata, persist, abl, xbl...) luôn bị chặn để tránh hard-brick / mất dữ liệu.");
                ui.label("• Slot A/B đang kích hoạt: Chỉ chuyển giữa Slot A và Slot B khi chắc chắn slot kia có bản ROM khởi động được.");
            });

        egui::CollapsingHeader::new("7. Xử lý sự cố thường gặp")
            .show(ui, |ui| {
                ui.label("• Không thấy máy ở Fastboot trên Windows: Cài Google USB Driver / driver của hãng (Xiaomi, Pixel, Motorola…) và kiểm tra Device Manager (Android Bootloader Interface).");
                ui.label("• Lỗi 'Cannot start fastboot': Mở mục 'Khởi động lại, Slot A/B & Đường dẫn công cụ' và trỏ trực tiếp tới fastboot.exe trong Android SDK platform-tools.");
                ui.label("• Lỗi 'Choose an explicit _a or _b partition': Thiết bị dùng A/B slot — hãy chọn boot_a/boot_b thay vì boot, hoặc chọn Slot _a/_b khi tạo manifest.");
                ui.label("• Lỗi 'Device product or mode does not match': Kiểm tra đúng ROM cho mã máy và đúng chế độ bootloader/fastbootd.");
                ui.label("• Lỗi giữa chừng khi đang flash: KHÔNG tự reboot ngay! Xem Nhật ký lệnh / Nhật ký ghi, kiểm tra cáp USB và flash lại phân vùng bị lỗi trước khi khởi động lại.");
            });
    }

    // ---- Background task starters ----

    fn start_prepare_plan(&mut self) {
        let Some(fb) = self.selected_fastboot.clone() else {
            return;
        };
        let path = PathBuf::from(self.manifest_path.trim());
        let fb_bin = self.fastboot_override();
        let tx = self.tx.clone();
        self.pending_bg_tasks += 1;
        self.progress_percent = Some(0.0);
        self.status = i18n::t("flash.status_verifying_plan").into();
        self.error = false;
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let res = flash::prepare_with_progress(fb_bin.as_deref(), &fb, &path, move |upd| {
                let _ = tx_prog.send(FlashEvent::Progress(upd));
            });
            let _ = tx.send(FlashEvent::PlanPrepared(res));
        });
    }

    fn start_inspect_single(&mut self) {
        let path = PathBuf::from(self.single_image_path.trim());
        let tx = self.tx.clone();
        self.pending_bg_tasks += 1;
        self.status = i18n::t("flash.status_inspecting_img").into();
        self.error = false;
        std::thread::spawn(move || {
            let res = if !path.is_file() {
                Err(i18n::t("flash.err_not_regular_file").into())
            } else {
                fs::metadata(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|meta| {
                        let sparse = flash::is_android_sparse_image(&path)?;
                        let digest = flash::hash(&path)?;
                        Ok((meta.len(), digest, sparse))
                    })
            };
            let _ = tx.send(FlashEvent::SingleInspected(res));
        });
    }

    fn start_inspect_sideload(&mut self) {
        let path = PathBuf::from(self.sideload_zip_path.trim());
        let tx = self.tx.clone();
        self.pending_bg_tasks += 1;
        self.status = i18n::t("flash.status_inspecting_zip").into();
        self.error = false;
        std::thread::spawn(move || {
            let res = if !path.is_file() {
                Err(i18n::t("flash.err_not_regular_file").into())
            } else {
                fs::metadata(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|meta| {
                        let digest = flash::hash(&path)?;
                        Ok((meta.len(), digest))
                    })
            };
            let _ = tx.send(FlashEvent::SideloadInspected(res));
        });
    }

    fn start_generate_manifest(&mut self) {
        let folder = PathBuf::from(self.gen_folder.trim());
        let product = self.gen_product.clone();
        let build = self.gen_build.clone();
        let mode = self.gen_mode.clone();
        let slot = self.gen_slot.as_suffix();
        let tx = self.tx.clone();
        self.pending_bg_tasks += 1;
        self.progress_percent = Some(0.0);
        self.status = i18n::t("flash.status_scanning_folder").into();
        self.error = false;
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let res = flash::generate_manifest_with_options(
                &folder,
                &product,
                &build,
                &mode,
                slot,
                move |upd| {
                    let _ = tx_prog.send(FlashEvent::Progress(upd));
                },
            );
            let _ = tx.send(FlashEvent::ManifestGenerated(res));
        });
    }

    fn run_adb_reboot(&mut self, serial: String, target: Option<&'static str>) {
        self.pending_bg_tasks += 1;
        let t_str = target.unwrap_or("system");
        self.status = i18n::tf(
            "flash.status_adb_rebooting",
            &[("serial", &serial), ("target", t_str)],
        );
        self.error = false;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = Adb::reboot(Some(&serial), target)
                .map(|_| {
                    i18n::tf(
                        "flash.status_adb_reboot_ok",
                        &[("target", t_str), ("serial", &serial)],
                    )
                })
                .map_err(|e| {
                    let err_s = i18n::adb_error_vi(&e.user_message());
                    i18n::tf("flash.status_adb_reboot_fail", &[("err", &err_s)])
                });
            let _ = tx.send(FlashEvent::QuickActionDone(res));
        });
    }

    fn run_fastboot_reboot(&mut self, serial: String, target: &'static str) {
        self.pending_bg_tasks += 1;
        let t_str = if target.is_empty() { "system" } else { target };
        self.status = i18n::tf(
            "flash.status_fb_rebooting",
            &[("serial", &serial), ("target", t_str)],
        );
        self.error = false;
        let fb_bin = self.fastboot_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = flash::reboot_with_bin(fb_bin.as_deref(), Some(&serial), target)
                .map(|t| i18n::tf("flash.status_fb_reboot_ok", &[("msg", t.trim())]))
                .map_err(|e| i18n::tf("flash.status_fb_reboot_fail", &[("err", &e)]));
            let _ = tx.send(FlashEvent::QuickActionDone(res));
        });
    }

    fn run_set_active_slot(&mut self, serial: String, slot: &'static str) {
        self.pending_bg_tasks += 1;
        self.status = i18n::tf(
            "flash.status_setting_slot",
            &[("slot", slot), ("serial", &serial)],
        );
        self.error = false;
        let fb_bin = self.fastboot_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = flash::set_active_slot(fb_bin.as_deref(), &serial, slot)
                .map(|t| i18n::tf("flash.status_slot_ok", &[("slot", slot), ("msg", t.trim())]))
                .map_err(|e| i18n::tf("flash.status_slot_fail", &[("err", &e)]));
            let _ = tx.send(FlashEvent::QuickActionDone(res));
        });
    }

    fn start_full_flash(&mut self, plan: Plan) {
        let serial = plan.device().serial.clone();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let log_dir = reports_dir();
        let _ = fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("flash-full-{serial}-{stamp}.log"));
        self.log_path = Some(log_path.clone());

        self.is_flashing = true;
        self.started = Some(Instant::now());
        self.progress_percent = Some(0.0);
        self.stock_progress = None;
        self.error = false;
        self.output = format!(
            "Bắt đầu flash trọn bộ ROM trên {serial}…\nLog: {}\n\n",
            log_path.display()
        );

        let fb_bin = self.fastboot_override();
        let options = self.full_options.clone();
        let audit_serial = serial.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let result = flash::execute_with_options(
                fb_bin.as_deref(),
                plan,
                &options,
                &log_path,
                move |upd| {
                    let _ = tx_prog.send(FlashEvent::Progress(upd));
                },
            );
            let (ok, msg) = match &result {
                Ok(()) => (true, String::new()),
                Err(e) => (false, e.clone()),
            };
            audit::record(
                &audit_serial,
                None,
                "flash.full_rom",
                "",
                &log_path.display().to_string(),
                ok,
                &msg,
            );
            let _ = tx.send(FlashEvent::WriteFinished(
                result.map(|()| i18n::t("flash.done_full").into()),
            ));
        });
    }

    fn start_single_flash(&mut self) {
        let Some(serial) = self.selected_fastboot.clone() else {
            return;
        };
        let partition = self.partition.trim().to_string();
        let path = PathBuf::from(self.single_image_path.trim());
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let log_dir = reports_dir();
        let _ = fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("flash-single-{serial}-{partition}-{stamp}.log"));
        self.log_path = Some(log_path.clone());

        self.is_flashing = true;
        self.started = Some(Instant::now());
        self.progress_percent = Some(0.0);
        self.stock_progress = None;
        self.error = false;
        self.output = format!(
            "Bắt đầu flash phân vùng [{partition}] trên {serial}…\nTệp: {}\nLog: {}\n\n",
            path.display(),
            log_path.display()
        );

        let fb_bin = self.fastboot_override();
        let options = self.single_options.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let result = flash::flash_single_image_with_options(
                fb_bin.as_deref(),
                &serial,
                &partition,
                &path,
                &options,
                &log_path,
                move |upd| {
                    let _ = tx_prog.send(FlashEvent::Progress(upd));
                },
            );
            let (ok, msg) = match &result {
                Ok(()) => (true, String::new()),
                Err(e) => (false, e.clone()),
            };
            audit::record(
                &serial,
                None,
                "flash.single_partition",
                &partition,
                &format!("image={}; log={}", path.display(), log_path.display()),
                ok,
                &msg,
            );
            let _ = tx.send(FlashEvent::WriteFinished(
                result.map(|()| i18n::tf("flash.done_single", &[("part", &partition)])),
            ));
        });
    }

    fn start_temp_boot(&mut self) {
        let Some(serial) = self.selected_fastboot.clone() else {
            return;
        };
        let path = PathBuf::from(self.single_image_path.trim());
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let log_dir = reports_dir();
        let _ = fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("fastboot-boot-{serial}-{stamp}.log"));
        self.log_path = Some(log_path.clone());

        self.is_flashing = true;
        self.started = Some(Instant::now());
        self.progress_percent = Some(0.0);
        self.stock_progress = None;
        self.error = false;
        self.output = format!(
            "Bắt đầu khởi động tạm (`fastboot boot`) trên {serial}…\nTệp: {}\nLog: {}\n\n",
            path.display(),
            log_path.display()
        );

        let fb_bin = self.fastboot_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let result =
                flash::boot_image(fb_bin.as_deref(), &serial, &path, &log_path, move |upd| {
                    let _ = tx_prog.send(FlashEvent::Progress(upd));
                });
            let (ok, msg) = match &result {
                Ok(()) => (true, String::new()),
                Err(e) => (false, e.clone()),
            };
            audit::record(
                &serial,
                None,
                "flash.temp_boot",
                &path.display().to_string(),
                &log_path.display().to_string(),
                ok,
                &msg,
            );
            let _ = tx.send(FlashEvent::WriteFinished(
                result.map(|()| i18n::t("flash.done_temp_boot").into()),
            ));
        });
    }

    fn start_adb_sideload(&mut self) {
        let Some(serial) = self.selected_sideload.clone() else {
            return;
        };
        let path = PathBuf::from(self.sideload_zip_path.trim());
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let log_dir = reports_dir();
        let _ = fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("adb-sideload-{serial}-{stamp}.log"));
        self.log_path = Some(log_path.clone());

        self.is_flashing = true;
        self.started = Some(Instant::now());
        self.progress_percent = Some(0.0);
        self.stock_progress = None;
        self.error = false;
        self.output = format!(
            "Bắt đầu ADB Sideload trên {serial}…\nTệp: {}\nLog: {}\n\n",
            path.display(),
            log_path.display()
        );

        let adb_bin = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let result =
                flash::adb_sideload(adb_bin.as_deref(), &serial, &path, &log_path, move |upd| {
                    let _ = tx_prog.send(FlashEvent::Progress(upd));
                });
            let (ok, msg) = match &result {
                Ok(()) => (true, String::new()),
                Err(e) => (false, e.clone()),
            };
            audit::record(
                &serial,
                None,
                "flash.adb_sideload",
                &path.display().to_string(),
                &log_path.display().to_string(),
                ok,
                &msg,
            );
            let _ = tx.send(FlashEvent::WriteFinished(
                result.map(|()| i18n::t("flash.done_sideload").into()),
            ));
        });
    }

    fn start_inspect_stock_lock(&mut self) {
        let folder = PathBuf::from(self.lock_rom_folder.trim());
        let tx = self.tx.clone();
        self.pending_bg_tasks += 1;
        self.status = i18n::t("flash.status_inspecting_stock").into();
        self.error = false;
        std::thread::spawn(move || {
            let res = flash::inspect_stock_rom_folder(&folder);
            let _ = tx.send(FlashEvent::StockLockInspected(res));
        });
    }

    fn start_inspect_stock_rom(&mut self, mode: StockFlashMode) {
        let folder = PathBuf::from(self.lock_rom_folder.trim());
        let tx = self.tx.clone();
        self.pending_bg_tasks += 1;
        self.status = i18n::tf(
            "flash.status_inspecting_script",
            &[("script", mode.script_name())],
        );
        self.error = false;
        std::thread::spawn(move || {
            let res = flash::inspect_stock_rom_script(&folder, mode);
            let _ = tx.send(FlashEvent::StockLockInspected(res));
        });
    }

    fn start_stock_flash_and_lock(&mut self, info: StockLockInfo) {
        let Some(serial) = self.selected_fastboot.clone() else {
            return;
        };
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let log_dir = reports_dir();
        let _ = fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("flash-stock-{serial}-{stamp}.log"));
        self.log_path = Some(log_path.clone());

        self.is_flashing = true;
        self.started = Some(Instant::now());
        self.progress_percent = Some(0.0);
        self.stock_progress = None;
        self.error = false;
        self.output = format!(
            "Bắt đầu {} trên {serial}…\nKịch bản: {}\nLog: {}\n\n",
            i18n::stock_mode_label(info.mode),
            info.script_path.display(),
            log_path.display()
        );

        let fb_bin = self.fastboot_override();
        let locks_bl = info.mode.locks_bootloader();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let result = flash::run_stock_flash_script(
                fb_bin.as_deref(),
                &serial,
                &info,
                &log_path,
                move |upd| {
                    let _ = tx_prog.send(FlashEvent::Progress(upd));
                },
            );
            let (ok, msg) = match &result {
                Ok(()) => (true, String::new()),
                Err(e) => (false, e.clone()),
            };
            audit::record(
                &serial,
                None,
                if locks_bl { "flash.stock_lock" } else { "flash.stock" },
                &info.script_path.display().to_string(),
                &log_path.display().to_string(),
                ok,
                &msg,
            );
            let done_msg = if locks_bl {
                i18n::t("flash.done_stock_locked").into()
            } else {
                i18n::t("flash.done_stock_unlocked").into()
            };
            let _ = tx.send(FlashEvent::WriteFinished(result.map(|()| done_msg)));
        });
    }

    fn start_direct_lock(&mut self) {
        let Some(serial) = self.selected_fastboot.clone() else {
            return;
        };
        let method = self.direct_lock_method;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let log_dir = reports_dir();
        let _ = fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("lock-bootloader-{serial}-{stamp}.log"));
        self.log_path = Some(log_path.clone());

        self.is_flashing = true;
        self.started = Some(Instant::now());
        self.progress_percent = Some(0.0);
        self.stock_progress = None;
        self.error = false;
        self.output = format!(
            "Bắt đầu khóa Bootloader trực tiếp (`{}`) trên {serial}…\nLog: {}\n\n",
            method.display_cmd(),
            log_path.display()
        );

        let fb_bin = self.fastboot_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let result =
                flash::lock_bootloader(fb_bin.as_deref(), &serial, method, &log_path, move |upd| {
                    let _ = tx_prog.send(FlashEvent::Progress(upd));
                });
            let (ok, msg) = match &result {
                Ok(()) => (true, String::new()),
                Err(e) => (false, e.clone()),
            };
            audit::record(
                &serial,
                None,
                "flash.lock_bootloader",
                &method.display_cmd(),
                &log_path.display().to_string(),
                ok,
                &msg,
            );
            let _ = tx.send(FlashEvent::WriteFinished(result.map(|()| {
                i18n::tf("flash.done_direct_lock", &[("cmd", method.display_cmd())])
            })));
        });
    }

    fn pick_path(&mut self, target: PickTarget) {
        self.pending_bg_tasks += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let path = native_pick_path(target);
            let _ = tx.send(FlashEvent::PickedPath { target, path });
        });
    }
}

#[cfg(windows)]
fn native_pick_path(target: PickTarget) -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let script = match target {
        PickTarget::GeneratorFolder => format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = '{}'; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.SelectedPath) }}",
            i18n::t("flash.pick_gen_folder")
        ),
        PickTarget::StockLockFolder => format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = '{}'; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.SelectedPath) }}",
            i18n::t("flash.pick_stock_folder")
        ),
        PickTarget::ManifestJson => format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Filter = '{}'; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.FileName) }}",
            i18n::t("flash.pick_manifest")
        ),
        PickTarget::SingleImage => format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Filter = '{}'; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.FileName) }}",
            i18n::t("flash.pick_img")
        ),
        PickTarget::SideloadZip => format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Filter = '{}'; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.FileName) }}",
            i18n::t("flash.pick_zip")
        ),
    };

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
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(not(windows))]
fn native_pick_path(_target: PickTarget) -> Option<String> {
    None
}

fn open_folder_in_os(dir: &PathBuf) {
    let _ = fs::create_dir_all(dir);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

fn format_bytes_human(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_duration_short(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}
