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
enum GuideLang {
    Vi,
    En,
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
    // Tab 6: Guide
    guide_lang: GuideLang,
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
            guide_lang: GuideLang::Vi,
            tx,
            rx,
            is_flashing: false,
            pending_bg_tasks: 0,
            started: None,
            progress_percent: None,
            progress_label: String::new(),
            stock_progress: None,
            status: "Ready. Connect a device in Fastboot / Fastbootd or Recovery Sideload mode."
                .into(),
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
        self.status = "Scanning Fastboot & Recovery devices…".into();
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
                                self.status = format!(
                                    "Found {} Fastboot device(s), {} Recovery/Sideload device(s).",
                                    self.fastboot_devices.len(),
                                    self.sideload_devices.len()
                                );
                                self.error = false;
                            }
                        }
                        Err(e) => {
                            self.fastboot_devices.clear();
                            self.selected_fastboot = None;
                            self.device_info = Some(Err(e.clone()));
                            self.status = format!("Fastboot scan failed: {e}");
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
                            self.status = format!(
                                "Verified plan for {} ({} images, {:.2} MB ready)",
                                plan.manifest().product,
                                plan.manifest().images.len(),
                                plan.total_bytes() as f64 / (1024.0 * 1024.0)
                            );
                            self.error = false;
                            self.plan_verified = Some(plan);
                            self.confirm_full_flash = false;
                        }
                        Err(e) => {
                            self.status = format!("Plan verification failed: {e}");
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
                            self.status = format!(
                                "Generated manifest with {} images: {}",
                                manifest.images.len(),
                                path.display()
                            );
                            self.error = false;
                            self.gen_result = Some(Ok((manifest, path)));
                        }
                        Err(e) => {
                            self.status = format!("Generation error: {e}");
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
                            self.status = format!(
                                "Detected {} ({} flash steps, {}, product: {})",
                                info.mode.label(),
                                info.flash_command_count,
                                format_bytes_human(info.total_bytes),
                                info.expected_product.as_deref().unwrap_or("unspecified")
                            );
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = format!("Stock ROM inspection failed: {e}");
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
                            self.output.push_str(&format!("ERROR: {err}\n"));
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
                            self.status = format!("COMPLETED: {msg}");
                            self.error = false;
                            self.output.push_str(&format!("\n=== SUCCESS: {msg} ===\n"));
                        }
                        Err(err) => {
                            self.progress_percent = None;
                            self.status = format!("FAILED: {err}");
                            self.error = true;
                            self.output.push_str(&format!("\n=== ERROR: {err} ===\n"));
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
        egui::Window::new("Flash ROM & Fastboot Tools")
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
            self.status =
                "Flashing/Sideload in progress — wait for completion before closing this window."
                    .into();
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
                    self.status = "Loaded dropped ROM folder into Manifest Generator.".into();
                    self.error = false;
                }
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                match ext.to_ascii_lowercase().as_str() {
                    "json" => {
                        self.manifest_path = display;
                        self.invalidate_full_plan();
                        self.tab = FlashTab::FullRom;
                        self.status = "Loaded dropped manifest.json.".into();
                        self.error = false;
                    }
                    "img" => {
                        self.single_image_path = display;
                        self.invalidate_single(true);
                        self.tab = FlashTab::SinglePartition;
                        self.status = "Loaded dropped .img file.".into();
                        self.error = false;
                    }
                    "zip" => {
                        self.sideload_zip_path = display;
                        self.invalidate_sideload(true);
                        self.tab = FlashTab::AdbSideload;
                        self.status = "Loaded dropped .zip package into ADB Sideload.".into();
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
                .add_enabled(!is_busy, egui::Button::new("Refresh Fastboot"))
                .clicked()
            {
                self.refresh_fastboot_devices();
            }
            if let Some(adb) = self.selected_adb.clone() {
                if ui
                    .add_enabled(!is_busy, egui::Button::new("Reboot ADB → Bootloader"))
                    .clicked()
                {
                    self.run_adb_reboot(adb, Some("bootloader"));
                }
            }
            if ui.button("Open Full Flash Window…").clicked() {
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
            ui.small("Other Flash Tools:");
            if ui.small_button("Full ROM Manifest").clicked() {
                self.open_tab(FlashTab::FullRom);
            }
            if ui.small_button("Single Partition / Boot").clicked() {
                self.open_tab(FlashTab::SinglePartition);
            }
            if ui.small_button("ADB Sideload (ZIP)").clicked() {
                self.open_tab(FlashTab::AdbSideload);
            }
            if ui.small_button("Guide / Hướng dẫn").clicked() {
                self.open_tab(FlashTab::Guide);
            }
        });
    }

    fn render_fastboot_connection_bar(&mut self, ui: &mut egui::Ui, is_busy: bool) {
        ui.horizontal_wrapped(|ui| {
            if self.fastboot_devices.is_empty() {
                ui.label(
                    egui::RichText::new("No Fastboot devices detected.")
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
                                &format!("Product: {}", dev.product),
                                colors::ACCENT_BLUE,
                            );
                            ui_elements::status_pill(
                                ui,
                                &format!("Mode: {}", dev.mode),
                                colors::TEXT_MUTED,
                            );
                            ui_elements::status_pill(
                                ui,
                                &format!(
                                    "Slot: {}",
                                    if dev.slot.is_empty() { "N/A" } else { &dev.slot }
                                ),
                                colors::TEXT_MUTED,
                            );
                            if dev.unlocked {
                                ui_elements::status_pill(
                                    ui,
                                    "Bootloader: Unlocked",
                                    colors::ACCENT_GREEN,
                                );
                            } else {
                                ui_elements::status_pill(
                                    ui,
                                    "Bootloader: LOCKED (flashing blocked)",
                                    colors::ACCENT_RED,
                                );
                            }
                        });
                        let mut extras = Vec::new();
                        if let Some(v) = &dev.battery_voltage {
                            extras.push(format!("Battery: {v}"));
                        }
                        if let Some(ok) = dev.battery_soc_ok {
                            extras.push(format!(
                                "Battery OK: {}",
                                if ok { "yes" } else { "LOW — charge before flashing!" }
                            ));
                        }
                        if let Some(sz) = dev.max_download_size {
                            extras.push(format!(
                                "Max Download: {:.0} MB",
                                sz as f64 / (1024.0 * 1024.0)
                            ));
                        }
                        if let Some(anti) = &dev.anti_rollback {
                            extras.push(format!("Anti-Rollback: {anti}"));
                        }
                        if let Some(bl) = &dev.version_bootloader {
                            extras.push(format!("Bootloader Ver: {bl}"));
                        }
                        if !extras.is_empty() {
                            ui.add_space(2.0);
                            ui.small(
                                egui::RichText::new(extras.join("  |  ")).color(colors::TEXT_MUTED),
                            );
                        }
                        if dev.battery_soc_ok == Some(false) {
                            ui.colored_label(
                                colors::ERROR,
                                "Warning: Device reports low battery. Charge above 30% before flashing.",
                            );
                        }
                    });
                }
                Err(err) => {
                    ui.colored_label(
                        colors::ERROR,
                        format!("Device Probe Warning: {err}"),
                    );
                }
            }
        }
    }

    fn render_live_progress_block(&self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if self.is_flashing {
                ui.spinner();
                ui.label(format!(
                    "Operation in progress… ({:.0}s elapsed)",
                    self.started
                        .map(|s| s.elapsed().as_secs_f64())
                        .unwrap_or_default()
                ));
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
                            format!("Active: {part} (chunk {cur}/{tot})")
                        }
                        (Some(part), None) => format!("Active: {part}"),
                        (None, _) if sp.completed_steps >= sp.total_steps && sp.total_steps > 0 => {
                            "Completed all script steps".into()
                        }
                        (None, _) => "Preparing / checking device…".into(),
                    };
                    ui_elements::status_pill(ui, &active_text, colors::ACCENT_GREEN);
                    ui.label(format!("Steps: {}/{}", sp.completed_steps, sp.total_steps));
                    if sp.total_bytes > 0 {
                        ui.label(format!(
                            "| {} / {}",
                            format_bytes_human(sp.completed_bytes),
                            format_bytes_human(sp.total_bytes)
                        ));
                    }
                    if let Some(eta) = sp.eta_seconds {
                        if eta > 0 {
                            ui_elements::status_pill(
                                ui,
                                &format!("ETA ~{}", format_duration_short(eta)),
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
                        format!(", +{extra} more")
                    } else {
                        String::new()
                    };
                    ui.add_space(2.0);
                    ui.small(
                        egui::RichText::new(format!(
                            "Remaining ({}): {}{}",
                            sp.remaining_summary.len(),
                            shown.join(", "),
                            suffix
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
            ui.heading("Fastboot & Recovery Devices");
            if ui
                .add_enabled(!is_busy, egui::Button::new("Refresh Devices"))
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
        ui.collapsing("Reboot, A/B Slot & Tool Paths", |ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(adb) = self.selected_adb.clone() {
                        ui.label(format!("From ADB ({adb}):"));
                        if ui.button("Reboot to Bootloader").clicked() {
                            self.run_adb_reboot(adb.clone(), Some("bootloader"));
                        }
                        if ui.button("Reboot to Fastbootd").clicked() {
                            self.run_adb_reboot(adb.clone(), Some("fastboot"));
                        }
                        if ui.button("Reboot to Recovery").clicked() {
                            self.run_adb_reboot(adb, Some("recovery"));
                        }
                    } else {
                        ui.small("No active ADB device selected in Dashboard.");
                    }
                });

                if let Some(fb) = self.selected_fastboot.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("From Fastboot ({fb}):"));
                        if ui.button("Reboot System").clicked() {
                            self.run_fastboot_reboot(fb.clone(), "");
                        }
                        if ui.button("Reboot Bootloader").clicked() {
                            self.run_fastboot_reboot(fb.clone(), "bootloader");
                        }
                        if ui.button("Reboot Fastbootd").clicked() {
                            self.run_fastboot_reboot(fb.clone(), "fastboot");
                        }
                        if ui.button("Reboot Recovery").clicked() {
                            self.run_fastboot_reboot(fb.clone(), "recovery");
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        ui.label("A/B Active Slot:");
                        if ui.button("Set Active Slot A").clicked() {
                            self.run_set_active_slot(fb.clone(), "a");
                        }
                        if ui.button("Set Active Slot B").clicked() {
                            self.run_set_active_slot(fb, "b");
                        }
                    });
                }

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("fastboot binary:");
                    ui.add(egui::TextEdit::singleline(&mut self.fastboot_bin).desired_width(320.0));
                    ui.label("adb binary:");
                    ui.add(egui::TextEdit::singleline(&mut self.adb_bin).desired_width(240.0));
                });
            });
        });

        ui.separator();

        // 3. Mode Tabs
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tab, FlashTab::FullRom, "Full ROM Manifest");
            ui.selectable_value(
                &mut self.tab,
                FlashTab::SinglePartition,
                "Single Partition / Boot",
            );
            ui.selectable_value(&mut self.tab, FlashTab::AdbSideload, "ADB Sideload (ZIP)");
            ui.selectable_value(
                &mut self.tab,
                FlashTab::ManifestGenerator,
                "Manifest Generator",
            );
            ui.selectable_value(&mut self.tab, FlashTab::LockBootloader, "Stock ROM & Lock");
            ui.selectable_value(&mut self.tab, FlashTab::Guide, "Guide / Hướng dẫn");
        });
        ui.small("Tip: Drag & drop .json / .img / .zip or a ROM folder onto this window.");

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
                ui.small(format!("Journal log: {}", log.display()));
            }
            if ui.small_button("Open Logs Folder").clicked() {
                open_folder_in_os(&reports_dir());
            }
        });

        ui.horizontal(|ui| {
            ui.label("Console Output:");
            if ui.button("Clear Console").clicked() {
                self.output.clear();
            }
            if ui.button("Copy Console").clicked() {
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
        ui.label(
            "Flash a complete ROM using an explicit, SHA-256 checksum-verified manifest.json file.",
        );
        ui.horizontal(|ui| {
            ui.label("Manifest Path:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.manifest_path)
                        .desired_width(ui.available_width() - 90.0),
                )
                .changed()
            {
                self.invalidate_full_plan();
            }
            if ui.button("Browse…").clicked() {
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
                .add_enabled(can_verify, egui::Button::new("Verify & Prepare Plan"))
                .clicked()
            {
                self.start_prepare_plan();
            }
        });

        if let Some(plan) = self.plan_verified.clone() {
            ui.group(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Verified Plan — Product: {} | Build: {} | Mode: {} | Total: {:.2} MB",
                        plan.manifest().product,
                        plan.manifest().build,
                        plan.manifest().mode,
                        plan.total_bytes() as f64 / (1024.0 * 1024.0)
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
            ui.checkbox(
                &mut self.confirm_full_flash,
                "I understand this writes multiple partitions and will NOT disconnect the device.",
            );

            let can_flash = self.confirm_full_flash && self.selected_fastboot.is_some();
            if ui
                .add_enabled_ui(can_flash, |ui| {
                    ui_elements::accent_button(ui, "Start Full ROM Flash", colors::ACCENT_BLUE)
                })
                .inner
                .clicked()
            {
                self.start_full_flash(plan);
            }
        }
    }

    fn render_single_partition_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Flash a single partition image (boot, recovery, init_boot, vbmeta, super, etc.) or temporarily boot a boot/recovery image.",
        );

        let active_slot = self
            .device_info
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|d| d.slot.as_str())
            .unwrap_or("");

        ui.horizontal_wrapped(|ui| {
            ui.label("Common Partitions:");
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
            ui.label("Target Partition:");
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
            ui.label("Image File Path (.img):");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.single_image_path)
                        .desired_width(ui.available_width() - 90.0),
                )
                .changed()
            {
                self.invalidate_single(true);
            }
            if ui.button("Browse…").clicked() {
                self.pick_path(PickTarget::SingleImage);
            }
        });

        ui.horizontal(|ui| {
            let path_set = !self.single_image_path.trim().is_empty();
            if ui
                .add_enabled(
                    path_set,
                    egui::Button::new("Inspect Image & Compute SHA-256"),
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
                        let mb = *size as f64 / (1024.0 * 1024.0);
                        ui.label(format!(
                            "Size: {mb:.2} MB ({size} bytes) | Format: {}",
                            if *is_sparse {
                                "Android Sparse Image"
                            } else {
                                "Raw Image"
                            }
                        ));
                        ui.label(format!("SHA-256: {hash}"));
                    });
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Inspection error: {e}"));
                }
            }
        }

        ui.add_space(4.0);
        ui.checkbox(
            &mut self.confirm_single_flash,
            format!(
                "I confirm flashing this image to partition '{}' on {}",
                self.partition,
                self.selected_fastboot.as_deref().unwrap_or("device")
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
                        &format!("Flash {}", self.partition),
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
                    egui::Button::new("Temporary Boot (fastboot boot)"),
                )
                .on_hover_text(
                    "Boots a boot/recovery .img in RAM once without writing to flash storage.",
                )
                .clicked()
            {
                self.start_temp_boot();
            }
        });
    }

    fn render_sideload_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Flash an OTA or Custom ROM .zip package via Recovery ADB Sideload (`adb sideload`).",
        );

        ui.horizontal_wrapped(|ui| {
            if self.sideload_devices.is_empty() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "No device in Recovery / Sideload mode. Boot to Recovery and choose 'Apply update from ADB'.",
                );
            } else {
                ui.label("Target Device:");
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
            ui.label("ROM / OTA ZIP Path:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.sideload_zip_path)
                        .desired_width(ui.available_width() - 90.0),
                )
                .changed()
            {
                self.invalidate_sideload(true);
            }
            if ui.button("Browse…").clicked() {
                self.pick_path(PickTarget::SideloadZip);
            }
        });

        ui.horizontal(|ui| {
            let path_set = !self.sideload_zip_path.trim().is_empty();
            if ui
                .add_enabled(path_set, egui::Button::new("Inspect ZIP & Compute SHA-256"))
                .clicked()
            {
                self.start_inspect_sideload();
            }
        });

        if let Some(res) = &self.sideload_inspection {
            match res {
                Ok((size, hash)) => {
                    ui.group(|ui| {
                        let mb = *size as f64 / (1024.0 * 1024.0);
                        ui.label(format!("Size: {mb:.2} MB ({size} bytes)"));
                        ui.label(format!("SHA-256: {hash}"));
                    });
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Inspection error: {e}"));
                }
            }
        }

        ui.add_space(4.0);
        ui.checkbox(
            &mut self.confirm_sideload,
            format!(
                "I confirm sideloading this ZIP package to {}",
                self.selected_sideload.as_deref().unwrap_or("device")
            ),
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
            ui.small("Selected device is in 'recovery' state — activate 'ADB Sideload' on the phone screen, then click Refresh Devices.");
        }

        if ui
            .add_enabled_ui(can_sideload, |ui| {
                ui_elements::accent_button(ui, "Start ADB Sideload", colors::ACCENT_GREEN)
            })
            .inner
            .clicked()
        {
            self.start_adb_sideload();
        }
    }

    fn render_generator_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Scan a folder of partition images (.img), compute SHA-256 digests, and generate a manifest.json.",
        );

        ui.horizontal(|ui| {
            ui.label("Folder Path:");
            ui.add(
                egui::TextEdit::singleline(&mut self.gen_folder)
                    .desired_width(ui.available_width() - 90.0),
            );
            if ui.button("Browse…").clicked() {
                self.pick_path(PickTarget::GeneratorFolder);
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("Product (Device Codename):");
            ui.add(egui::TextEdit::singleline(&mut self.gen_product).desired_width(180.0));
            if let Some(Ok(dev)) = &self.device_info {
                if ui.button(format!("Use '{}'", dev.product)).clicked() {
                    self.gen_product = dev.product.clone();
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Build Identifier:");
            ui.add(egui::TextEdit::singleline(&mut self.gen_build).desired_width(180.0));
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("Fastboot Mode:");
            ui.radio_value(
                &mut self.gen_mode,
                "bootloader".into(),
                "bootloader (boot / recovery / dtbo / vbmeta / super)",
            );
            ui.radio_value(
                &mut self.gen_mode,
                "fastbootd".into(),
                "fastbootd (dynamic system / vendor / product)",
            );
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("A/B Slot Suffix:");
            ui.radio_value(
                &mut self.gen_slot,
                SlotTarget::None,
                "None (non-A/B or already suffixed)",
            );
            ui.radio_value(&mut self.gen_slot, SlotTarget::SlotA, "Slot _a");
            ui.radio_value(&mut self.gen_slot, SlotTarget::SlotB, "Slot _b");
        });

        let can_generate = !self.gen_folder.trim().is_empty()
            && !self.gen_product.trim().is_empty()
            && !self.gen_build.trim().is_empty();

        if ui
            .add_enabled(
                can_generate,
                egui::Button::new("Generate manifest.json from Folder"),
            )
            .clicked()
        {
            self.start_generate_manifest();
        }

        if let Some(res) = &self.gen_result {
            match res {
                Ok((m, p)) => {
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Success! Saved: {}", p.display()))
                                .color(egui::Color32::GREEN),
                        );
                        ui.label(format!(
                            "Product: {} | Build: {} | Mode: {}",
                            m.product, m.build, m.mode
                        ));
                        for img in &m.images {
                            ui.label(format!("• {} ← {}", img.partition, img.file));
                        }
                        if ui.button("Switch to Full ROM Tab to Flash").clicked() {
                            self.tab = FlashTab::FullRom;
                        }
                    });
                }
                Err(err) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Error: {err}"));
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
                    format!(
                        "Current mode is '{}'. Reboot to Bootloader first before running Stock ROM script.",
                        dev.mode
                    ),
                );
            } else if !dev.unlocked {
                ui.colored_label(
                    egui::Color32::GREEN,
                    "Bootloader is already LOCKED on this device.",
                );
            }
        }

        ui.group(|ui| {
            ui.label(
                egui::RichText::new(
                    "Stock Fastboot ROM Runner (flash_all_lock / flash_all / flash_all_except_storage)",
                )
                .strong(),
            );
            ui.small(
                "Parses all script steps & image sizes, enforces Product & Anti-Rollback checks, and tracks sparse chunks + ETA in real time.",
            );

            ui.horizontal(|ui| {
                ui.label("Extracted Stock ROM Folder:");
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
                if ui.button("Browse…").clicked() {
                    self.pick_path(PickTarget::StockLockFolder);
                }
            });

            let can_inspect = !self.lock_rom_folder.trim().is_empty();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_inspect,
                        egui::Button::new("Inspect Stock ROM & Verify Compatibility"),
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
                            ui.label("Script Mode:");
                            for mode in StockFlashMode::ALL {
                                let available = info.available_modes.contains(&mode);
                                let mut selected_mode = self.stock_mode;
                                let color = match mode {
                                    StockFlashMode::CleanAllAndLock => egui::Color32::LIGHT_RED,
                                    StockFlashMode::CleanAll => egui::Color32::YELLOW,
                                    StockFlashMode::KeepUserData => egui::Color32::LIGHT_GREEN,
                                };
                                let label = egui::RichText::new(mode.label()).color(color);
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
                            ui.label(
                                egui::RichText::new(format!(
                                    "Script: {} ({} flash steps | {} total)",
                                    info.script_path.display(),
                                    info.flash_command_count,
                                    format_bytes_human(info.total_bytes),
                                ))
                                .color(egui::Color32::GREEN),
                            );
                            if let Some(expected) = &info.expected_product {
                                let dev_prod = dev_ok.as_ref().map(|d| d.product.as_str());
                                if dev_prod == Some(expected.as_str()) {
                                    ui.colored_label(
                                        egui::Color32::GREEN,
                                        format!("Product check: MATCH ({expected})"),
                                    );
                                } else if let Some(actual) = dev_prod {
                                    compat_ok = false;
                                    ui.colored_label(
                                        egui::Color32::LIGHT_RED,
                                        format!(
                                            "Product MISMATCH: device is '{actual}', ROM expects '{expected}' (BLOCKED)"
                                        ),
                                    );
                                } else {
                                    ui.label(format!("Expected Product: {expected}"));
                                }
                            }
                            if let Some(rom_anti) = info.rom_anti_version {
                                let dev_anti = dev_ok
                                    .as_ref()
                                    .and_then(|d| d.anti_rollback.as_deref())
                                    .and_then(|s| s.trim().parse::<u32>().ok());
                                if let Some(da) = dev_anti {
                                    if da > rom_anti {
                                        compat_ok = false;
                                        ui.colored_label(
                                            egui::Color32::LIGHT_RED,
                                            format!(
                                                "Anti-Rollback BLOCKED: device={da} > ROM={rom_anti}"
                                            ),
                                        );
                                    } else {
                                        ui.colored_label(
                                            egui::Color32::GREEN,
                                            format!(
                                                "Anti-Rollback check: PASS (device={da}, ROM={rom_anti})"
                                            ),
                                        );
                                    }
                                } else {
                                    ui.label(format!("ROM Anti-Rollback version: {rom_anti}"));
                                }
                            }

                            egui::CollapsingHeader::new(format!(
                                "Parsed flash & lock steps ({})",
                                info.steps.len()
                            ))
                            .id_salt("stock_parsed_steps_list")
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("stock_steps_scroll")
                                    .max_height(150.0)
                                    .show(ui, |ui| {
                                        for (idx, step) in info.steps.iter().enumerate() {
                                            let cond = if step.conditional {
                                                " [conditional]"
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
                            StockFlashMode::CleanAllAndLock => {
                                "I backed up my data, signed out of Mi/Google Account, and accept full Factory Reset + Bootloader Lock."
                            }
                            StockFlashMode::CleanAll => {
                                "I backed up my data and accept full Factory Reset (userdata wipe; bootloader stays Unlocked)."
                            }
                            StockFlashMode::KeepUserData => {
                                "I confirm updating Stock ROM while keeping userdata and keeping bootloader Unlocked."
                            }
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
                                ui.label(format!("Type {word} to confirm:"));
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
                            StockFlashMode::CleanAllAndLock => {
                                "Start Flash Stock & Lock Bootloader"
                            }
                            StockFlashMode::CleanAll => "Start Flash Stock (Keep Unlocked)",
                            StockFlashMode::KeepUserData => {
                                "Start Update Stock (Keep User Data)"
                            }
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
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("Inspection error: {e}"));
                    }
                }
            }
        });
    }

    fn render_lock_bootloader_tab(&mut self, ui: &mut egui::Ui) {
        ui_elements::tool_subcard(ui, Some(colors::ACCENT_RED), |ui| {
            ui.label(
                egui::RichText::new("WARNING: Stock ROM Flash & Bootloader Lock")
                    .color(colors::ERROR)
                    .strong(),
            );
            ui.label("• Locking the bootloader or running `flash_all` will ERASE ALL USER DATA (Factory Reset). Back up first.");
            ui.label("• ONLY lock on 100% matching Stock ROM for your device region (e.g. CN device + CN Stock ROM). Locking on Custom ROM or mismatched region ROM will brick the device.");
            ui.label("• Sign out of Mi Account (Find Device OFF) & Google Account before locking. Use a USB 2.0 port on AMD PCs.");
            ui.label("• Device MUST be in `bootloader` mode (not `fastbootd`) and currently `Unlocked`.");
        });

        ui.add_space(6.0);

        self.render_stock_rom_selector_and_run(ui);

        ui.add_space(6.0);

        let dev_ok = self.device_info.as_ref().and_then(|r| r.as_ref().ok());
        let is_bootloader_unlocked = dev_ok.is_some_and(|d| d.mode == "bootloader" && d.unlocked);

        // Mode B: Direct Fastboot Lock (only if 100% stock is already installed)
        ui.collapsing(
            "Option 2 (Advanced): Direct Lock Command (ONLY if full Stock ROM is already installed)",
            |ui| {
                ui.small(
                    "Use only on devices already running 100% unmodified Stock firmware (e.g. Pixel / Motorola / Xiaomi after full stock flash).",
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("Lock Command:");
                    ui.radio_value(
                        &mut self.direct_lock_method,
                        LockMethod::OemLock,
                        "fastboot oem lock (Xiaomi / OnePlus / OEM)",
                    );
                    ui.radio_value(
                        &mut self.direct_lock_method,
                        LockMethod::FlashingLock,
                        "fastboot flashing lock (Pixel / AOSP)",
                    );
                });
                ui.checkbox(
                    &mut self.confirm_direct_lock,
                    "I confirm this device is already running 100% unmodified Stock ROM matching its region.",
                );
                ui.horizontal(|ui| {
                    ui.label("Type LOCK to confirm:");
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
                            &format!("Run `{}`", self.direct_lock_method.display_cmd()),
                            colors::ACCENT_RED,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.start_direct_lock();
                }
            },
        );
    }

    fn render_guide_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let vi = self.guide_lang == GuideLang::Vi;

        ui.horizontal_wrapped(|ui| {
            ui.heading(if vi {
                "Hướng dẫn sử dụng Flash ROM & Fastboot Tools"
            } else {
                "Flash ROM & Fastboot Tools Guide"
            });
            ui.add_space(12.0);
            ui.label(if vi { "Ngôn ngữ:" } else { "Language:" });
            ui.selectable_value(&mut self.guide_lang, GuideLang::Vi, "VI");
            ui.selectable_value(&mut self.guide_lang, GuideLang::En, "EN");
        });

        ui.add_space(4.0);

        // Quick jump buttons
        ui.horizontal_wrapped(|ui| {
            ui.label(if vi { "Đi nhanh:" } else { "Jump to:" });
            if ui.small_button("Full ROM Manifest").clicked() {
                self.tab = FlashTab::FullRom;
            }
            if ui.small_button("Single Partition / Boot").clicked() {
                self.tab = FlashTab::SinglePartition;
            }
            if ui.small_button("ADB Sideload (ZIP)").clicked() {
                self.tab = FlashTab::AdbSideload;
            }
            if ui.small_button("Manifest Generator").clicked() {
                self.tab = FlashTab::ManifestGenerator;
            }
            if ui.small_button("Stock ROM & Lock").clicked() {
                self.tab = FlashTab::LockBootloader;
            }
            if ui
                .small_button(if vi {
                    "Copy mẫu manifest.json"
                } else {
                    "Copy Sample manifest.json"
                })
                .clicked()
            {
                ctx.copy_text(SAMPLE_MANIFEST_JSON.to_string());
                self.status = if vi {
                    "Đã copy mẫu manifest.json vào clipboard.".into()
                } else {
                    "Copied sample manifest.json to clipboard.".into()
                };
                self.error = false;
            }
        });

        ui.add_space(6.0);

        // Safety checklist
        ui.group(|ui| {
            ui.label(
                egui::RichText::new(if vi {
                    "Checklist an toàn trước khi Flash"
                } else {
                    "Pre-Flash Safety Checklist"
                })
                .color(egui::Color32::YELLOW)
                .strong(),
            );
            if vi {
                ui.label("• Pin điện thoại nên >= 50% (xem dòng Battery OK ở phần thông tin thiết bị).");
                ui.label("• Sao lưu dữ liệu quan trọng trước (có thể dùng nút Backup / Transfer… trên Dashboard).");
                ui.label("• Bootloader bắt buộc ở trạng thái Unlocked. Module này KHÔNG tự unlock bootloader và KHÔNG chạy lệnh erase/format userdata.");
                ui.label("• Dùng cáp USB tốt, cắm trực tiếp vào cổng USB máy tính; tuyệt đối không rút cáp khi đang ghi.");
                ui.label("• Mọi thao tác flash/sideload đều được ghi nhật ký (journal log) — bấm 'Open Logs Folder' để xem lại.");
            } else {
                ui.label("• Keep battery >= 50% (check 'Battery OK' in the device info bar).");
                ui.label("• Back up important data first (you can use 'Backup / Transfer…' in Dashboard).");
                ui.label("• Bootloader must be Unlocked. This module NEVER unlocks bootloaders or runs erase/format userdata.");
                ui.label("• Use a reliable USB cable plugged directly into the PC; never disconnect during a write.");
                ui.label("• Every flash/sideload session writes a journal log — click 'Open Logs Folder' to inspect.");
            }
        });

        ui.add_space(4.0);

        egui::CollapsingHeader::new(if vi {
            "1. Full ROM Manifest — Flash trọn bộ ROM từ manifest.json"
        } else {
            "1. Full ROM Manifest — Flash a full ROM via manifest.json"
        })
        .default_open(true)
        .show(ui, |ui| {
            if vi {
                ui.label("1. Đưa máy vào chế độ Bootloader hoặc Fastbootd (dùng mục 'Reboot, A/B Slot & Tool Paths' hoặc phím cứng) rồi bấm 'Refresh Devices'.");
                ui.label("2. Nếu thư mục ROM chỉ có các file .img và chưa có manifest.json, sang tab 'Manifest Generator' để tạo trước.");
                ui.label("3. Chọn (Browse…) hoặc kéo-thả file manifest.json vào cửa sổ.");
                ui.label("4. Bấm 'Verify & Prepare Plan': tool sẽ đối chiếu product, mode, dung lượng phân vùng và toàn bộ mã SHA-256.");
                ui.label("5. Nếu flash Custom ROM/GSI có vbmeta, bật thêm '--disable-verity' và '--disable-verification' nếu cần.");
                ui.label("6. Tick xác nhận và bấm 'Start Full ROM Flash'. Khi hoàn tất, máy giữ nguyên ở Fastboot để bạn kiểm tra log rồi tự reboot.");
            } else {
                ui.label("1. Boot the device into Bootloader or Fastbootd (via 'Reboot, A/B Slot & Tool Paths' or hardware keys) and click 'Refresh Devices'.");
                ui.label("2. If your ROM folder only has .img files without manifest.json, create one first in the 'Manifest Generator' tab.");
                ui.label("3. Browse or drag & drop manifest.json into this window.");
                ui.label("4. Click 'Verify & Prepare Plan': the tool checks product, mode, partition sizes, and every SHA-256 digest.");
                ui.label("5. For Custom ROMs/GSIs with vbmeta, optionally enable '--disable-verity' and '--disable-verification'.");
                ui.label("6. Check the confirmation box and click 'Start Full ROM Flash'. The device stays in Fastboot after completion so you can review logs before rebooting.");
            }
        });

        egui::CollapsingHeader::new(if vi {
            "2. Single Partition / Boot — Flash lẻ 1 phân vùng hoặc Boot tạm (RAM)"
        } else {
            "2. Single Partition / Boot — Flash a single partition or Temporary Boot"
        })
        .show(ui, |ui| {
            if vi {
                ui.label("• Phù hợp khi cài Magisk (boot/init_boot), Custom Recovery (recovery/vendor_boot), dtbo, vbmeta hoặc super.");
                ui.label("• Trên máy A/B, chọn rõ phân vùng có hậu tố _a hoặc _b (ví dụ boot_a, init_boot_b). Tool tự gợi ý theo slot đang chạy.");
                ui.label("• Bấm 'Inspect Image & Compute SHA-256' để kiểm tra dung lượng, định dạng Raw/Sparse và mã SHA-256.");
                ui.label("• Nút 'Temporary Boot (fastboot boot)': chạy thử boot.img hoặc recovery.img trực tiếp trên RAM mà KHÔNG ghi đè flash (rất an toàn để test).");
            } else {
                ui.label("• Ideal for flashing Magisk (boot/init_boot), Custom Recovery (recovery/vendor_boot), dtbo, vbmeta, or super.");
                ui.label("• On A/B devices, always pick an explicit _a or _b partition (e.g. boot_a, init_boot_b). Quick presets adapt to the active slot.");
                ui.label("• Click 'Inspect Image & Compute SHA-256' to check size, Raw/Sparse format, and SHA-256.");
                ui.label("• 'Temporary Boot (fastboot boot)' boots a boot/recovery image in RAM once without writing to flash storage.");
            }
        });

        egui::CollapsingHeader::new(if vi {
            "3. ADB Sideload (ZIP) — Cài Custom ROM / OTA qua Recovery"
        } else {
            "3. ADB Sideload (ZIP) — Flash Custom ROM / OTA via Recovery"
        })
        .show(ui, |ui| {
            if vi {
                ui.label("1. Khởi động máy vào Recovery (TWRP, OrangeFox, Lineage Recovery hoặc Stock Recovery hỗ trợ sideload).");
                ui.label("2. Trên điện thoại, chọn 'Apply update' → 'Apply from ADB' (hoặc 'ADB Sideload').");
                ui.label("3. Bấm 'Refresh Devices' — thiết bị sẽ hiện ở tab ADB Sideload với trạng thái '(sideload)'.");
                ui.label("4. Chọn hoặc kéo-thả file .zip, có thể bấm 'Inspect ZIP & Compute SHA-256' để đối chiếu checksum.");
                ui.label("5. Tick xác nhận và bấm 'Start ADB Sideload'. Theo dõi phần trăm tiến độ realtime ở thanh progress.");
            } else {
                ui.label("1. Reboot into Recovery (TWRP, OrangeFox, Lineage Recovery, or stock recovery with sideload support).");
                ui.label("2. On the phone, choose 'Apply update' → 'Apply from ADB' (or 'ADB Sideload').");
                ui.label("3. Click 'Refresh Devices' — the device should appear with '(sideload)' state.");
                ui.label("4. Browse or drag & drop the .zip package, and optionally inspect its SHA-256 checksum.");
                ui.label("5. Confirm and click 'Start ADB Sideload'. Live progress percentage is streamed to the progress bar.");
            }
        });

        egui::CollapsingHeader::new(if vi {
            "4. Manifest Generator — Tự tạo manifest.json từ thư mục .img"
        } else {
            "4. Manifest Generator — Generate manifest.json from an .img folder"
        })
        .show(ui, |ui| {
            if vi {
                ui.label("• Chọn hoặc kéo-thả thư mục chứa các file .img (tên file trùng tên phân vùng, ví dụ boot.img, dtbo.img, vbmeta.img, super.img).");
                ui.label("• Nhập Product (hoặc bấm nút 'Use ...' để lấy trực tiếp từ máy đang cắm), nhập tên Build và chọn Mode.");
                ui.label("• Với máy A/B, chọn 'A/B Slot Suffix' là 'Slot _a' hoặc 'Slot _b' (tool tự giữ nguyên tên 'super' và các file đã có _a/_b).");
                ui.label("• Bấm 'Generate manifest.json from Folder' → sau khi xong, bấm 'Switch to Full ROM Tab to Flash'.");
            } else {
                ui.label("• Browse or drag & drop a folder of .img files (filenames matching partition names, e.g. boot.img, dtbo.img, vbmeta.img, super.img).");
                ui.label("• Enter Product (or click 'Use ...' from the connected device), Build identifier, and Fastboot Mode.");
                ui.label("• For A/B devices, select 'Slot _a' or 'Slot _b' (non-slotted 'super' and already-suffixed files are preserved).");
                ui.label("• Click 'Generate manifest.json from Folder', then switch to the Full ROM tab to verify and flash.");
            }
        });

        egui::CollapsingHeader::new(if vi {
            "5. Stock ROM & Lock Bootloader (flash_all_lock / flash_all / flash_all_except_storage)"
        } else {
            "5. Stock ROM & Lock Bootloader (flash_all_lock / flash_all / flash_all_except_storage)"
        })
        .show(ui, |ui| {
            if vi {
                ui.label("• Chọn hoặc kéo-thả thư mục ROM Fastboot Stock (chứa flash_all_lock.bat / flash_all.bat / flash_all_except_storage.bat và thư mục images/).");
                ui.label("• 3 chế độ: Clean All & Lock (xóa dữ liệu + khóa bootloader), Clean All (xóa dữ liệu, giữ unlocked), Keep User Data (giữ dữ liệu + giữ unlocked).");
                ui.label("• Trước khi chạy, công cụ kiểm tra Product codename, Anti-Rollback, trạng thái Unlocked, chế độ bootloader và pin; trong lúc chạy sẽ hiển thị chunk sparse (vd super 3/11), phân vùng đang ghi, danh sách còn lại và ETA.");
                ui.label("• LƯU Ý KHÓA BOOTLOADER: Thoát Mi Account (tắt Find Device) & Google Account, dùng đúng ROM Stock cùng vùng máy (VD: máy nội địa CN dùng CN Stock ROM), cắm cổng USB 2.0 ổn định trên PC AMD.");
                ui.label("• Option 2 (Direct Lock Command): chỉ dùng khi máy đã chạy 100% Stock ROM nguyên bản đúng vùng.");
            } else {
                ui.label("• Browse or drag & drop an extracted Stock Fastboot ROM folder (containing flash_all_lock / flash_all / flash_all_except_storage and images/).");
                ui.label("• 3 modes: Clean All & Lock (wipes userdata + locks bootloader), Clean All (wipes userdata, keeps unlocked), Keep User Data (preserves userdata + keeps unlocked).");
                ui.label("• Pre-flight gates verify product codename, anti-rollback version, bootloader mode, unlocked state, and battery; live progress tracks sparse chunks (e.g. super 3/11), active partition, remaining list, and ETA.");
                ui.label("• LOCK WARNING: Sign out of Mi Account (Find Device OFF) & Google Account, use 100% matching region Stock ROM, and prefer USB 2.0 ports on AMD PCs.");
                ui.label("• Option 2 (Direct Lock Command): use only if the device is already running 100% unmodified regional Stock firmware.");
            }
        });

        egui::CollapsingHeader::new(if vi {
            "6. Phân biệt Bootloader vs Fastbootd & Quản lý A/B Slot"
        } else {
            "6. Bootloader vs Fastbootd & A/B Slot Reference"
        })
        .show(ui, |ui| {
            if vi {
                ui.label("• bootloader mode: Flash phân vùng phần cứng / khởi động — boot, init_boot, vendor_boot, vendor_kernel_boot, dtbo, recovery, vbmeta*, super.");
                ui.label("• fastbootd mode (userspace): Flash phân vùng động nằm trong super — system, system_ext, vendor, product, odm, *_dlkm.");
                ui.label("• Các phân vùng nguy hiểm (userdata, metadata, persist, abl, xbl...) luôn bị chặn để tránh hard-brick / mất dữ liệu.");
                ui.label("• A/B Active Slot: Chỉ chuyển giữa Slot A và Slot B khi chắc chắn slot kia có bản ROM khởi động được.");
            } else {
                ui.label("• bootloader mode: Hardware/boot partitions — boot, init_boot, vendor_boot, vendor_kernel_boot, dtbo, recovery, vbmeta*, super.");
                ui.label("• fastbootd mode (userspace): Logical dynamic partitions inside super — system, system_ext, vendor, product, odm, *_dlkm.");
                ui.label("• Dangerous partitions (userdata, metadata, persist, abl, xbl...) are always blocked to prevent hard-bricks and data loss.");
                ui.label("• A/B Active Slot: Switch between Slot A and Slot B only when you know the target slot contains a bootable image.");
            }
        });

        egui::CollapsingHeader::new(if vi {
            "7. Xử lý sự cố thường gặp (Troubleshooting)"
        } else {
            "7. Common Troubleshooting"
        })
        .show(ui, |ui| {
            if vi {
                ui.label("• Không thấy máy ở Fastboot trên Windows: Cài Google USB Driver / driver của hãng (Xiaomi, Pixel, Motorola…) và kiểm tra Device Manager (Android Bootloader Interface).");
                ui.label("• Lỗi 'Cannot start fastboot': Mở mục 'Reboot, A/B Slot & Tool Paths' và trỏ trực tiếp tới fastboot.exe trong Android SDK platform-tools.");
                ui.label("• Lỗi 'Choose an explicit _a or _b partition': Thiết bị dùng A/B slot — hãy chọn boot_a/boot_b thay vì boot, hoặc chọn Slot _a/_b khi tạo manifest.");
                ui.label("• Lỗi 'Device product or mode does not match': Kiểm tra đúng ROM cho mã máy (codename) và đúng chế độ bootloader/fastbootd.");
                ui.label("• Lỗi giữa chừng khi đang flash: KHÔNG tự reboot ngay! Xem Console Output / Journal log, kiểm tra cáp USB và flash lại phân vùng bị lỗi trước khi khởi động lại.");
            } else {
                ui.label("• Fastboot device not detected on Windows: Install Google USB Driver / OEM driver and check Device Manager for 'Android Bootloader Interface'.");
                ui.label("• 'Cannot start fastboot': Open 'Reboot, A/B Slot & Tool Paths' and point directly to platform-tools/fastboot.exe.");
                ui.label("• 'Choose an explicit _a or _b partition': Device uses A/B slots — pick boot_a/boot_b instead of boot, or set Slot _a/_b in Manifest Generator.");
                ui.label("• 'Device product or mode does not match': Verify the ROM matches your device codename and you are in the required mode (bootloader vs fastbootd).");
                ui.label("• Write failed mid-flash: Do NOT reboot immediately. Review Console Output and the journal log, check the USB cable, and re-flash the failed partition first.");
            }
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
        self.status = "Verifying manifest and computing SHA-256 checksums…".into();
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
        self.status = "Inspecting image and computing SHA-256…".into();
        self.error = false;
        std::thread::spawn(move || {
            let res = if !path.is_file() {
                Err("File does not exist or is not a regular file".into())
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
        self.status = "Inspecting ZIP and computing SHA-256…".into();
        self.error = false;
        std::thread::spawn(move || {
            let res = if !path.is_file() {
                Err("File does not exist or is not a regular file".into())
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
        self.status = "Scanning folder and hashing .img files…".into();
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
        self.status = format!("ADB: Rebooting {serial} to {}…", target.unwrap_or("system"));
        self.error = false;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = Adb::reboot(Some(&serial), target)
                .map(|_| {
                    format!(
                        "ADB reboot ({}) sent to {serial}.",
                        target.unwrap_or("system")
                    )
                })
                .map_err(|e| format!("ADB reboot failed: {}", e.user_message()));
            let _ = tx.send(FlashEvent::QuickActionDone(res));
        });
    }

    fn run_fastboot_reboot(&mut self, serial: String, target: &'static str) {
        self.pending_bg_tasks += 1;
        self.status = format!(
            "Fastboot: Rebooting {serial} to {}…",
            if target.is_empty() { "system" } else { target }
        );
        self.error = false;
        let fb_bin = self.fastboot_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = flash::reboot_with_bin(fb_bin.as_deref(), Some(&serial), target)
                .map(|t| format!("Fastboot reboot: {}", t.trim()))
                .map_err(|e| format!("Fastboot reboot error: {e}"));
            let _ = tx.send(FlashEvent::QuickActionDone(res));
        });
    }

    fn run_set_active_slot(&mut self, serial: String, slot: &'static str) {
        self.pending_bg_tasks += 1;
        self.status = format!("Switching active slot to '{slot}' on {serial}…");
        self.error = false;
        let fb_bin = self.fastboot_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = flash::set_active_slot(fb_bin.as_deref(), &serial, slot)
                .map(|t| format!("Active slot set to '{slot}': {}", t.trim()))
                .map_err(|e| format!("Slot switch failed: {e}"));
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
            "Starting full ROM flash on {serial}…\nLog: {}\n\n",
            log_path.display()
        );

        let fb_bin = self.fastboot_override();
        let options = self.full_options.clone();
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
            let _ = tx.send(FlashEvent::WriteFinished(result.map(|()| {
                "Full ROM flashed successfully. Reboot manually when ready.".into()
            })));
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
            "Starting single partition flash [{partition}] on {serial}…\nFile: {}\nLog: {}\n\n",
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
            let _ =
                tx.send(FlashEvent::WriteFinished(result.map(|()| {
                    format!("Partition {partition} flashed successfully.")
                })));
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
            "Starting temporary boot (`fastboot boot`) on {serial}…\nFile: {}\nLog: {}\n\n",
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
            let _ = tx.send(FlashEvent::WriteFinished(
                result.map(|()| "Temporary image booted.".into()),
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
            "Starting ADB sideload on {serial}…\nFile: {}\nLog: {}\n\n",
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
            let _ = tx.send(FlashEvent::WriteFinished(
                result.map(|()| "ADB sideload completed.".into()),
            ));
        });
    }

    fn start_inspect_stock_lock(&mut self) {
        let folder = PathBuf::from(self.lock_rom_folder.trim());
        let tx = self.tx.clone();
        self.pending_bg_tasks += 1;
        self.status = "Inspecting Stock ROM scripts & image sizes…".into();
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
        self.status = format!("Inspecting {}…", mode.script_name());
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
            "Starting {} on {serial}…\nScript: {}\nLog: {}\n\n",
            info.mode.label(),
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
            let done_msg = if locks_bl {
                "Stock ROM flashed and bootloader locked successfully.".into()
            } else {
                "Stock ROM flashed successfully (bootloader kept unlocked).".into()
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
            "Starting Direct Bootloader Lock (`{}`) on {serial}…\nLog: {}\n\n",
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
            let _ = tx.send(FlashEvent::WriteFinished(
                result.map(|()| format!("`{}` completed.", method.display_cmd())),
            ));
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
        PickTarget::GeneratorFolder => {
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = 'Select ROM images folder'; \
             if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.SelectedPath) }"
        }
        PickTarget::StockLockFolder => {
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = 'Select extracted Stock ROM folder (with flash_all_lock.bat)'; \
             if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.SelectedPath) }"
        }
        PickTarget::ManifestJson => {
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Filter = 'Manifest JSON (*.json)|*.json|All files (*.*)|*.*'; \
             if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.FileName) }"
        }
        PickTarget::SingleImage => {
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Filter = 'Partition Image (*.img)|*.img|All files (*.*)|*.*'; \
             if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.FileName) }"
        }
        PickTarget::SideloadZip => {
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Filter = 'ROM / OTA ZIP (*.zip)|*.zip|All files (*.*)|*.*'; \
             if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.FileName) }"
        }
    };

    let out = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", script])
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
