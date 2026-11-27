//! "HyperOS China Fix & Việt Hóa" window for optimizing Chinese HyperOS/MIUI
//! phones over PC ADB without installing APKs or background services.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use adb_client::hyperos::{
    self, DEBLOAT_PACKAGES, DebloatOutcome, DebloatReport, DebloatState, DebloatStatus,
    DeepRemovalMode, DeviceHyperOsInfo, LocaleStatus, NOTIFICATION_APPS, NotificationAppState,
    NotificationFixReport, StagedMtz, THEME_CATALOG, ThemeGuardStatus,
};
use adb_client::hyperos_vi::{
    self, DeviceViProfile, DeviceViState, HealthCheckReport, MtzValidationReport,
    RevertDiffReport, ThemeLocaleSnapshot, TranslationDiff, TranslationMemoryReport,
    ViBackupRecord,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;

use crate::audit;
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
enum ViSubTab {
    ApplyAndGuard,
    DiagnosticsAndHealth,
    BuildAndTranslation,
    BackupAndDryRun,
}

impl ViSubTab {
    fn label(self) -> &'static str {
        match self {
            ViSubTab::ApplyAndGuard => "Áp dụng & Khóa DRM",
            ViSubTab::DiagnosticsAndHealth => "Chẩn đoán & Sức khỏe",
            ViSubTab::BuildAndTranslation => "Pipeline MTZ & Bản dịch",
            ViSubTab::BackupAndDryRun => "Backup, Rollback & Dry-Run",
        }
    }
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
            DebloatFilter::Removed => "Đã gỡ / tắt / đóng băng",
        }
    }
}

enum HyperOsEvent {
    Log(String),
    DeviceInfoLoaded(
        String,
        Result<(DeviceHyperOsInfo, LocaleStatus, ThemeGuardStatus), String>,
    ),
    NotiScanned(String, Result<Vec<NotificationAppState>, String>),
    NotiApplied(Result<NotificationFixReport, String>),
    MilletRead(Vec<String>),
    MilletRestored(Result<String, String>),
    LocaleUpdated(Result<String, String>, LocaleStatus),
    PickedMtz(Option<PathBuf>),
    MtzDeployed(
        Result<StagedMtz, String>,
        Option<ThemeGuardStatus>,
        Option<DeviceViState>,
    ),
    ThemeGuardUpdated(Result<String, String>, ThemeGuardStatus),
    ProfileLoaded(String, Result<DeviceViProfile, String>),
    HealthChecked {
        serial: String,
        silent: bool,
        res: Result<HealthCheckReport, String>,
    },
    SnapshotCaptured(Result<(ThemeLocaleSnapshot, PathBuf), String>, Vec<(PathBuf, ThemeLocaleSnapshot)>),
    MtzValidated(Result<MtzValidationReport, String>),
    TranslationProcessed(Result<(TranslationMemoryReport, Option<TranslationDiff>), String>),
    ViBackupCreated(Result<ViBackupRecord, String>, Vec<ViBackupRecord>),
    ViRollbackDone(Result<String, String>, Option<HealthCheckReport>),
    ReapplyDone(Result<String, String>, Option<HealthCheckReport>),
    MagiskExported(Result<PathBuf, String>),
    DebloatScanned(String, Result<Vec<DebloatState>, String>),
    DebloatUninstallDone {
        report: DebloatReport,
        rescanned: Result<Vec<DebloatState>, String>,
    },
    DebloatDone {
        action: &'static str,
        ok: usize,
        fail: usize,
        rescanned: Result<Vec<DebloatState>, String>,
    },
    BankLocked(Result<String, String>),
}

pub struct HyperOsDialog {
    adb_bin: String,
    selected_serial: Option<String>,
    active_tab: HyperOsTab,

    device_info: Option<DeviceHyperOsInfo>,
    noti_items: Vec<NotificationAppState>,
    noti_scanned: bool,
    custom_noti_input: String,
    send_fcm_ping: bool,

    locale_status: LocaleStatus,
    theme_guard: ThemeGuardStatus,
    freeze_theme_manager: bool,
    themes_dir: PathBuf,
    selected_theme_idx: Option<usize>, // None = custom path
    custom_mtz_path: String,
    mtz_overwrite: bool,

    vi_sub_tab: ViSubTab,
    vi_profile: Option<DeviceViProfile>,
    vi_state: DeviceViState,
    health_report: Option<HealthCheckReport>,
    auto_health_check: bool,
    last_auto_health_at: Option<Instant>,
    snapshots: Vec<(PathBuf, ThemeLocaleSnapshot)>,
    revert_diff: Option<RevertDiffReport>,
    mtz_validation: Option<MtzValidationReport>,
    diff_old_theme_idx: usize,
    tm_report: Option<TranslationMemoryReport>,
    tm_diff: Option<TranslationDiff>,
    vi_backups: Vec<ViBackupRecord>,
    dry_run: bool,
    dry_run_preview: Vec<String>,

    debloat_items: Vec<DebloatState>,
    debloat_scanned: bool,
    debloat_filter: DebloatFilter,
    debloat_mode: DeepRemovalMode,
    debloat_search: String,
    custom_debloat_pkg: String,
    debloat_report: Option<DebloatReport>,
    confirm_debloat: bool,

    confirm_bank_lock: bool,

