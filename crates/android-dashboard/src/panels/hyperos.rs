//! "HyperOS China Fix & Việt Hóa" window for optimizing Chinese HyperOS/MIUI
//! phones over PC ADB without installing APKs or background services.

use std::path::PathBuf;
use std::time::Duration;

use adb_client::hyperos::{
    self, DEBLOAT_PACKAGES, DebloatState, DebloatStatus, DeviceHyperOsInfo, LocaleStatus,
    NOTIFICATION_APPS, NotificationAppState, NotificationFixReport, StagedMtz, THEME_CATALOG,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;

use crate::i18n;
use crate::theme::colors;
use crate::ui_elements;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HyperOsTab {
    Notifications,
    LocaleAndTheme,
    Debloat,
    BankLock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DebloatFilter {
    All,
    Installed,
    Removed,
}

impl DebloatFilter {
    fn label(self) -> &'static str {
        match self {
            DebloatFilter::All => "Tất cả",
            DebloatFilter::Installed => "Đang cài",
            DebloatFilter::Removed => "Đã gỡ / Đã tắt",
        }
    }
}

enum HyperOsEvent {
    Log(String),
    DeviceInfoLoaded(String, Result<DeviceHyperOsInfo, String>),
    NotiScanned(String, Result<Vec<NotificationAppState>, String>),
    NotiApplied(Result<NotificationFixReport, String>),
    MilletRead(Vec<String>),
    MilletRestored(Result<String, String>),
    LocaleUpdated(Result<String, String>, LocaleStatus),
    PickedMtz(Option<PathBuf>),
    MtzDeployed(Result<StagedMtz, String>),
    DebloatScanned(String, Result<Vec<DebloatState>, String>),
    DebloatDone {
        action: &'static str,
        ok: usize,
        fail: usize,
        rescanned: Result<Vec<DebloatState>, String>,
    },
    BankLocked(Result<String, String>),
}

pub struct HyperOsDialog {
    pub open: bool,
    adb_bin: String,
    selected_serial: Option<String>,
    active_tab: HyperOsTab,

    device_info: Option<DeviceHyperOsInfo>,
    noti_items: Vec<NotificationAppState>,
    noti_scanned: bool,
    custom_noti_input: String,
    send_fcm_ping: bool,

    locale_status: LocaleStatus,
    themes_dir: PathBuf,
    selected_theme_idx: Option<usize>, // None = custom path
    custom_mtz_path: String,
    mtz_overwrite: bool,

    debloat_items: Vec<DebloatState>,
    debloat_scanned: bool,
    debloat_filter: DebloatFilter,
    confirm_debloat: bool,

    confirm_bank_lock: bool,

    busy: bool,
    busy_label: String,
    pending_pickers: usize,
    status: String,
    error: bool,
    log: String,

    tx: Sender<HyperOsEvent>,
    rx: Receiver<HyperOsEvent>,
}

impl Default for HyperOsDialog {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        let themes_dir = hyperos::resolve_themes_dir();
        let noti_items = NOTIFICATION_APPS
            .iter()
            .map(|a| NotificationAppState {
                package: a.package.to_string(),
                name: a.name.to_string(),
                category: a.category.to_string(),
                installed: false,
                in_millet: false,
                selected: a.default_selected,
            })
            .collect();
        let debloat_items = DEBLOAT_PACKAGES
            .iter()
            .map(|d| DebloatState {
                package: d.package.to_string(),
                name: d.name.to_string(),
                category: d.category.to_string(),
                risk: d.risk,
                description: d.description,
                status: DebloatStatus::NotPresent,
                installed: false,
                restorable: false,
                selected: d.default_selected,
            })
            .collect();

        Self {
            open: false,
            adb_bin: hyperos::resolve_adb_verified(None),
            selected_serial: None,
            active_tab: HyperOsTab::Notifications,
            device_info: None,
            noti_items,
            noti_scanned: false,
            custom_noti_input: String::new(),
            send_fcm_ping: true,
            locale_status: LocaleStatus {
                current: "(chưa đọc)".into(),
                backup: "(chưa sao lưu)".into(),
                has_backup: false,
            },
            themes_dir,
            selected_theme_idx: Some(0),
            custom_mtz_path: String::new(),
            mtz_overwrite: false,
            debloat_items,
            debloat_scanned: false,
            debloat_filter: DebloatFilter::All,
            confirm_debloat: false,
            confirm_bank_lock: false,
            busy: false,
            busy_label: String::new(),
            pending_pickers: 0,
            status: "Sẵn sàng. Chỉ chạy qua PC ADB — không cài APK hay dịch vụ ngầm lên điện thoại.".into(),
            error: false,
            log: String::new(),
            tx,
            rx,
        }
    }
}