    noti_applied: bool,
    locale_applied: bool,
    theme_deployed: bool,
    debloat_applied: bool,
    bank_locked: bool,

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
                protected_reason: None,
                selected: d.default_selected,
            })
            .collect();

        Self {
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
            theme_guard: ThemeGuardStatus {
                state_label: "(chưa kiểm tra)".into(),
                ..Default::default()
            },
            freeze_theme_manager: true,
            themes_dir,
            selected_theme_idx: Some(0),
            custom_mtz_path: String::new(),
            mtz_overwrite: false,
            vi_sub_tab: ViSubTab::ApplyAndGuard,
            vi_profile: None,
            vi_state: DeviceViState::default(),
            health_report: None,
            auto_health_check: true,
            last_auto_health_at: None,
            snapshots: Vec::new(),
            revert_diff: None,
            mtz_validation: None,
            diff_old_theme_idx: 3.min(THEME_CATALOG.len().saturating_sub(1)),
            tm_report: None,
            tm_diff: None,
            vi_backups: Vec::new(),
            dry_run: false,
            dry_run_preview: Vec::new(),
            debloat_items,
            debloat_scanned: false,
            debloat_filter: DebloatFilter::All,
            debloat_mode: DeepRemovalMode::AutoDeep,
            debloat_search: String::new(),
            custom_debloat_pkg: String::new(),
            debloat_report: None,
            confirm_debloat: false,
            confirm_bank_lock: false,
            noti_applied: false,
            locale_applied: false,
            theme_deployed: false,
            debloat_applied: false,
            bank_locked: false,
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
            self.debloat_report = None;
            self.confirm_debloat = false;
            self.confirm_bank_lock = false;
            self.noti_applied = false;
            self.locale_applied = false;
            self.theme_deployed = false;
            self.debloat_applied = false;
            self.bank_locked = false;
            self.locale_status = LocaleStatus {
                current: "(chưa đọc)".into(),
                backup: "(chưa sao lưu)".into(),
                has_backup: false,
            };
            self.theme_guard = ThemeGuardStatus {
                state_label: "(chưa kiểm tra)".into(),
                ..Default::default()
            };
            self.vi_profile = None;
            self.health_report = None;
            self.last_auto_health_at = None;
            self.revert_diff = None;
            self.dry_run_preview.clear();
            if let Some(ref s) = next {
                self.vi_state = hyperos_vi::load_device_vi_state(s);
                self.snapshots = hyperos_vi::list_snapshots(s);
                self.vi_backups = hyperos_vi::list_vi_backups(s);
            } else {
                self.vi_state = DeviceViState::default();
                self.snapshots.clear();
                self.vi_backups.clear();
            }
            if next.is_some() && !self.busy {
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

    /// Tên máy "Hãng Model" nếu đã đọc được thông tin thiết bị — đính kèm
    /// vào audit log để file theo serial vẫn đọc hiểu được bằng mắt.
    fn device_model(&self) -> Option<String> {
        self.device_info
            .as_ref()
            .map(|i| format!("{} {}", i.brand, i.model))
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
                            Ok((info, loc_st, guard_st)) => {
                                self.locale_status = loc_st;
                                self.locale_applied =
                                    self.locale_status.current.to_ascii_lowercase().contains("vi");
                                self.theme_guard = guard_st;
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
                            self.noti_applied = true;
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
                            self.locale_applied =
                                self.locale_status.current.to_ascii_lowercase().contains("vi");
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
                HyperOsEvent::MtzDeployed(res, guard_opt, state_opt) => {
                    self.busy = false;
                    if let Some(guard) = guard_opt {
                        self.theme_guard = guard;
                    }
                    if let Some(st) = state_opt {
                        self.vi_state = st;
                    }
                    if let Some(ref s) = self.selected_serial {
                        self.vi_backups = hyperos_vi::list_vi_backups(s);
                    }
                    match res {
                        Ok(staged) => {
                            self.theme_deployed = true;
                            self.status = format!(
                                "Đã nạp \"{}\" (UUID {}). Hãy bấm [Áp dụng] trên điện thoại rồi khóa DRM!",
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
                HyperOsEvent::ThemeGuardUpdated(res, guard) => {
                    self.busy = false;
                    self.theme_guard = guard;
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
                HyperOsEvent::ProfileLoaded(serial, res) => {
                    self.busy = false;
                    if self.selected_serial.as_deref() == Some(serial.as_str()) {
                        match res {
                            Ok(prof) => {
                                self.status = format!(
                                    "Đã nhận diện Device Profile: {} ({}, {}) -> {}",
                                    prof.model,
                                    prof.region,
                                    if prof.has_magisk || prof.has_su {
                                        "Có Root"
                                    } else {
                                        "Không Root"
                                    },
                                    prof.recommended_strategy.label()
                                );
                                self.error = false;
                                self.vi_profile = Some(prof);
                            }
                            Err(e) => {
                                self.status = e;
                                self.error = true;
                            }
                        }
                    }
                }
                HyperOsEvent::HealthChecked {
                    serial,
                    silent,
                    res,
                } => {
                    if !silent {
                        self.busy = false;
                    }
                    if self.selected_serial.as_deref() == Some(serial.as_str()) {
                        match res {
                            Ok(rep) => {
                                self.theme_guard = rep.theme_guard.clone();
                                self.vi_state = rep.state.clone();
                                if rep.reverted {
                                    self.status = format!(
                                        "CẢNH BÁO REVERT: {}",
                                        rep.reasons.join(" | ")
                                    );
                                    self.error = true;
                                    self.append_log(&self.status.clone());
                                } else if !silent {
                                    self.status = format!(
                                        "Health Check OK: locale={}, overlay={} tệp.",
                                        rep.current_system_locales, rep.active_overlay_count
                                    );
                                    self.error = false;
                                }
                                self.health_report = Some(rep);
                            }
                            Err(e) if !silent => {
                                self.status = e;
                                self.error = true;
                            }
                            Err(_) => {}
                        }
                    }
                }
                HyperOsEvent::SnapshotCaptured(res, list) => {
                    self.busy = false;
                    self.snapshots = list;
                    match res {
                        Ok((snap, _path)) => {
                            self.status = format!(
                                "Đã chụp snapshot '{}' ({} overlay, locale={}).",
                                snap.label,
                                snap.overlay_files.len(),
                                snap.system_locales
                            );
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
                    }
                }
                HyperOsEvent::MtzValidated(res) => {
                    self.busy = false;
                    match res {
                        Ok(rep) => {
                            self.status = if rep.ok {
                                format!(
                                    "MTZ '{}' đạt 100% kiểm tra theme.toml ({} module, author='{}').",
                                    rep.title, rep.module_count, rep.author
                                )
                            } else {
                                format!("MTZ '{}' chưa đạt kiểm tra theme.toml!", rep.title)
                            };
                            self.error = !rep.ok;
                            self.mtz_validation = Some(rep);
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
                    }
                }
                HyperOsEvent::TranslationProcessed(res) => {
                    self.busy = false;
                    match res {
                        Ok((tm, diff_opt)) => {
                            self.status = format!(
                                "Đã cập nhật Translation Memory: {} app, {} khóa (tổng TM: {}).",
                                tm.total_apps, tm.total_keys, tm.memory_total_keys
                            );
                            self.error = false;
                            self.tm_report = Some(tm);
                            if diff_opt.is_some() {
                                self.tm_diff = diff_opt;
                            }
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
                    }
                }
                HyperOsEvent::ViBackupCreated(res, list) => {
                    self.busy = false;
                    self.vi_backups = list;
                    match res {
                        Ok(rec) => {
                            self.status = format!(
                                "Đã tạo backup '{}' ({} tệp .mrm, locale='{}').",
                                rec.id,
                                rec.deployed_mrm_files.len(),
                                rec.system_locales
                            );
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
                    }
                }
                HyperOsEvent::ViRollbackDone(res, health_opt) => {
                    self.busy = false;
                    if let Some(rep) = health_opt {
                        self.theme_guard = rep.theme_guard.clone();
                        self.vi_state = rep.state.clone();
                        self.health_report = Some(rep);
                    }
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
                HyperOsEvent::ReapplyDone(res, health_opt) => {
                    self.busy = false;
                    if let Some(rep) = health_opt {
                        self.theme_guard = rep.theme_guard.clone();
                        self.vi_state = rep.state.clone();
                        self.health_report = Some(rep);
                    }
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
                HyperOsEvent::MagiskExported(res) => {
                    self.busy = false;
                    match res {
                        Ok(path) => {
                            self.status = format!(
                                "Đã xuất Magisk Module Việt hóa: {}",
                                path.display()
                            );
                            self.error = false;
                        }
                        Err(e) => {
                            self.status = e;
                            self.error = true;
                        }
                    }
                }
                HyperOsEvent::DebloatScanned(serial, res) => {
                    self.busy = false;
                    if self.selected_serial.as_deref() == Some(serial.as_str()) {
                        match res {
                            Ok(rows) => {
                                let inst = rows.iter().filter(|r| r.installed).count();
                                let rem = rows.iter().filter(|r| r.restorable).count();
                                self.status = format!(
                                    "Quét Debloat xong: {inst} đang cài, {rem} đã gỡ/tắt/đóng băng."
                                );
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
                HyperOsEvent::DebloatUninstallDone { report, rescanned } => {
                    self.busy = false;
                    self.confirm_debloat = false;
                    if report.ok_total() > 0 {
                        self.debloat_applied = true;
                    }
                    self.status = format!(
                        "Hoàn tất: gỡ {}, tắt {}, đóng băng {}, đã gỡ sẵn {}, bỏ qua bảo vệ {}, lỗi {}",
                        report.uninstalled,
                        report.disabled,
                        report.frozen,
                        report.already_removed,
                        report.skipped_protected,
                        report.failed
                    );
                    self.error = report.ok_total() == 0 && report.failed > 0;
                    self.debloat_report = Some(report);
                    if let Ok(rows) = rescanned {
                        self.debloat_items = rows;
                        self.debloat_scanned = true;
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
                            self.bank_locked = true;
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

    /// Renders the HyperOS tools inside the HyperOS main tab.
    pub fn show_in(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.device_info.is_none() && self.selected_serial.is_some() && !self.busy {
            self.refresh_device_info();
        }

        self.render_header(ui);
        ui.add_space(6.0);

        let steps = [
            ui_elements::StepItem {
                number: 1,
                title: i18n::t("hyperos.tab_notifications"),
                subtitle: if self.noti_applied {
                    "Đã cấu hình MILLET & Doze"
                } else {
                    "Quét & giữ app chạy nền"
                },
                state: if self.noti_applied {
                    ui_elements::StepState::Done
                } else if self.active_tab == HyperOsTab::Notifications {
                    ui_elements::StepState::Active
                } else {
                    ui_elements::StepState::Pending
                },
            },
            ui_elements::StepItem {
                number: 2,
                title: i18n::t("hyperos.tab_locale_theme"),
                subtitle: if self.locale_applied && self.theme_deployed {
                    "Đã bật vi-VN & nạp MTZ"
                } else {
                    "Ngôn ngữ vi-VN & Theme"
                },
                state: if self.locale_applied && self.theme_deployed {
                    ui_elements::StepState::Done
                } else if self.active_tab == HyperOsTab::LocaleAndTheme {
                    ui_elements::StepState::Active
                } else if self.noti_applied && self.active_tab == HyperOsTab::Notifications {
                    ui_elements::StepState::Next
                } else {
                    ui_elements::StepState::Pending
                },
            },
            ui_elements::StepItem {
                number: 3,
                title: i18n::t("hyperos.tab_debloat"),
                subtitle: if self.debloat_applied {
                    "Đã dọn app rác TQ"
                } else {
                    "Gỡ / đóng băng rác & QC"
                },
                state: if self.debloat_applied {
                    ui_elements::StepState::Done
                } else if self.active_tab == HyperOsTab::Debloat {
                    ui_elements::StepState::Active
                } else if self.theme_deployed && self.active_tab == HyperOsTab::LocaleAndTheme {
                    ui_elements::StepState::Next
                } else {
                    ui_elements::StepState::Pending
                },
            },
            ui_elements::StepItem {
                number: 4,
                title: i18n::t("hyperos.tab_bank_lock"),
                subtitle: if self.bank_locked {
                    "Sẵn sàng Bank & VNeID"
                } else {
                    "Tắt Dev + USB Debug"
                },
                state: if self.bank_locked {
                    ui_elements::StepState::Done
                } else if self.active_tab == HyperOsTab::BankLock {
                    ui_elements::StepState::Active
                } else if self.debloat_applied && self.active_tab == HyperOsTab::Debloat {
                    ui_elements::StepState::Next
                } else {
                    ui_elements::StepState::Pending
                },
            },
        ];
        if let Some(idx) = ui_elements::workflow_stepper(ui, &steps) {
            self.active_tab = match idx {
                0 => HyperOsTab::Notifications,
                1 => HyperOsTab::LocaleAndTheme,
                2 => HyperOsTab::Debloat,
                _ => HyperOsTab::BankLock,
            };
        }
        ui.add_space(6.0);

        if self.busy {
            ui_elements::busy_banner(ui, &self.busy_label);
            ui.add_space(6.0);
        }

        let has_device = self.selected_serial.is_some();
        let can_act = has_device && !self.busy;

        if self.auto_health_check
            && can_act
            && self.active_tab == HyperOsTab::LocaleAndTheme
            && self
                .last_auto_health_at
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
        {
            self.start_health_check(true);
        }

        match self.active_tab {
            HyperOsTab::Notifications => self.render_tab_notifications(ui, can_act),
            HyperOsTab::LocaleAndTheme => self.render_tab_locale_theme(ui, can_act),
            HyperOsTab::Debloat => self.render_tab_debloat(ui, can_act),
            HyperOsTab::BankLock => self.render_tab_bank_lock(ui, can_act),
        }

        ui.add_space(6.0);
        self.render_log_and_status(ui, ctx);

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
                        ui_elements::pulsing_cta_button(
                            ui,
                            "Quét app trên máy",
                            colors::ACCENT_BLUE,
                            can_act && !self.noti_scanned,
                        )
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
                        ui_elements::pulsing_cta_button(
                            ui,
                            "Áp dụng Fix Thông báo",
                            colors::ACCENT_GREEN,
                            can_act && self.noti_scanned && !self.noti_applied,
                        )
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
                if self.noti_applied {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui_elements::pulsing_cta_button(
                            ui,
                            "Tiếp theo: 2. Việt hóa & Theme MTZ →",
                            colors::ACCENT_CYAN,
                            true,
                        )
                        .clicked()
                        {
                            self.active_tab = HyperOsTab::LocaleAndTheme;
                        }
                    });
                }
            });
        });
    }

    fn selected_mtz_path(&self) -> PathBuf {
        match self.selected_theme_idx {
            Some(idx) if idx < THEME_CATALOG.len() => {
                self.themes_dir.join(THEME_CATALOG[idx].filename)
            }
            _ => PathBuf::from(self.custom_mtz_path.trim()),
        }
    }

    fn render_tab_locale_theme(&mut self, ui: &mut egui::Ui, can_act: bool) {
        self.render_vi_status_panel(ui, can_act);
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            for tab in [
                ViSubTab::ApplyAndGuard,
                ViSubTab::DiagnosticsAndHealth,
                ViSubTab::BuildAndTranslation,
                ViSubTab::BackupAndDryRun,
            ] {
                ui.selectable_value(&mut self.vi_sub_tab, tab, tab.label());
            }
        });
        ui.add_space(6.0);

        match self.vi_sub_tab {
            ViSubTab::ApplyAndGuard => self.render_vi_subtab_apply(ui, can_act),
            ViSubTab::DiagnosticsAndHealth => self.render_vi_subtab_diagnostics(ui, can_act),
            ViSubTab::BuildAndTranslation => self.render_vi_subtab_build_translation(ui),
            ViSubTab::BackupAndDryRun => self.render_vi_subtab_backup_dryrun(ui, can_act),
        }
    }

    fn render_vi_status_panel(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui_elements::section_header(
                    ui,
                    "Bảng Trạng Thái Việt Hóa & Chiến Lược Tự Động",
                    "Theo dõi Theme đang chạy, Locale, Checksum overlay, Lịch sử revert & chế độ Dry-Run",
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.dry_run, "Dry-Run (chỉ in lệnh ADB)");
                    if ui
                        .add_enabled(can_act, egui::Button::new("Dò Profile & Health"))
                        .clicked()
                    {
                        self.start_detect_profile();
                    }
                });
            });
            ui.add_space(4.0);

            if let Some(ref prof) = self.vi_profile {
                ui.horizontal_wrapped(|ui| {
                    ui_elements::status_pill(
                        ui,
                        &format!("ROM: {}", prof.rom_version),
                        colors::ACCENT_BLUE,
                    );
                    ui_elements::status_pill(
                        ui,
                        &format!("Region: {} ({})", prof.region, prof.hwc),
                        colors::ACCENT_CYAN,
                    );
                    let bl_txt = match prof.bootloader_locked {
                        Some(true) => "Bootloader: Locked",
                        Some(false) => "Bootloader: Unlocked",
                        None => "Bootloader: ?",
                    };
                    ui_elements::status_pill(ui, bl_txt, colors::ACCENT_AMBER);
                    let has_root = prof.has_magisk || prof.has_su;
                    ui_elements::status_pill(
                        ui,
                        if has_root {
                            "Root / Magisk: Có"
                        } else {
                            "Root: Không"
                        },
                        if has_root {
                            colors::ACCENT_GREEN
                        } else {
                            colors::TEXT_MUTED
                        },
                    );
                });
                ui.small(format!(
                    "▸ Phương án đề xuất: {} — {}",
                    prof.recommended_strategy.label(),
                    prof.strategy_reason
                ));
            } else {
                ui.small("Chưa quét Device Profile chi tiết (bấm [Dò Profile & Health] để tự chọn luồng Root / Non-Root).");
            }

            ui.add_space(4.0);
            let theme_name = if self.vi_state.last_theme_title.is_empty() {
                "(chưa áp qua tool)".to_string()
            } else {
                self.vi_state.last_theme_title.clone()
            };
            let persist_loc = self
                .health_report
                .as_ref()
                .map(|r| r.current_persist_locale.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("en-US");
            ui.small(format!(
                "Theme gần nhất: {} | system_locales: {} (persist.sys.locale: {}) | Overlay: {} tệp | Số lần revert ghi nhận: {}",
                theme_name,
                self.locale_status.current,
                persist_loc,
                self.theme_guard.active_overlay_count,
                self.vi_state.revert_history.len(),
            ));

            if let Some(reasons) = self
                .health_report
                .as_ref()
                .filter(|r| r.reverted)
                .map(|r| r.reasons.clone())
            {
                let mut do_reapply = false;
                let mut do_snapshot = false;
                ui.add_space(6.0);
                ui_elements::tool_subcard(ui, Some(colors::ACCENT_RED), |ui| {
                    ui.label(
                        egui::RichText::new("⚠ PHÁT HIỆN BỊ REVERT VỀ TIẾNG ANH / GỐC!")
                            .color(colors::ACCENT_RED)
                            .strong(),
                    );
                    for reason in &reasons {
                        ui.small(format!("  • {reason}"));
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled_ui(can_act, |ui| {
                                ui_elements::pulsing_cta_button(
                                    ui,
                                    "Áp lại Việt hóa ngay (1 nút)",
                                    colors::ACCENT_RED,
                                    true,
                                )
                            })
                            .inner
                            .clicked()
                        {
                            do_reapply = true;
                        }
                        if ui
                            .add_enabled(
                                can_act,
                                egui::Button::new("Chụp Snapshot & Logcat chẩn đoán"),
                            )
                            .clicked()
                        {
                            do_snapshot = true;
                        }
                    });
                });
                if do_reapply {
                    self.start_one_click_reapply();
                } else if do_snapshot {
                    self.start_capture_snapshot("after_revert");
                }
            }
        });
    }

    fn render_vi_subtab_apply(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "A. Ngôn ngữ hệ thống / App (system_locales + cmd locale)",
                "Sao lưu ngôn ngữ gốc trước khi bật Tiếng Việt (vi-VN,en-US) và khóa ngôn ngữ bền vững cho từng ứng dụng",
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "Hiện tại: {}   |   Bản sao lưu: {}",
                    self.locale_status.current, self.locale_status.backup
                ))
                .strong(),
            );
            ui.small("Bật vi-VN,en-US đồng thời khóa Tiếng Việt bền vững cho ứng dụng (Android 13+) và tránh rơi về chữ Trung.");
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::pulsing_cta_button(
                            ui,
                            "Sao lưu & Bật Tiếng Việt (vi-VN,en-US)",
                            colors::ACCENT_GREEN,
                            can_act && !self.locale_applied,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.start_set_locale("vi-VN,en-US");
                }
                if ui.add_enabled(can_act, egui::Button::new("Chỉ vi-VN")).clicked() {
                    self.start_set_locale("vi-VN");
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
                "B. Đóng gói & Nạp Theme Việt hóa (.mtz) / Magisk Overlay",
                "Đọc metadata từ themes/theme.toml, tự mở lại Chủ đề nếu đang khóa, đẩy .mrm/.mrc khớp SHA-1",
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
                        ui_elements::pulsing_cta_button(
                            ui,
                            "Nạp Theme lên máy & Mở màn hình Áp dụng",
                            colors::ACCENT_BLUE,
                            can_act && self.locale_applied && !self.theme_deployed,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.start_deploy_mtz();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Xuất Magisk Module (.zip)"))
                    .clicked()
                {
                    self.start_export_magisk_module();
                }
            });
        });

        ui.add_space(6.0);
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "C. Chống tự quay về Tiếng Anh (Khóa DRM Chủ đề)",
                "Chặn com.android.thememanager chạy ngầm kiểm tra DRM (nguyên nhân khiến Theme Việt hóa bị reset về gốc sau 30–60 phút)",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let guard_color = if self.theme_guard.locked {
                    colors::ACCENT_GREEN
                } else {
                    colors::ACCENT_AMBER
                };
                ui_elements::status_pill(ui, &self.theme_guard.state_label, guard_color);
                let ovl_color = if self.theme_guard.active_overlay_count > 0 {
                    colors::ACCENT_GREEN
                } else {
                    colors::ACCENT_AMBER
                };
                ui_elements::status_pill(
                    ui,
                    &format!(
                        "Overlay đang áp dụng: {} tệp",
                        self.theme_guard.active_overlay_count
                    ),
                    ovl_color,
                );
            });
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.freeze_theme_manager,
                "Đóng băng app Chủ đề (khuyên dùng — chống hoàn tác 100%, mở lại 1 chạm)",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::pulsing_cta_button(
                            ui,
                            "Khóa chống quay về Tiếng Anh (sau khi bấm Áp dụng)",
                            colors::ACCENT_GREEN,
                            can_act && self.theme_deployed && !self.theme_guard.locked,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.start_lock_theme_guard();
                }
                if ui
                    .add_enabled(
                        can_act,
                        egui::Button::new("Mở lại app Chủ đề (khi cần đổi theme/hình nền)"),
                    )
                    .clicked()
                {
                    self.start_unlock_theme_guard();
                }
                if self.theme_deployed || self.theme_guard.locked {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui_elements::pulsing_cta_button(
                            ui,
                            "Tiếp theo: 3. Gỡ App rác TQ →",
                            colors::ACCENT_CYAN,
                            self.theme_guard.locked,
                        )
                        .clicked()
                        {
                            self.active_tab = HyperOsTab::Debloat;
                        }
                    });
                }
            });
        });
    }

    fn render_vi_subtab_diagnostics(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "Chẩn đoán Logcat ThemeManager, Snapshot & Health Check",
                "Ghi logcat com.android.thememanager + snapshot /data/system/theme/ + dumpsys locale để tìm đúng nguyên nhân revert",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(can_act, egui::Button::new("Kiểm tra sức khỏe ngay (Health Check)"))
                    .clicked()
                {
                    self.start_health_check(false);
                }
                ui.checkbox(&mut self.auto_health_check, "Tự kiểm tra mỗi 60s");
                if ui
                    .add_enabled(can_act, egui::Button::new("Chụp Snapshot TRƯỚC revert"))
                    .clicked()
                {
                    self.start_capture_snapshot("before_revert");
                }
                if ui
                    .add_enabled(can_act, egui::Button::new("Chụp Snapshot SAU revert"))
                    .clicked()
                {
                    self.start_capture_snapshot("after_revert");
                }
                if ui
                    .add_enabled(self.snapshots.len() >= 2, egui::Button::new("So sánh 2 Snapshot gần nhất"))
                    .clicked()
                {
                    let (_, after) = &self.snapshots[0];
                    let (_, before) = &self.snapshots[1];
                    self.revert_diff = Some(hyperos_vi::compare_snapshots(before, after));
                }
            });

            if let Some(ref diff) = self.revert_diff {
                ui.add_space(6.0);
                ui_elements::tool_subcard(ui, Some(colors::ACCENT_AMBER), |ui| {
                    ui.label(egui::RichText::new(&diff.summary).strong());
                    if !diff.missing_overlays.is_empty() {
                        ui.small(format!("Overlay bị mất: {}", diff.missing_overlays.join(", ")));
                    }
                    if !diff.changed_overlays.is_empty() {
                        ui.small(format!("Overlay bị đổi checksum: {}", diff.changed_overlays.join(", ")));
                    }
                });
            }

            if !self.snapshots.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(format!("Snapshot đã lưu ({}):", self.snapshots.len())).strong());
                egui::ScrollArea::vertical()
                    .id_salt("vi_snapshots_scroll")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for (path, snap) in self.snapshots.iter().take(10) {
                            ui.small(format!(
                                "• [{}] {} | locale={} | persist={} | overlay={} tệp | logcat {} dòng ({})",
                                snap.captured_at_epoch,
                                snap.label,
                                snap.system_locales,
                                snap.persist_sys_locale,
                                snap.overlay_files.len(),
                                snap.logcat_excerpt.lines().count(),
                                path.file_name().and_then(|n| n.to_str()).unwrap_or("")
                            ));
                        }
                    });
            }

            if !self.vi_state.revert_history.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Lịch sử phát hiện Revert:").strong());
                for entry in self.vi_state.revert_history.iter().take(8) {
                    ui.small(format!(
                        "• [{}] {} (locale: {} -> {}; overlay: {} -> {})",
                        entry.detected_at_epoch,
                        entry.reason,
                        entry.locale_before,
                        entry.locale_after,
                        entry.overlay_count_before,
                        entry.overlay_count_after
                    ));
                }
            }
        });
    }

    fn render_vi_subtab_build_translation(&mut self, ui: &mut egui::Ui) {
        let mtz_path = self.selected_mtz_path();
        let stem = mtz_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let cfg = hyperos_vi::load_theme_config(Some(&self.themes_dir), stem);

        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "Pipeline MTZ Chuẩn Hóa (theme.toml) & Quản Lý Bản Dịch (Translation Memory)",
                "Validate cấu trúc MTZ/XML từ 1 file theme.toml duy nhất; trích chuỗi, diff 2 phiên bản và thống kê độ phủ từng app",
            );
            ui.add_space(4.0);
            ui.small(format!(
                "Cấu hình đang áp cho '{}': title='{}', author='{}', designer='{}', version={}, uiVersion={}, isThirdParties={}",
                mtz_path.file_name().and_then(|n| n.to_str()).unwrap_or("-"),
                cfg.title,
                cfg.author,
                cfg.designer,
                cfg.version,
                cfg.ui_version,
                cfg.is_third_parties
            ));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Validate MTZ theo theme.toml"))
                    .clicked()
                {
                    self.start_validate_mtz(false);
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Rebuild description.xml từ theme.toml"))
                    .clicked()
                {
                    self.start_validate_mtz(true);
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Trích chuỗi & Cập nhật Translation Memory"))
                    .clicked()
                {
                    self.start_extract_and_score_tm(false);
                }
                ui.label("So sánh với:");
                let mut cur_name = THEME_CATALOG
                    .get(self.diff_old_theme_idx)
                    .map(|p| p.filename)
                    .unwrap_or("MTZOS4_fold.mtz");
                egui::ComboBox::from_id_salt("vi_diff_base_mtz")
                    .selected_text(cur_name)
                    .show_ui(ui, |ui| {
                        for (idx, preset) in THEME_CATALOG.iter().enumerate() {
                            if ui
                                .selectable_value(&mut self.diff_old_theme_idx, idx, preset.filename)
                                .clicked()
                            {
                                cur_name = preset.filename;
                            }
                        }
                    });
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Diff chuỗi giữa 2 bản MTZ"))
                    .clicked()
                {
                    self.start_extract_and_score_tm(true);
                }
            });

            if let Some(ref rep) = self.mtz_validation {
                ui.add_space(6.0);
                ui_elements::tool_subcard(
                    ui,
                    Some(if rep.ok { colors::ACCENT_GREEN } else { colors::ACCENT_RED }),
                    |ui| {
                        for chk in &rep.checks {
                            ui.horizontal_wrapped(|ui| {
                                ui_elements::status_pill(
                                    ui,
                                    if chk.passed { "ĐẠT" } else { "LỖI" },
                                    if chk.passed { colors::ACCENT_GREEN } else { colors::ACCENT_RED },
                                );
                                ui.small(format!("{}: {}", chk.name, chk.detail));
                            });
                        }
                    },
                );
            }

            if let Some(ref diff) = self.tm_diff {
                ui.add_space(6.0);
                ui_elements::tool_subcard(ui, Some(colors::ACCENT_CYAN), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Diff chuỗi giữa 2 bản: +{} khóa mới | ~{} khóa đổi | -{} khóa bỏ ({} app)",
                            diff.total_added,
                            diff.total_changed,
                            diff.total_removed,
                            diff.apps.len()
                        ))
                        .strong(),
                    );
                    for a in diff.apps.iter().filter(|x| x.added > 0 || x.changed > 0 || x.removed > 0) {
                        ui.small(format!(
                            "  • {}: +{} mới, ~{} đổi, -{} bỏ, ={} giữ nguyên",
                            a.app, a.added, a.changed, a.removed, a.unchanged
                        ));
                    }
                });
            }

            if let Some(ref tm) = self.tm_report {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Translation Memory: {} app | {} khóa ({} khóa lưu lâu dài tại {})",
                        tm.total_apps, tm.total_keys, tm.memory_total_keys, tm.storage_path
                    ))
                    .strong(),
                );
                egui::ScrollArea::vertical()
                    .id_salt("vi_tm_cov_scroll")
                    .max_height(130.0)
                    .show(ui, |ui| {
                        for row in &tm.per_app {
                            ui.small(format!(
                                "• {}: {}/{} khóa đã dịch ({:.1}%)",
                                row.app, row.translated_keys, row.total_keys, row.coverage_pct
                            ));
                        }
                    });
            }
        });
    }

    fn render_vi_subtab_backup_dryrun(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(
                ui,
                "Backup Trạng Thái, Rollback 1 Nút & Xem Trước Lệnh ADB (Dry-Run)",
                "Sao lưu settings + theme manager + .mrm trước mỗi lần áp và khôi phục nhanh nếu cần",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(can_act, egui::Button::new("Tạo Backup trạng thái ngay"))
                    .clicked()
                {
                    self.start_create_vi_backup();
                }
                if ui.button("Xem lệnh Dry-Run: Luồng đầy đủ (Reapply)").clicked() {
                    let serial = self.selected_serial.as_deref().unwrap_or("<serial>");
                    let mtz = self.selected_mtz_path();
                    let name = mtz.file_name().and_then(|n| n.to_str());
                    self.dry_run_preview = hyperos_vi::plan_dry_run_commands(
                        "reapply",
                        serial,
                        "vi-VN,en-US",
                        name,
                        self.freeze_theme_manager,
                    );
                }
            });

            if !self.vi_backups.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Danh sách Backup đã lưu:").strong());
                let backups_snapshot = self.vi_backups.clone();
                egui::ScrollArea::vertical()
                    .id_salt("vi_backups_scroll")
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for rec in backups_snapshot {
                            ui.horizontal_wrapped(|ui| {
                                ui.small(format!(
                                    "• {} | locale='{}' | locked={} | {} tệp .mrm",
                                    rec.id,
                                    rec.system_locales,
                                    rec.theme_guard_locked,
                                    rec.deployed_mrm_files.len()
                                ));
                                if ui
                                    .add_enabled(can_act, egui::Button::new("Rollback về bản này"))
                                    .clicked()
                                {
                                    self.start_rollback_vi_backup(rec);
                                }
                            });
                        }
                    });
            }

            if !self.dry_run_preview.is_empty() {
                ui.add_space(6.0);
                ui_elements::tool_subcard(ui, Some(colors::ACCENT_CYAN), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Lệnh ADB dự kiến (Dry-Run):").strong());
                        if ui.small_button("Sao chép lệnh").clicked() {
                            ui.ctx().copy_text(self.dry_run_preview.join("\n"));
                        }
                    });
                    for line in &self.dry_run_preview {
                        ui.small(line);
                    }
                });
            }
        });
    }

    fn render_tab_debloat(&mut self, ui: &mut egui::Ui, can_act: bool) {
        ui_elements::tool_card(ui, |ui| {
            ui.small(
                "Gỡ sâu đa tầng cho User 0 & app cài sẵn (`/data/app`): Gỡ bản cập nhật → Gỡ User 0 → Gỡ toàn cục → Vô hiệu hóa + Force-stop → Đóng băng (Suspend/Hide). Tự bảo vệ Launcher, Bàn phím đang dùng và Find Device chống bootloop.",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Chế độ:");
                for mode in [DeepRemovalMode::AutoDeep, DeepRemovalMode::FreezeOnly] {
                    if ui
                        .radio_value(&mut self.debloat_mode, mode, mode.label())
                        .changed()
                    {
                        self.confirm_debloat = false;
                    }
                }
            });
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(can_act, |ui| {
                        ui_elements::pulsing_cta_button(
                            ui,
                            "Quét App rác trên máy",
                            colors::ACCENT_BLUE,
                            can_act && !self.debloat_scanned,
                        )
                    })
                    .inner
                    .clicked()
                {
                    self.confirm_debloat = false;
                    self.debloat_report = None;
                    self.start_scan_debloat();
                }
                if ui.button("Chọn gói An toàn đang cài").clicked() {
                    self.confirm_debloat = false;
                    for item in &mut self.debloat_items {
                        item.selected =
                            item.installed && item.risk == "An toàn" && item.protected_reason.is_none();
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
                ui.label("Tìm:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.debloat_search)
                        .hint_text("tên app / package")
                        .desired_width(140.0),
                );
                ui.label("Lọc:");
                let prev_filter = self.debloat_filter;
                for f in [DebloatFilter::All, DebloatFilter::Installed, DebloatFilter::Removed] {
                    ui.selectable_value(&mut self.debloat_filter, f, f.label());
                }
                if self.debloat_filter != prev_filter {
                    self.confirm_debloat = false;
                }
            });

            if let Some(rep) = &self.debloat_report {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui_elements::status_pill(
                        ui,
                        &format!(
                            "Kết quả gần nhất: Gỡ {} · Tắt {} · Đóng băng {} · Đã gỡ sẵn {} · Bảo vệ {} · Lỗi {}",
                            rep.uninstalled,
                            rep.disabled,
                            rep.frozen,
                            rep.already_removed,
                            rep.skipped_protected,
                            rep.failed
                        ),
                        if rep.failed > 0 && rep.ok_total() == 0 {
                            colors::ACCENT_RED
                        } else {
                            colors::ACCENT_GREEN
                        },
                    );
                });
            }

            let needle = self.debloat_search.trim().to_ascii_lowercase();
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("hyperos_debloat_scroll")
                .max_height(245.0)
                .show(ui, |ui| {
                    for item in &mut self.debloat_items {
                        if !needle.is_empty()
                            && !item.name.to_ascii_lowercase().contains(&needle)
                            && !item.package.to_ascii_lowercase().contains(&needle)
                            && !item.category.to_ascii_lowercase().contains(&needle)
                        {
                            continue;
                        }
                        if self.debloat_scanned {
                            match self.debloat_filter {
                                DebloatFilter::All => {}
                                DebloatFilter::Installed if !item.installed => continue,
                                DebloatFilter::Removed if !item.restorable => continue,
                                _ => {}
                            }
                        }
                        ui_elements::tool_subcard(ui, None, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let locked = item.protected_reason.is_some();
                                ui.add_enabled_ui(!locked, |ui| {
                                    ui.checkbox(
                                        &mut item.selected,
                                        format!("{}  ({})", item.name, item.package),
                                    );
                                });
                                let risk_color = match item.risk {
                                    "An toàn" => colors::ACCENT_GREEN,
                                    "Cảnh báo" => colors::ACCENT_RED,
                                    _ => colors::ACCENT_AMBER,
                                };
                                ui_elements::status_pill(ui, item.risk, risk_color);
                                if let Some(reason) = &item.protected_reason {
                                    ui_elements::status_pill(
                                        ui,
                                        &format!("Bảo vệ: {reason}"),
                                        colors::ACCENT_RED,
                                    );
                                }
                                if self.debloat_scanned {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let st_color = match item.status {
                                            DebloatStatus::Installed => colors::ACCENT_GREEN,
                                            DebloatStatus::DisabledUser0 => colors::ACCENT_AMBER,
                                            DebloatStatus::SuspendedUser0 => colors::ACCENT_BLUE,
                                            DebloatStatus::RemovedUser0 => colors::ACCENT_RED,
                                            DebloatStatus::NotPresent => colors::TEXT_MUTED,
                                        };
                                        ui_elements::status_pill(ui, item.status.label(), st_color);
                                    });
                                }
                            });
                            ui.small(format!("  {} · [{}]", item.description, item.category));
                        });
                        ui.add_space(3.0);
                    }
                });

            ui.add_space(6.0);
            let removable_selected = self
                .debloat_items
                .iter()
                .filter(|i| {
                    i.selected
                        && i.protected_reason.is_none()
                        && (!self.debloat_scanned || i.installed)
                })
                .count();
            let restorable_selected = self
                .debloat_items
                .iter()
                .filter(|i| i.selected && (!self.debloat_scanned || i.restorable))
                .count();

            ui.horizontal_wrapped(|ui| {
                let verb = match self.debloat_mode {
                    DeepRemovalMode::AutoDeep => "gỡ/tắt/đóng băng",
                    DeepRemovalMode::FreezeOnly => "tắt/đóng băng",
                };
                if self.confirm_debloat {
                    if ui
                        .add_enabled_ui(can_act && removable_selected > 0, |ui| {
                            ui_elements::pulsing_cta_button(
                                ui,
                                &format!("Xác nhận {verb} {removable_selected} gói đang cài"),
                                colors::ACCENT_RED,
                                can_act && removable_selected > 0,
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
                        ui_elements::pulsing_cta_button(
                            ui,
                            &format!("Thực hiện {verb} ({removable_selected})"),
                            colors::ACCENT_AMBER,
                            can_act && self.debloat_scanned && !self.debloat_applied && removable_selected > 0,
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

                if self.debloat_applied {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui_elements::pulsing_cta_button(
                            ui,
                            "Tiếp theo: 4. Khóa Bảo mật (Bank-Ready) →",
                            colors::ACCENT_CYAN,
                            true,
                        )
                        .clicked()
                        {
                            self.active_tab = HyperOsTab::BankLock;
                        }
                    });
                }
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Gói tùy chọn:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_debloat_pkg)
                        .hint_text("VD: com.example.bloatware (cách nhau dấu phẩy)")
                        .desired_width(300.0),
                );
                let has_custom = !self.custom_debloat_pkg.trim().is_empty();
                if ui
                    .add_enabled(can_act && has_custom, egui::Button::new("Gỡ/Đóng băng gói này"))
                    .clicked()
                {
                    self.start_custom_debloat_action(false);
                }
                if ui
                    .add_enabled(can_act && has_custom, egui::Button::new("Khôi phục gói này"))
                    .clicked()
                {
                    self.start_custom_debloat_action(true);
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

            if self.bank_locked {
                ui.add_space(10.0);
                ui_elements::tool_subcard(ui, Some(colors::ACCENT_GREEN), |ui| {
                    ui.label(
                        egui::RichText::new("✓ Đã khóa Developer Options & USB Debugging thành công — Điện thoại sẵn sàng cho VNeID & App Ngân hàng!")
                            .color(colors::ACCENT_GREEN)
                            .strong(),
                    );
                    ui.small("Hãy rút cáp USB khỏi máy tính và mở thử VNeID / ứng dụng ngân hàng.");
                });
            }

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
                            ui_elements::pulsing_cta_button(
                                ui,
                                "Xác nhận TẮT Developer Options & ADB ngay!",
                                colors::ACCENT_RED,
                                can_act,
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
                        ui_elements::pulsing_cta_button(
                            ui,
                            "Hoàn tất & Khóa bảo mật (Tắt Dev + ADB)",
                            colors::ACCENT_AMBER,
                            can_act && !self.bank_locked,
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
                    if ui.small_button("Mở thư mục Audit").clicked() {
                        ui_elements::open_folder_in_os(&audit::audit_dir());
                    }
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
        let backups = hyperos::default_backups_dir();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = hyperos::fetch_device_info(adb.as_deref(), &serial).map(|info| {
                let loc_st = hyperos::get_locale_status(adb.as_deref(), &serial, &backups);
                let guard_st = hyperos::get_theme_guard_status(adb.as_deref(), &serial);
                (info, loc_st, guard_st)
            });
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
        let model = self.device_model();
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
            match &res {
                Ok(rep) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.notification_fix",
                    "MILLET_NO_RESTRICT_APP",
                    &format!(
                        "pkgs=[{}]; millet_ok={}; doze={}/{}; appops={}/{}",
                        pkgs.join(","),
                        rep.millet_ok,
                        rep.doze_ok,
                        rep.total_pkgs,
                        rep.appops_ok,
                        rep.total_pkgs
                    ),
                    true,
                    "",
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.notification_fix",
                    "MILLET_NO_RESTRICT_APP",
                    &format!("pkgs=[{}]", pkgs.join(",")),
                    false,
                    e,
                ),
            }
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
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::restore_millet_backup(adb.as_deref(), &serial, &backups, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            match &res {
                Ok(msg) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.millet_restore",
                    "MILLET_NO_RESTRICT_APP",
                    "",
                    true,
                    msg,
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.millet_restore",
                    "MILLET_NO_RESTRICT_APP",
                    "",
                    false,
                    e,
                ),
            }
            let _ = tx.send(HyperOsEvent::MilletRestored(res));
        });
    }

    fn apply_dry_run_preview(&mut self, cmds: Vec<String>, status_msg: String) {
        for line in &cmds {
            self.append_log(&format!("[DRY-RUN] {line}"));
        }
        self.dry_run_preview = cmds;
        self.status = status_msg;
        self.error = false;
    }

    fn start_set_locale(&mut self, tag: &str) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        if self.dry_run {
            let cmds = hyperos_vi::plan_dry_run_commands("locale", &serial, tag, None, false);
            let msg = format!("[Dry-Run] Đã in {} lệnh đặt locale '{tag}'.", cmds.len());
            self.apply_dry_run_preview(cmds, msg);
            return;
        }
        let tag = tag.to_string();
        self.busy = true;
        self.busy_label = format!("Đang đặt system_locales = {tag}...");
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::set_system_locale(adb.as_deref(), &serial, &tag, &backups, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            match &res {
                Ok(msg) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.locale",
                    &tag,
                    "system_locales",
                    true,
                    msg,
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.locale",
                    &tag,
                    "system_locales",
                    false,
                    e,
                ),
            }
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
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::restore_locale_backup(adb.as_deref(), &serial, &backups, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            match &res {
                Ok(msg) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.locale_restore",
                    "system_locales",
                    "",
                    true,
                    msg,
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.locale_restore",
                    "system_locales",
                    "",
                    false,
                    e,
                ),
            }
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
        let mtz_path = self.selected_mtz_path();
        if !mtz_path.is_file() {
            self.status = format!("Không tìm thấy file MTZ: {}", mtz_path.display());
            self.error = true;
            return;
        }
        if self.dry_run {
            let name = mtz_path.file_name().and_then(|n| n.to_str());
            let cmds = hyperos_vi::plan_dry_run_commands(
                "deploy_mtz",
                &serial,
                &self.locale_status.current,
                name,
                self.freeze_theme_manager,
            );
            let msg = format!("[Dry-Run] Đã in {} bước nạp MTZ.", cmds.len());
            self.apply_dry_run_preview(cmds, msg);
            return;
        }
        let template_path = self.themes_dir.join("metadata_template.json");
        let overwrite = self.mtz_overwrite;
        let expected_locale = self.locale_status.current.clone();
        self.busy = true;
        self.busy_label = "Đang sao lưu, đóng gói .mrm/.mrc & đẩy Theme qua ADB...".into();
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_bak = tx.clone();
            let _ = hyperos_vi::create_vi_backup(adb.as_deref(), &serial, move |l| {
                let _ = tx_bak.send(HyperOsEvent::Log(l));
            });
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
            match &res {
                Ok(staged) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.theme",
                    &staged.title,
                    &format!(
                        "uuid={}; modules={}; files={}; src={}",
                        staged.theme_uuid,
                        staged.module_count,
                        staged.file_count,
                        mtz_path.display()
                    ),
                    true,
                    "",
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.theme",
                    &mtz_path.display().to_string(),
                    "",
                    false,
                    e,
                ),
            }
            let (guard_opt, state_opt) = match &res {
                Ok(staged) => {
                    let g = hyperos::get_theme_guard_status(adb.as_deref(), &serial);
                    let st = hyperos_vi::record_vi_applied(
                        adb.as_deref(),
                        &serial,
                        Some(&mtz_path),
                        Some(&staged.title),
                        &expected_locale,
                    );
                    (Some(g), Some(st))
                }
                Err(_) => (None, None),
            };
            let _ = tx.send(HyperOsEvent::MtzDeployed(res, guard_opt, state_opt));
        });
    }

    fn start_lock_theme_guard(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let freeze = self.freeze_theme_manager;
        if self.dry_run {
            let cmds = hyperos_vi::plan_dry_run_commands(
                "lock_guard",
                &serial,
                "vi-VN,en-US",
                None,
                freeze,
            );
            let msg = format!("[Dry-Run] Đã in {} lệnh khóa DRM Chủ đề.", cmds.len());
            self.apply_dry_run_preview(cmds, msg);
            return;
        }
        self.busy = true;
        self.busy_label = "Đang khóa chống hoàn tác DRM Chủ đề...".into();
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::lock_theme_anti_revert(adb.as_deref(), &serial, freeze, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            if res.is_ok() {
                let st = hyperos_vi::load_device_vi_state(&serial);
                let _ = hyperos_vi::record_vi_applied(
                    adb.as_deref(),
                    &serial,
                    None,
                    None,
                    &st.expected_locale,
                );
            }
            match &res {
                Ok(msg) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.theme_guard.lock",
                    "com.android.thememanager",
                    &format!("freeze={freeze}"),
                    true,
                    msg,
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.theme_guard.lock",
                    "com.android.thememanager",
                    &format!("freeze={freeze}"),
                    false,
                    e,
                ),
            }
            let guard = hyperos::get_theme_guard_status(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::ThemeGuardUpdated(res, guard));
        });
    }

    fn start_detect_profile(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang dò Device Profile & kiểm tra sức khỏe Việt hóa...".into();
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let prof_res = hyperos_vi::detect_device_vi_profile(adb.as_deref(), &serial);
            let health_res = hyperos_vi::check_vi_health(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::HealthChecked {
                serial: serial.clone(),
                silent: true,
                res: health_res,
            });
            let _ = tx.send(HyperOsEvent::ProfileLoaded(serial, prof_res));
        });
    }

    fn start_health_check(&mut self, silent: bool) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.last_auto_health_at = Some(Instant::now());
        if !silent {
            self.busy = true;
            self.busy_label = "Đang kiểm tra sức khỏe Locale & Checksum Overlay...".into();
        }
        let adb = self.adb_override();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = hyperos_vi::check_vi_health(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::HealthChecked {
                serial,
                silent,
                res,
            });
        });
    }

    fn start_capture_snapshot(&mut self, label: &str) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let label = label.to_string();
        self.busy = true;
        self.busy_label = format!("Đang chụp Snapshot + Logcat ThemeManager ({label})...");
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos_vi::capture_revert_snapshot(
                adb.as_deref(),
                &serial,
                &label,
                move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                },
            );
            match &res {
                Ok((_, path)) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.snapshot",
                    &label,
                    &path.display().to_string(),
                    true,
                    "",
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.snapshot",
                    &label,
                    "",
                    false,
                    e,
                ),
            }
            let list = hyperos_vi::list_snapshots(&serial);
            let _ = tx.send(HyperOsEvent::SnapshotCaptured(res, list));
        });
    }

    fn start_one_click_reapply(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let mtz_path = self.selected_mtz_path();
        let template_path = self.themes_dir.join("metadata_template.json");
        let locale = if self.vi_state.expected_locale.is_empty() {
            "vi-VN,en-US".to_string()
        } else {
            self.vi_state.expected_locale.clone()
        };
        let dry_run = self.dry_run;
        self.busy = true;
        self.busy_label = "Đang áp lại Việt hóa (1 nút)...".into();
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos_vi::one_click_reapply(
                adb.as_deref(),
                &serial,
                mtz_path.is_file().then_some(mtz_path.as_path()),
                template_path.is_file().then_some(template_path.as_path()),
                &locale,
                dry_run,
                move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                },
            );
            let health_opt = if !dry_run {
                hyperos_vi::check_vi_health(adb.as_deref(), &serial).ok()
            } else {
                None
            };
            let _ = audit::record(
                &serial,
                model.as_deref(),
                "hyperos.reapply",
                &locale,
                &format!("dry_run={dry_run}"),
                res.is_ok(),
                res.as_deref().unwrap_or(""),
            );
            let _ = tx.send(HyperOsEvent::ReapplyDone(res, health_opt));
        });
    }

    fn start_validate_mtz(&mut self, rebuild: bool) {
        let mtz_path = self.selected_mtz_path();
        let themes_dir = self.themes_dir.clone();
        self.busy = true;
        self.busy_label = if rebuild {
            "Đang rebuild description.xml từ theme.toml & validate...".into()
        } else {
            "Đang validate MTZ theo theme.toml...".into()
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let stem = mtz_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let cfg = hyperos_vi::load_theme_config(Some(&themes_dir), stem);
            let res = if rebuild {
                hyperos_vi::rebuild_mtz_description_from_config(&mtz_path, &cfg)
            } else if !mtz_path.is_file() {
                Err(format!("Không tìm thấy tệp MTZ: {}", mtz_path.display()))
            } else {
                Ok(hyperos_vi::validate_mtz_archive(&mtz_path, &cfg))
            };
            let _ = tx.send(HyperOsEvent::MtzValidated(res));
        });
    }

    fn start_extract_and_score_tm(&mut self, with_diff: bool) {
        let mtz_path = self.selected_mtz_path();
        let old_path = THEME_CATALOG
            .get(self.diff_old_theme_idx)
            .map(|p| self.themes_dir.join(p.filename));
        self.busy = true;
        self.busy_label = "Đang trích chuỗi XML & tính độ phủ Translation Memory...".into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = (|| -> Result<(TranslationMemoryReport, Option<TranslationDiff>), String> {
                let cats = hyperos_vi::extract_strings_from_mtz(&mtz_path)?;
                let label = mtz_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("custom.mtz");
                let rep = hyperos_vi::import_and_score_translation_memory(&cats, label)?;
                let diff_opt = if with_diff {
                    if let Some(op) = old_path.filter(|p| p.is_file()) {
                        let old_cats = hyperos_vi::extract_strings_from_mtz(&op)?;
                        Some(hyperos_vi::diff_string_catalogs(&old_cats, &cats))
                    } else {
                        None
                    }
                } else {
                    None
                };
                Ok((rep, diff_opt))
            })();
            let _ = tx.send(HyperOsEvent::TranslationProcessed(res));
        });
    }

    fn start_export_magisk_module(&mut self) {
        let mtz_path = self.selected_mtz_path();
        let themes_dir = self.themes_dir.clone();
        let serial = self.selected_serial.clone();
        let model = self.device_model();
        self.busy = true;
        self.busy_label = "Đang đóng gói Magisk Module Việt hóa (.zip)...".into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let stem = mtz_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("MTZOS4");
            let cfg = hyperos_vi::load_theme_config(Some(&themes_dir), stem);
            let out_zip = hyperos_vi::default_vi_data_dir()
                .join("magisk_modules")
                .join(format!("Magisk_HyperOS_VI_{stem}.zip"));
            let res = hyperos_vi::export_magisk_vi_module(&mtz_path, &out_zip, &cfg);
            if let Some(ref s) = serial {
                audit::record(
                    s,
                    model.as_deref(),
                    "hyperos.magisk_export",
                    stem,
                    &out_zip.display().to_string(),
                    res.is_ok(),
                    "",
                );
            }
            let _ = tx.send(HyperOsEvent::MagiskExported(res));
        });
    }

    fn start_create_vi_backup(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang tạo Backup trạng thái Việt hóa...".into();
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos_vi::create_vi_backup(adb.as_deref(), &serial, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            let _ = audit::record(
                &serial,
                model.as_deref(),
                "hyperos.vi_backup",
                &res.as_ref().map(|r| r.id.as_str()).unwrap_or(""),
                "",
                res.is_ok(),
                "",
            );
            let list = hyperos_vi::list_vi_backups(&serial);
            let _ = tx.send(HyperOsEvent::ViBackupCreated(res, list));
        });
    }

    fn start_rollback_vi_backup(&mut self, rec: ViBackupRecord) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let dry_run = self.dry_run;
        self.busy = true;
        self.busy_label = format!("Đang rollback về bản sao lưu '{}'...", rec.id);
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let id = rec.id.clone();
            let res = hyperos_vi::rollback_vi_backup(
                adb.as_deref(),
                &serial,
                &rec,
                dry_run,
                move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                },
            );
            let health_opt = if !dry_run {
                hyperos_vi::check_vi_health(adb.as_deref(), &serial).ok()
            } else {
                None
            };
            let _ = audit::record(
                &serial,
                model.as_deref(),
                "hyperos.vi_rollback",
                &id,
                &format!("dry_run={dry_run}"),
                res.is_ok(),
                res.as_deref().unwrap_or(""),
            );
            let _ = tx.send(HyperOsEvent::ViRollbackDone(res, health_opt));
        });
    }

    fn start_unlock_theme_guard(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang mở lại app Chủ đề...".into();
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::unlock_theme_manager(adb.as_deref(), &serial, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            match &res {
                Ok(msg) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.theme_guard.unlock",
                    "com.android.thememanager",
                    "",
                    true,
                    msg,
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.theme_guard.unlock",
                    "com.android.thememanager",
                    "",
                    false,
                    e,
                ),
            }
            let guard = hyperos::get_theme_guard_status(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::ThemeGuardUpdated(res, guard));
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
            .filter(|i| {
                i.selected
                    && i.protected_reason.is_none()
                    && (!self.debloat_scanned || i.installed)
            })
            .map(|i| i.package.clone())
            .collect();
        if pkgs.is_empty() {
            return;
        }
        let mode = self.debloat_mode;
        self.busy = true;
        self.busy_label = format!("Đang xử lý sâu {} gói...", pkgs.len());
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let report = hyperos::uninstall_packages(
                adb.as_deref(),
                &serial,
                &pkgs,
                mode,
                &backups,
                move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                },
            );
            audit_debloat_outcomes(&serial, model.as_deref(), mode, &report);
            let rescanned = hyperos::scan_debloat_packages(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::DebloatUninstallDone { report, rescanned });
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
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let report =
                hyperos::restore_packages(adb.as_deref(), &serial, &pkgs, &backups, move |l| {
                    let _ = tx_log.send(HyperOsEvent::Log(l));
                });
            audit_restore_outcomes(&serial, model.as_deref(), &report);
            let (ok, fail) = (report.ok, report.fail);
            let rescanned = hyperos::scan_debloat_packages(adb.as_deref(), &serial);
            let _ = tx.send(HyperOsEvent::DebloatDone {
                action: "Khôi phục app đã gỡ",
                ok,
                fail,
                rescanned,
            });
        });
    }

    fn start_custom_debloat_action(&mut self, restore: bool) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        let pkgs: Vec<String> = self
            .custom_debloat_pkg
            .split([',', ';', ' ', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if pkgs.is_empty() {
            return;
        }
        let mode = self.debloat_mode;
        self.busy = true;
        self.busy_label = if restore {
            format!("Đang khôi phục {} gói tùy chọn...", pkgs.len())
        } else {
            format!("Đang xử lý sâu {} gói tùy chọn...", pkgs.len())
        };
        let adb = self.adb_override();
        let backups = hyperos::default_backups_dir();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            if restore {
                let report = hyperos::restore_packages(
                    adb.as_deref(),
                    &serial,
                    &pkgs,
                    &backups,
                    move |l| {
                        let _ = tx_log.send(HyperOsEvent::Log(l));
                    },
                );
                audit_restore_outcomes(&serial, model.as_deref(), &report);
                let (ok, fail) = (report.ok, report.fail);
                let rescanned = hyperos::scan_debloat_packages(adb.as_deref(), &serial);
                let _ = tx.send(HyperOsEvent::DebloatDone {
                    action: "Khôi phục gói tùy chọn",
                    ok,
                    fail,
                    rescanned,
                });
            } else {
                let report = hyperos::uninstall_packages(
                    adb.as_deref(),
                    &serial,
                    &pkgs,
                    mode,
                    &backups,
                    move |l| {
                        let _ = tx_log.send(HyperOsEvent::Log(l));
                    },
                );
                audit_debloat_outcomes(&serial, model.as_deref(), mode, &report);
                let rescanned = hyperos::scan_debloat_packages(adb.as_deref(), &serial);
                let _ = tx.send(HyperOsEvent::DebloatUninstallDone { report, rescanned });
            }
        });
    }

    fn start_bank_lock(&mut self) {
        let Some(serial) = self.selected_serial.clone() else {
            return;
        };
        self.busy = true;
        self.busy_label = "Đang tắt Developer Options & USB Debugging...".into();
        let adb = self.adb_override();
        let model = self.device_model();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let res = hyperos::lock_developer_and_adb(adb.as_deref(), &serial, move |l| {
                let _ = tx_log.send(HyperOsEvent::Log(l));
            });
            match &res {
                Ok(msg) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.bank_lock",
                    "",
                    "developer_options+usb_debugging",
                    true,
                    msg,
                ),
                Err(e) => audit::record(
                    &serial,
                    model.as_deref(),
                    "hyperos.bank_lock",
                    "",
                    "developer_options+usb_debugging",
                    false,
                    e,
                ),
            }
            let _ = tx.send(HyperOsEvent::BankLocked(res));
        });
    }
}