impl HyperOsDialog {
    pub fn bind_device(&mut self, serial: Option<&str>) {
        let next = serial.map(str::to_string);
        if self.selected_serial != next {
            self.selected_serial = next.clone();
            self.device_info = None;
            self.noti_scanned = false;
            self.debloat_scanned = false;
            self.confirm_debloat = false;
            self.confirm_bank_lock = false;
            self.locale_status = LocaleStatus {
                current: "(chưa đọc)".into(),
                backup: "(chưa sao lưu)".into(),
                has_backup: false,
            };
            if self.open && next.is_some() && !self.busy {
                self.refresh_device_info();
            }
        }
    }

    pub fn sync_adb_path(&mut self, adb: &str) {
        if !self.busy && !adb.trim().is_empty() {
            self.adb_bin = hyperos::resolve_adb_verified(Some(adb.trim()));
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy || self.pending_pickers > 0
    }

    fn adb_override(&self) -> Option<String> {
        let s = self.adb_bin.trim();
        if s.is_empty() { None } else { Some(s.to_string()) }
    }

    fn append_log(&mut self, msg: &str) {
        let ts = chrono::Local::now().format("%H:%M:%S");
        if !self.log.is_empty() && !self.log.ends_with('\n') {
            self.log.push('\n');
        }
        self.log.push_str(&format!("[{ts}] {msg}\n"));
    }

    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        while let Ok(ev) = self.rx.try_recv() {
            changed = true;
            match ev {
                HyperOsEvent::Log(line) => self.append_log(&line),
                HyperOsEvent::DeviceInfoLoaded(serial, res) => {
                    self.busy = false;
                    if self.selected_serial.as_deref() == Some(serial.as_str()) {
                        match res {
                            Ok(info) => {
                                let backups = hyperos::default_backups_dir();
                                self.locale_status = hyperos::get_locale_status(
                                    self.adb_override().as_deref(),
                                    &serial,
                                    &backups,
                                );
                                self.status = format!(
                                    "Đã đọc thông tin {} {} (Android {} / {}).",
                                    info.brand, info.model, info.android, info.hyperos
                                );
                                self.error = false;
                                self.device_info = Some(info);
                            }
                            Err(e) => {
                                self.status = e.clone();
                                self.error = true;
                                self.append_log(&format!("Lỗi đọc thông tin máy: {e}"));
                            }
                        }
                    }
                }
                HyperOsEvent::NotiScanned(serial, res) => {
                    self.busy = false;
                    if self.selected_serial.as_deref() == Some(serial.as_str()) {
                        match res {
                            Ok(rows) => {
                                let inst = rows.iter().filter(|r| r.installed).count();
                                self.status = format!("Đã quét xong: {inst}/{} app phổ biến đang cài trên máy.", rows.len());
                                self.error = false;
                                self.append_log(&self.status.clone());
                                self.noti_items = rows;
                                self.noti_scanned = true;
                            }
                            Err(e) => {
                                self.status = e.clone();
                                self.error = true;
                                self.append_log(&format!("Lỗi quét app: {e}"));
                            }
                        }
                    }
                }
                HyperOsEvent::NotiApplied(res) => {
                    self.busy = false;
                    match res {
                        Ok(rep) => {
                            self.status = format!(
                                "Hoàn tất Fix Thông báo ({} gói): MILLET={} (tổng {}), Doze={}/{}, AppOps={}/{}.",
                                rep.total_pkgs,
                                if rep.millet_ok { "OK" } else { "Cảnh báo" },
                                rep.millet_total,
                                rep.doze_ok,
                                rep.total_pkgs,
                                rep.appops_ok,
                                rep.total_pkgs
                            );
                            self.error = false;
                            self.refresh_device_info();
                        }
                        Err(e) => {
                            self.status = e.clone();
                            self.error = true;
                            self.append_log(&format!("LỖI Fix Thông báo: {e}"));
                        }
                    }
                }
                HyperOsEvent::MilletRead(items) => {
                    self.busy = false;
                    let text = if items.is_empty() {
                        "(trống)".to_string()
                    } else {
                        items.join(", ")
                    };
                    self.append_log(&format!("MILLET_NO_RESTRICT_APP ({} gói): {text}", items.len()));
                    self.status = format!("MILLET_NO_RESTRICT_APP hiện có {} gói.", items.len());
                    self.error = false;
                }
                HyperOsEvent::MilletRestored(res) => {
                    self.busy = false;
                    match res {
                        Ok(msg) => {
                            self.status = msg;
                            self.error = false;
                            self.refresh_device_info();
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
                    }
                }
                HyperOsEvent::LocaleUpdated(res, loc_st) => {
                    self.busy = false;
                    self.locale_status = loc_st;
                    match res {
                        Ok(msg) => {
                            self.status = msg;
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
                    }
                }
                HyperOsEvent::PickedMtz(opt) => {
                    self.pending_pickers = self.pending_pickers.saturating_sub(1);
                    if let Some(p) = opt {
                        self.custom_mtz_path = p.display().to_string();
                        self.selected_theme_idx = None;
                    }
                }
                HyperOsEvent::MtzDeployed(res) => {
                    self.busy = false;
                    match res {
                        Ok(staged) => {
                            self.status = format!(
                                "Đã nạp \"{}\" (UUID {}). Hãy bấm [Áp dụng] trên màn hình điện thoại!",
                                staged.title, staged.theme_uuid
                            );
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = e.clone();
                            self.error = true;
                            self.append_log(&format!("LỖI nạp MTZ: {e}"));
                        }
                    }
                }
                HyperOsEvent::DebloatScanned(serial, res) => {
                    self.busy = false;
                    if self.selected_serial.as_deref() == Some(serial.as_str()) {
                        match res {
                            Ok(rows) => {
                                let inst = rows.iter().filter(|r| r.installed).count();
                                let rem = rows
                                    .iter()
                                    .filter(|r| {
                                        matches!(
                                            r.status,
                                            DebloatStatus::RemovedUser0 | DebloatStatus::DisabledUser0
                                        )
                                    })
                                    .count();
                                self.status = format!("Quét Debloat xong: {inst} đang cài, {rem} đã gỡ/tắt (User 0).");
                                self.error = false;
                                self.append_log(&self.status.clone());
                                self.debloat_items = rows;
                                self.debloat_scanned = true;
                            }
                            Err(e) => {
                                self.status = e.clone();
                                self.error = true;
                                self.append_log(&format!("Lỗi quét Debloat: {e}"));
                            }
                        }
                    }
                }
                HyperOsEvent::DebloatDone {
                    action,
                    ok,
                    fail,
                    rescanned,
                } => {
                    self.busy = false;
                    self.confirm_debloat = false;
                    self.status = format!("{action}: thành công {ok} gói, thất bại/không có {fail} gói.");
                    self.error = ok == 0 && fail > 0;
                    if let Ok(rows) = rescanned {
                        self.debloat_items = rows;
                        self.debloat_scanned = true;
                    }
                }
                HyperOsEvent::BankLocked(res) => {
                    self.busy = false;
                    self.confirm_bank_lock = false;
                    match res {
                        Ok(msg) => {
                            self.status = msg;
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
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
        if self.device_info.is_none() && self.selected_serial.is_some() && !self.busy {
            self.refresh_device_info();
        }

        let mut open = self.open;
        egui::Window::new(i18n::t("hyperos.window_title"))
            .open(&mut open)
            .default_width(920.0)
            .default_height(720.0)
            .vscroll(false)
            .show(ctx, |ui| {
                self.render_header(ui);
                ui.add_space(6.0);

                ui_elements::folder_tab_bar(
                    ui,
                    &mut self.active_tab,
                    &[
                        (HyperOsTab::Notifications, i18n::t("hyperos.tab_notifications")),
                        (HyperOsTab::LocaleAndTheme, i18n::t("hyperos.tab_locale_theme")),
                        (HyperOsTab::Debloat, i18n::t("hyperos.tab_debloat")),
                        (HyperOsTab::BankLock, i18n::t("hyperos.tab_bank_lock")),
                    ],
                );
                ui.add_space(6.0);

                let has_device = self.selected_serial.is_some();
                let can_act = has_device && !self.busy;

                match self.active_tab {
                    HyperOsTab::Notifications => self.render_tab_notifications(ui, can_act),
                    HyperOsTab::LocaleAndTheme => self.render_tab_locale_theme(ui, can_act),
                    HyperOsTab::Debloat => self.render_tab_debloat(ui, can_act),
                    HyperOsTab::BankLock => self.render_tab_bank_lock(ui, can_act),
                }

                ui.add_space(6.0);
                self.render_log_and_status(ui, ctx);
            });
        self.open = open;

        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn render_header(&mut self, ui: &mut egui::Ui) {
        ui_elements::tool_card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui_elements::section_header(
                    ui,
                    "HyperOS China Fix & Việt Hóa (An toàn tuyệt đối cho Bank & VNeID)",
                    "Chạy 100% từ PC qua ADB — Không cài APK lạ, không chạy dịch vụ ngầm hay app_process trên điện thoại",
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(serial) = &self.selected_serial {
                        ui_elements::status_pill(ui, &format!("● {serial}"), colors::ACCENT_GREEN);
                    } else {
                        ui_elements::status_pill(ui, "○ Chưa chọn thiết bị ADB", colors::ACCENT_AMBER);
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.small(format!("ADB: {}", self.adb_bin));
                ui.separator();
                ui.small(format!("Themes: {}", self.themes_dir.display()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(self.selected_serial.is_some() && !self.busy, egui::Button::new("Đọc lại thông tin máy"))
                        .clicked()
                    {
                        self.refresh_device_info();
                    }
                });
            });

            if let Some(info) = &self.device_info {
                ui.add_space(2.0);
                ui.small(format!(
                    "Máy: {} {} | OS: {} (Android {} / SDK {}) | Locale: {} | MILLET: {} app | Dev={} | ADB={}",
                    info.brand,
                    info.model,
                    if info.hyperos.is_empty() { "HyperOS/MIUI" } else { &info.hyperos },
                    info.android,
                    info.sdk,
                    if info.system_locales.is_empty() { "(mặc định)" } else { &info.system_locales },
                    info.millet_count,
                    if info.dev_enabled.is_empty() { "?" } else { &info.dev_enabled },
                    if info.adb_enabled.is_empty() { "?" } else { &info.adb_enabled },
                ));
            }
        });
    }

    fn render_tab_notifications(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui.small(
                "Ghi gói vào `MILLET_NO_RESTRICT_APP`, thêm vào Doze whitelist (`dumpsys deviceidle whitelist +`) và cấp `cmd appops set ... RUN_ANY_IN_BACKGROUND allow`.",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::accent_button(ui, "Quét app trên máy", colors::ACCENT_BLUE)
                    })
                    .inner
                    .clicked()
                {
                    self.start_scan_noti();
                }
                if ui.button("Chọn app đã cài").clicked() {
                    for item in &mut self.noti_items {
                        item.selected = item.installed;
                    }
                }
                if ui.button("Bỏ chọn hết").clicked() {
                    for item in &mut self.noti_items {
                        item.selected = false;
                    }
                }
                ui.checkbox(
                    &mut self.send_fcm_ping,
                    "Gửi nhịp tim FCM 1 lần (GTALK/MCS_HEARTBEAT)",
                );
            });

            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("hyperos_noti_scroll")
                .max_height(250.0)
                .show(ui, |ui| {
                    let mut last_cat = String::new();
                    for item in &mut self.noti_items {
                        if item.category != last_cat {
                            if !last_cat.is_empty() {
                                ui.add_space(4.0);
                            }
                            ui.label(
                                egui::RichText::new(format!("▸ {}", item.category))
                                    .color(colors::LOG_INFO)
                                    .strong(),
                            );
                            last_cat = item.category.clone();
                        }
                        ui.horizontal(|ui| {
                            ui.checkbox(
                                &mut item.selected,
                                format!("{}  ({})", item.name, item.package),
                            );
                            if self.noti_scanned {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if item.in_millet {
                                        ui_elements::status_pill(ui, "Trong MILLET", colors::ACCENT_BLUE);
                                    }
                                    if item.installed {
                                        ui_elements::status_pill(ui, "Đã cài", colors::ACCENT_GREEN);
                                    } else {
                                        ui.small("Chưa cài");
                                    }
                                });
                            }
                        });
                    }
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Thêm package khác (ngăn cách dấu phẩy):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_noti_input)
                        .desired_width(f32::INFINITY)
                        .hint_text("vd: com.example.bank, vn.com.mbbank"),
                );
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::accent_button(ui, "Áp dụng Fix Thông báo", colors::ACCENT_GREEN)
                    })
                    .inner
                    .clicked()
                {
                    self.start_apply_noti();
                }
                if ui
                    .add_enabled(can_act, egui::Button::new("Xem MILLET_NO_RESTRICT_APP"))
                    .clicked()
                {
                    self.start_view_millet();
                }
                if ui
                    .add_enabled(can_act, egui::Button::new("Khôi phục MILLET gốc"))
                    .clicked()
                {
                    self.start_restore_millet();
                }
            });
        });
    }

    fn render_tab_locale_theme(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "A. Ngôn ngữ hệ thống / App (system_locales)",
                "Sao lưu ngôn ngữ gốc trước khi đổi sang Tiếng Việt (vi-VN) hoặc khôi phục lại",
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "Hiện tại: {}   |   Bản sao lưu: {}",
                    self.locale_status.current, self.locale_status.backup
                ))
                .strong(),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::accent_button(ui, "Sao lưu & Bật Tiếng Việt (vi-VN)", colors::ACCENT_GREEN)
                    })
                    .inner
                    .clicked()
                {
                    self.start_set_locale("vi-VN");
                }
                if ui.add_enabled(can_act, egui::Button::new("Bật vi-VN,en-US")).clicked() {
                    self.start_set_locale("vi-VN,en-US");
                }
                if ui.add_enabled(can_act, egui::Button::new("Đưa về Tiếng Anh (en-US)")).clicked() {
                    self.start_set_locale("en-US");
                }
                if ui
                    .add_enabled(can_act && self.locale_status.has_backup, egui::Button::new("Khôi phục ngôn ngữ gốc"))
                    .clicked()
                {
                    self.start_restore_locale();
                }
            });
        });

        ui.add_space(6.0);
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "B. Đóng gói & Nạp Theme Việt hóa (.mtz)",
                "Chuyển .mtz sang .mrm/.mrc chuẩn UUIDv3 trên PC, đẩy qua ADB (khớp SHA-1) và mở màn hình Áp dụng thủ công",
            );
            ui.add_space(4.0);

            for (idx, preset) in THEME_CATALOG.iter().enumerate() {
                let exists = self.themes_dir.join(preset.filename).is_file();
                ui.horizontal_wrapped(|ui| {
                    let selected = self.selected_theme_idx == Some(idx);
                    if ui
                        .radio(selected, format!("{}  [{}]", preset.title, preset.filename))
                        .clicked()
                    {
                        self.selected_theme_idx = Some(idx);
                    }
                    if !exists {
                        ui_elements::status_pill(ui, "Thiếu file", colors::ACCENT_RED);
                    }
                    ui.small(format!("— {}", preset.description));
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let custom_sel = self.selected_theme_idx.is_none();
                if ui.radio(custom_sel, "File .mtz ngoài:").clicked() {
                    self.selected_theme_idx = None;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_mtz_path)
                        .desired_width(420.0)
                        .hint_text("Chọn file .mtz trên máy tính..."),
                );
                if ui.button("Chọn file .mtz…").clicked() {
                    self.pick_mtz_file();
                }
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(
                    &mut self.mtz_overwrite,
                    "Ghi đè nếu Theme đã tồn tại trong Chủ đề",
                );
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::accent_button(
                            ui,
                            "Nạp Theme lên máy & Mở màn hình Áp dụng",
                            colors::ACCENT_BLUE,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.start_deploy_mtz();
                }
            });
        });
    }

    fn render_tab_debloat(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui.small(
                "Gỡ cho User 0 (`pm uninstall -k --user 0`, tự fallback `pm disable-user --user 0` nếu ROM chặn gỡ); không động vào gói lõi chống bootloop.",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::accent_button(ui, "Quét App rác trên máy", colors::ACCENT_BLUE)
                    })
                    .inner
                    .clicked()
                {
                    self.confirm_debloat = false;
                    self.start_scan_debloat();
                }
                if ui.button("Chọn gói An toàn đang cài").clicked() {
                    self.confirm_debloat = false;
                    for item in &mut self.debloat_items {
                        item.selected = item.installed && item.risk == "An toàn";
                    }
                }
                if ui.button("Chọn tất cả đã gỡ/tắt").clicked() {
                    self.confirm_debloat = false;
                    for item in &mut self.debloat_items {
                        item.selected = item.restorable;
                    }
                }
                if ui.button("Bỏ chọn hết").clicked() {
                    self.confirm_debloat = false;
                    for item in &mut self.debloat_items {
                        item.selected = false;
                    }
                }
                ui.separator();
                ui.label("Lọc:");
                let prev_filter = self.debloat_filter;
                for f in [DebloatFilter::All, DebloatFilter::Installed, DebloatFilter::Removed] {
                    ui.selectable_value(&mut self.debloat_filter, f, f.label());
                }
                if self.debloat_filter != prev_filter {
                    self.confirm_debloat = false;
                }
            });

            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("hyperos_debloat_scroll")
                .max_height(265.0)
                .show(ui, |ui| {
                    for item in &mut self.debloat_items {
                        if self.debloat_scanned {
                            match self.debloat_filter {
                                DebloatFilter::All => {}
                                DebloatFilter::Installed if !item.installed => continue,
                                DebloatFilter::Removed
                                    if !matches!(
                                        item.status,
                                        DebloatStatus::RemovedUser0 | DebloatStatus::DisabledUser0
                                    ) =>
                                {
                                    continue
                                }
                                _ => {}
                            }
                        }
                        ui_elements::tool_subcard(ui, None, |ui| {
                            ui.horizontal(|ui| {
                                ui.checkbox(
                                    &mut item.selected,
                                    format!("{}  ({})", item.name, item.package),
                                );
                                let risk_color = if item.risk == "An toàn" {
                                    colors::ACCENT_GREEN
                                } else {
                                    colors::ACCENT_AMBER
                                };
                                ui_elements::status_pill(ui, item.risk, risk_color);
                                if self.debloat_scanned {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let st_color = match item.status {
                                            DebloatStatus::Installed => colors::ACCENT_GREEN,
                                            DebloatStatus::DisabledUser0 => colors::ACCENT_AMBER,
                                            DebloatStatus::RemovedUser0 => colors::ACCENT_RED,
                                            DebloatStatus::NotPresent => colors::TEXT_MUTED,
                                        };
                                        ui_elements::status_pill(ui, item.status.label(), st_color);
                                    });
                                }
                            });
                            ui.small(format!("  {}", item.description));
                        });
                        ui.add_space(3.0);
                    }
                });

            ui.add_space(6.0);
            let removable_selected = self
                .debloat_items
                .iter()
                .filter(|i| i.selected && (!self.debloat_scanned || i.installed))
                .count();
            let restorable_selected = self
                .debloat_items
                .iter()
                .filter(|i| i.selected && (!self.debloat_scanned || i.restorable))
                .count();

            ui.horizontal_wrapped(|ui| {
                if self.confirm_debloat {
                    if ui
                        .add_enabled_ui(can_act && removable_selected > 0, |ui| {
                            ui_elements::accent_button(
                                ui,
                                &format!("Xác nhận gỡ/tắt {removable_selected} gói đang cài"),
                                colors::ACCENT_RED,
                            )
                        })
                        .inner
                        .clicked()
                    {
                        self.start_uninstall_debloat();
                    }
                    if ui.button("Hủy").clicked() {
                        self.confirm_debloat = false;
                    }
                } else if ui
                    .add_enabled_ui(can_act && removable_selected > 0, |ui| {
                        ui_elements::accent_button(
                            ui,
                            &format!("Gỡ/Tắt gói đang cài ({removable_selected})"),
                            colors::ACCENT_AMBER,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.confirm_debloat = true;
                }

                if ui
                    .add_enabled(
                        can_act && restorable_selected > 0,
                        egui::Button::new(format!(
                            "Khôi phục gói đã gỡ/tắt ({restorable_selected})"
                        )),
                    )
                    .clicked()
                {
                    self.start_restore_debloat();
                }
            });
        });
    }

    fn render_tab_bank_lock(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "Khóa bảo mật trước khi rút cáp (Bank & VNeID Ready)",
                "Đảm bảo điện thoại hoàn toàn sạch sẽ để vượt kiểm tra bảo mật của VNeID và các ứng dụng Ngân hàng",
            );
            ui.add_space(8.0);

            if let Some(info) = &self.device_info {
                ui.horizontal_wrapped(|ui| {
                    let dev_on = info.dev_enabled == "1";
                    let adb_on = info.adb_enabled == "1";
                    ui_elements::status_pill(
                        ui,
                        if dev_on { "Developer Options: BẬT (1)" } else { "Developer Options: TẮT (0)" },
                        if dev_on { colors::ACCENT_AMBER } else { colors::ACCENT_GREEN },
                    );
                    ui_elements::status_pill(
                        ui,
                        if adb_on { "USB Debugging (ADB): BẬT (1)" } else { "USB Debugging (ADB): TẮT (0)" },
                        if adb_on { colors::ACCENT_AMBER } else { colors::ACCENT_GREEN },
                    );
                });
                ui.add_space(8.0);
            }

            ui_elements::tool_subcard(ui, None, |ui| {
                ui.label("Kiểm tra an toàn:");
                ui.small("  • Không cài đặt bất kỳ APK lạ, module hay tiến trình app_process nào lên điện thoại.");
                ui.small("  • Tất cả file tạm trên /data/local/tmp/ đã được dọn sạch.");
                ui.small("  • Lệnh khóa chạy nguyên tử: `settings put global development_settings_enabled 0 && settings put global adb_enabled 0`.");
                ui.small("  • Sau khi bấm, kết nối ADB với PC sẽ tự ngắt — hãy rút cáp USB và mở thử VNeID / App Ngân hàng.");
            });

            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(can_act, egui::Button::new("Kiểm tra lại trạng thái ADB/Dev"))
                    .clicked()
                {
                    self.refresh_device_info();
                }
                if self.confirm_bank_lock {
                    if ui
                        .add_enabled_ui(can_act, |ui| {
                            ui_elements::accent_button(
                                ui,
                                "Xác nhận TẮT Developer Options & ADB ngay!",
                                colors::ACCENT_RED,
                            )
                        })
                        .inner
                        .clicked()
                    {
                        self.start_bank_lock();
                    }
                    if ui.button("Hủy").clicked() {
                        self.confirm_bank_lock = false;
                    }
                } else if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::accent_button(
                            ui,
                            "Hoàn tất & Khóa bảo mật (Tắt Dev + ADB)",
                            colors::ACCENT_RED,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.confirm_bank_lock = true;
                }
            });
        });
    }

    fn render_log_and_status(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui_elements::tool_card(ui, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                    ui.label(egui::RichText::new(&self.busy_label).color(colors::LOG_INFO));
                } else {
                    ui.colored_label(
                        if self.error { colors::ERROR } else { colors::LOG_INFO },
                        &self.status,
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(i18n::t("common.clear_log")).clicked() {
                        self.log.clear();
                    }
                    if ui.small_button(i18n::t("common.copy_log")).clicked() {
                        ctx.copy_text(self.log.clone());
                    }
                });
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("hyperos_log_scroll")
                .max_height(130.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let text = if self.log.is_empty() {
                        "Nhật ký thao tác sẽ hiện tại đây..."
                    } else {
                        &self.log
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .monospace()
                                .size(12.0)
                                .color(colors::OFF_WHITE),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                });
        });
    }

    // ------------------------------------------------------------------
    // Background Workers
    // ------------------------------------------------------------------
    fn refresh_device_info(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang đọc thông tin thiết bị...".into();
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = hyperos::fetch_device_info(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::DeviceInfoLoaded(serial, res));
        });
    }

    fn start_scan_noti(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang quét ứng dụng trên điện thoại...".into();
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = hyperos::scan_notification_apps(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::NotiScanned(serial, res));
        });
    }

    fn start_apply_noti(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let mut pkgs: Vec<String> = self
            .noti_items
            .iter()
            .filter(|i| i.selected)
            .map(|i| i.package.clone())
            .collect();
        for extra in self.custom_noti_input.split(',') {
            let p = extra.trim();
            if !p.is_empty() {
                pkgs.push(p.to_string());
            }
        }
        if pkgs.is_empty() {
            self.status = "Hãy tick chọn ít nhất 1 ứng dụng trước khi áp dụng.".into();
            self.error = true;
            return;
        }
        self.busy = true;
        self.busy_label = "Đang áp dụng Fix Thông báo (MILLET + Doze + AppOps)...".into();
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let ping = self.send_fcm_ping;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::apply_notification_fix(
                adb.as_deref(),
                &serial,
                &pkgs,
                &backups,
                ping,
                move |line| {
                    let _ = tx_log.send(HyperOsEvent::Log(line));
                },
            );
            let _ = tx.send(HyperOsEvent::NotiApplied(res));
        });
    }

    fn start_view_millet(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang đọc MILLET_NO_RESTRICT_APP...".into();
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let list = hyperos::read_millet_list(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::MilletRead(list));
        });
    }

    fn start_restore_millet(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang khôi phục MILLET_NO_RESTRICT_APP gốc...".into();
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::restore_millet_backup(adb.as_deref(), &serial, &backups, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            let _ = tx.send(HyperOsEvent::MilletRestored(res));
        });
    }

    fn start_set_locale(&mut self, tag: &str) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let tag = tag.to_string();
        self.busy = true;
        self.busy_label = format!("Đang đặt system_locales = {tag}...");
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::set_system_locale(adb.as_deref(), &serial, &tag, &backups, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            let loc_st = hyperos::get_locale_status(adb.as_deref(), &serial, &backups);
            let _ = tx.send(HyperOsEvent::LocaleUpdated(res, loc_st));
        });
    }

    fn start_restore_locale(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang khôi phục ngôn ngữ gốc...".into();
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::restore_locale_backup(adb.as_deref(), &serial, &backups, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            let loc_st = hyperos::get_locale_status(adb.as_deref(), &serial, &backups);
            let _ = tx.send(HyperOsEvent::LocaleUpdated(res, loc_st));
        });
    }

    fn pick_mtz_file(&mut self) {
        self.pending_pickers += 1;
        let initial_dir = self.themes_dir.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let picked = native_pick_mtz_file(&initial_dir);
            let _ = tx.send(HyperOsEvent::PickedMtz(picked));
        });
    }

    fn start_deploy_mtz(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let mtz_path = match self.selected_theme_idx {
            Some(idx) if idx < THEME_CATALOG.len() => {
                self.themes_dir.join(THEME_CATALOG[idx].filename)
            }
            _ => PathBuf::from(self.custom_mtz_path.trim()),
        };
        if !mtz_path.is_file() {
            self.status = format!("Không tìm thấy file MTZ: {}", mtz_path.display());
            self.error = true;
            return;
        }
        let template_path = self.themes_dir.join("metadata_template.json");
        let overwrite = self.mtz_overwrite;
        self.busy = true;
        self.busy_label = "Đang đóng gói .mrm/.mrc & đẩy Theme qua ADB...".into();
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::deploy_mtz_via_adb(
                adb.as_deref(),
                &serial,
                &mtz_path,
                template_path.is_file().then_some(template_path.as_path()),
                overwrite,
                true,
                move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                },
            );
            let _ = tx.send(HyperOsEvent::MtzDeployed(res));
        });
    }

    fn start_scan_debloat(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang quét App rác trên điện thoại...".into();
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = hyperos::scan_debloat_packages(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::DebloatScanned(serial, res));
        });
    }

    fn start_uninstall_debloat(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let pkgs: Vec<String> = self
            .debloat_items
            .iter()
            .filter(|i| i.selected && (!self.debloat_scanned || i.installed))
            .map(|i| i.package.clone())
            .collect();
        if pkgs.is_empty() {
            return;
        }
        self.busy = true;
        self.busy_label = format!("Đang gỡ/tắt {} gói cho User 0...", pkgs.len());
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let (ok, fail) =
                hyperos::uninstall_packages(adb.as_deref(), &serial, &pkgs, &backups, move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                });
            let rescanned = hyperos::scan_debloat_packages(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::DebloatDone {
                action: "Gỡ/Tắt App rác (User 0)",
                ok,
                fail,
                rescanned,
            });
        });
    }

    fn start_restore_debloat(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let pkgs: Vec<String> = self
            .debloat_items
            .iter()
            .filter(|i| i.selected && (!self.debloat_scanned || i.restorable))
            .map(|i| i.package.clone())
            .collect();
        if pkgs.is_empty() {
            return;
        }
        self.busy = true;
        self.busy_label = format!("Đang khôi phục {} gói...", pkgs.len());
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let (ok, fail) =
                hyperos::restore_packages(adb.as_deref(), &serial, &pkgs, &backups, move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                });
            let rescanned = hyperos::scan_debloat_packages(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::DebloatDone {
                action: "Khôi phục app đã gỡ",
                ok,
                fail,
                rescanned,
            });
        });
    }

    fn start_bank_lock(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang tắt Developer Options & USB Debugging...".into();
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::lock_developer_and_adb(adb.as_deref(), &serial, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            let _ = tx.send(HyperOsEvent::BankLocked(res));
        });
    }
}

#[cfg(windows)]
fn native_pick_mtz_file(initial_dir: &PathBuf) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let init_escaped = initial_dir.display().to_string().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $d = New-Object System.Windows.Forms.OpenFileDialog; \
         $d.InitialDirectory = '{init_escaped}'; \
         $d.Filter = 'Xiaomi Theme (*.mtz)|*.mtz|All files (*.*)|*.*'; \
         if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.WriteLine($d.FileName) }}"
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
fn native_pick_mtz_file(_initial_dir: &PathBuf) -> Option<PathBuf> {
    None
}