/// Ghi audit log cho một đợt debloat: một dòng cho MỖI package, action theo
/// đúng tier đã xử lý package đó (gỡ / tắt / đóng băng), detail là chế độ chạy.
fn audit_debloat_outcomes(
    serial: &str,
    model: Option<&str>,
    mode: DeepRemovalMode,
    report: &DebloatReport,
) {
    let detail = mode.label();
    for (pkg, outcome) in &report.outcomes {
        let (action, ok, message) = match outcome {
            DebloatOutcome::Removed => ("debloat.remove", true, ""),
            DebloatOutcome::Disabled => ("debloat.disable", true, ""),
            DebloatOutcome::Frozen => ("debloat.freeze", true, ""),
            DebloatOutcome::AlreadyRemoved => {
                ("debloat.remove", true, "Đã gỡ sẵn cho User 0 từ trước")
            }
            DebloatOutcome::SkippedProtected => (
                "debloat.skipped_protected",
                false,
                "Bỏ qua do bảo vệ hệ thống",
            ),
            DebloatOutcome::Failed => (
                "debloat.failed",
                false,
                "Gỡ/tắt/đóng băng đều thất bại",
            ),
        };
        audit::record(serial, model, action, pkg, detail, ok, message);
    }
}

/// Ghi audit log cho một đợt khôi phục: một dòng cho mỗi package.
fn audit_restore_outcomes(serial: &str, model: Option<&str>, report: &hyperos::RestoreReport) {
    for (pkg, ok) in &report.outcomes {
        audit::record(
            serial,
            model,
            "restore.package",
            pkg,
            "",
            *ok,
            if *ok { "" } else { "Không khôi phục được" },
        );
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
