//! App cleanup dialog: list installed packages, select junk apps, and remove
//! them for the primary user. System packages are restored with a single click.

use std::collections::HashSet;
use std::thread;

use adb_client::apps::UninstallOutcome;
use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;

use crate::audit;
use crate::format::format_bytes_mb;
use crate::i18n;
use crate::metrics::PackageStorageState;
use crate::theme::{self, colors};
use crate::ui_elements;

/// Packages that must never be offered for removal: removing them can brick
/// the UI, telephony, permissions, or the default launcher.
const PROTECTED_PACKAGES: &[&str] = &[
    "android",
    "com.android.systemui",
    "com.android.settings",
    "com.android.phone",
    "com.android.shell",
    "com.android.packageinstaller",
    "com.google.android.packageinstaller",
    "com.android.permissioncontroller",
    "com.google.android.permissioncontroller",
    "com.android.providers.settings",
    "com.android.providers.telephony",
    "com.android.providers.contacts",
    "com.android.providers.media",
    "com.android.providers.media.module",
    "com.android.providers.downloads",
    "com.android.server.telecom",
    "com.android.incallui",
    "com.android.dialer",
    "com.google.android.dialer",
    "com.android.contacts",
    "com.google.android.contacts",
    "com.android.mms.service",
    "com.android.messaging",
    "com.google.android.apps.messaging",
    "com.android.bluetooth",
    "com.android.networkstack",
    "com.google.android.networkstack",
    "com.android.networkstack.tethering",
    "com.android.externalstorage",
    "com.android.documentsui",
    "com.google.android.documentsui",
    "com.android.keychain",
    "com.android.certinstaller",
    "com.android.webview",
    "com.google.android.webview",
    "com.google.android.gms",
    "com.google.android.gsf",
    "com.android.vending",
    "com.miui.home",
    "com.miui.securitycenter",
    "com.google.android.apps.nexuslauncher",
    "com.android.launcher3",
    "com.sec.android.app.launcher",
    "com.oplus.launcher",
    "com.oneplus.launcher",
    "com.vivo.launcher",
    "com.bbk.launcher2",
    "com.huawei.android.launcher",
    "com.transsion.XOSLauncher",
];


const LIST_HEIGHT: f32 = 330.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageAction {
    Uninstall,
    Restore,
}

impl PackageAction {
    fn verb(self) -> &'static str {
        match self {
            PackageAction::Uninstall => i18n::t("cleanup.verb_uninstall"),
            PackageAction::Restore => i18n::t("cleanup.verb_restore"),
        }
    }
}

enum CleanupEvent {
    ProtectedLoaded(String, Vec<String>),
    Done {
        action: PackageAction,
        package: String,
        result: Result<Option<UninstallOutcome>, String>,
    },
    Finished {
        action: PackageAction,
        succeeded: usize,
        failed: usize,
    },
}

struct Status {
    text: String,
    error: bool,
}

struct Row {
    package: String,
    system: bool,
    size: Option<u64>,
    protected: bool,
}

pub struct AppCleanup {
    search: String,
    show_system: bool,
    selected: HashSet<String>,
    /// Active IME and launcher detected from the bound device.
    active_protected: HashSet<String>,
    protected_serial: Option<String>,
    protected_rx: Option<Receiver<CleanupEvent>>,
    /// Removed during this session; hidden from the list until rescanned.
    hidden: HashSet<String>,
    /// Removed/disabled/suspended packages that can be restored in this session.
    removed: Vec<(String, UninstallOutcome)>,
    /// Packages awaiting confirmation.
    pending: Option<Vec<String>>,
    /// System packages in the pending batch; only these (plus disabled/suspended) can be restored.
    pending_system: HashSet<String>,
    status: Option<Status>,
    busy: bool,
    rx: Option<Receiver<CleanupEvent>>,
}

impl Default for AppCleanup {
    fn default() -> Self {
        Self {
            search: String::new(),
            show_system: true,
            selected: HashSet::new(),
            active_protected: HashSet::new(),
            protected_serial: None,
            protected_rx: None,
            hidden: HashSet::new(),
            removed: Vec::new(),
            pending: None,
            pending_system: HashSet::new(),
            status: None,
            busy: false,
            rx: None,
        }
    }
}

impl AppCleanup {
    /// Prepares the tab view, keeping the current search and selection
    /// but dropping any stale pending confirmation.
    pub fn open(&mut self) {
        self.pending = None;
        self.pending_system.clear();
    }

    /// Drops all state, e.g. when the selected device changes.
    pub fn reset(&mut self) {
        let rx = self.rx.take();
        drop(rx);
        let prx = self.protected_rx.take();
        drop(prx);
        *self = Self::default();
    }

    /// Applies worker results. Returns true when the UI needs a repaint.
    pub fn tick(&mut self) -> bool {
        let mut events = Vec::new();
        if let Some(rx) = self.rx.as_ref() {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }
        if let Some(prx) = self.protected_rx.as_ref() {
            while let Ok(event) = prx.try_recv() {
                events.push(event);
            }
        }
        if events.is_empty() {
            return false;
        }

        for event in events {
            match event {
                CleanupEvent::ProtectedLoaded(serial, pkgs) => {
                    self.protected_rx = None;
                    if self.protected_serial.as_deref() == Some(serial.as_str()) {
                        for p in &pkgs {
                            self.selected.remove(p);
                        }
                        self.active_protected = pkgs.into_iter().collect();
                    }
                }
                CleanupEvent::Done {
                    action,
                    package,
                    result,
                } => match result {
                    Ok(outcome_opt) => {
                        if action == PackageAction::Uninstall {
                            let outcome = outcome_opt.unwrap_or(UninstallOutcome::Uninstalled);
                            self.hidden.insert(package.clone());
                            self.selected.remove(&package);
                            let restorable = outcome != UninstallOutcome::Uninstalled
                                || self.pending_system.contains(&package);
                            if restorable && !self.removed.iter().any(|(name, _)| name == &package)
                            {
                                self.removed.push((package, outcome));
                            }
                        } else {
                            self.hidden.remove(&package);
                            self.removed.retain(|(name, _)| name != &package);
                        }
                    }
                    Err(message) => {
                        self.status = Some(Status {
                            text: message,
                            error: true,
                        });
                    }
                },
                CleanupEvent::Finished {
                    action,
                    succeeded,
                    failed,
                } => {
                    self.busy = false;
                    self.rx = None;
                    let ok_s = succeeded.to_string();
                    let fail_s = failed.to_string();
                    self.status = Some(Status {
                        text: if failed == 0 {
                            i18n::tf("cleanup.result_ok", &[("verb", action.verb()), ("ok", &ok_s)])
                        } else {
                            i18n::tf(
                                "cleanup.result_fail",
                                &[("verb", action.verb()), ("ok", &ok_s), ("fail", &fail_s)],
                            )
                        },
                        error: failed > 0,
                    });
                    if action == PackageAction::Uninstall {
                        self.pending = None;
                        self.pending_system.clear();
                    }
                }
            }
        }
        true
    }

    /// Renders the cleanup UI inside the Apps tab.
    pub fn show_in(
        &mut self,
        ui: &mut egui::Ui,
        serial: Option<&str>,
        storage: &PackageStorageState,
    ) {
        if let Some(s) = serial {
            if self.protected_serial.as_deref() != Some(s) {
                self.protected_serial = Some(s.to_string());
                self.active_protected.clear();
                let (tx, rx) = unbounded();
                self.protected_rx = Some(rx);
                let serial_owned = s.to_string();
                thread::spawn(move || {
                    let pkgs = adb_client::apps::query_active_protected_packages(&serial_owned);
                    let _ = tx.send(CleanupEvent::ProtectedLoaded(serial_owned, pkgs));
                });
            }
        }

        ui_elements::tool_card(ui, |ui| {
            ui_elements::section_header(ui, i18n::t("cleanup.title"), "");
            ui.add_space(4.0);
            if self.pending.is_some() {
                self.show_confirmation(ui, serial, storage);
            } else {
                self.show_list(ui, serial, storage);
            }
        });
    }

    fn show_list(
        &mut self,
        ui: &mut egui::Ui,
        serial: Option<&str>,
        storage: &PackageStorageState,
    ) {
        if let Some(serial) = serial {
            ui.label(
                egui::RichText::new(i18n::tf("cleanup.device", &[("serial", serial)]))
                    .small()
                    .color(colors::FOOTER_TEXT),
            );
        }

        ui.add_space(theme::ITEM_SPACING_Y);
        ui.horizontal(|ui| {
            ui.label(i18n::t("cleanup.search"));
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(
                        egui::RichText::new(i18n::t("cleanup.search_hint"))
                            .color(colors::PLACEHOLDER_TEXT),
                    )
                    .desired_width(240.0),
            );
            ui.checkbox(&mut self.show_system, i18n::t("cleanup.show_system"));
            if ui.button(i18n::t("cleanup.clear_selection")).clicked() {
                self.selected.clear();
            }
        });
        ui.add_space(theme::ITEM_SPACING_Y);

        let rows = self.visible_rows(storage);
        if rows.is_empty() {
            ui.label(if storage.packages.is_empty() {
                i18n::t("cleanup.waiting_scan")
            } else {
                i18n::t("cleanup.no_match")
            });
            return;
        }

        ui.horizontal(|ui| {
            let selectable: Vec<&str> = rows
                .iter()
                .filter(|row| !row.protected)
                .map(|row| row.package.as_str())
                .collect();
            if ui.button(i18n::t("cleanup.select_all")).clicked() {
                for package in selectable {
                    self.selected.insert(package.to_string());
                }
            }
            let shown = rows.len().to_string();
            let total = storage.packages.len().to_string();
            ui.label(
                egui::RichText::new(i18n::tf(
                    "cleanup.package_count",
                    &[("shown", &shown), ("total", &total)],
                ))
                .small()
                .color(colors::FOOTER_TEXT),
            );
            if storage.scanning {
                ui.spinner();
            }
        });
        ui.add_space(theme::ITEM_SPACING_Y);

        egui::ScrollArea::vertical()
            .id_salt(egui::Id::new("cleanup_rows"))
            .auto_shrink([false, false])
            .max_height(LIST_HEIGHT)
            .show(ui, |ui| {
                egui::Grid::new("cleanup_grid")
                    .num_columns(3)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for row in &rows {
                            self.row_ui(ui, row);
                            ui.end_row();
                        }
                    });
            });

        ui.add_space(theme::ITEM_SPACING_Y);
        ui.separator();
        self.show_selection_bar(ui, serial, storage);
        self.show_status(ui);
        self.show_removed(ui, serial);
        self.show_hint(ui);
    }

    fn row_ui(&mut self, ui: &mut egui::Ui, row: &Row) {
        let mut checked = self.selected.contains(&row.package);
        ui.add_enabled_ui(!row.protected && !self.busy, |ui| {
            if ui.checkbox(&mut checked, "").changed() {
                if checked {
                    self.selected.insert(row.package.clone());
                } else {
                    self.selected.remove(&row.package);
                }
            }
        });
        if row.protected {
            ui.label(
                egui::RichText::new(&row.package)
                    .monospace()
                    .color(colors::FOOTER_TEXT),
            )
            .on_hover_text(i18n::t("cleanup.protected_hover"));
        } else {
            ui.label(egui::RichText::new(&row.package).monospace())
                .on_hover_text(&row.package);
        }

        let kind = if row.protected {
            i18n::t("cleanup.kind_core")
        } else if row.system {
            i18n::t("cleanup.kind_system")
        } else {
            i18n::t("cleanup.kind_user")
        };
        let kind_color = if row.system {
            colors::LOG_WARNING
        } else {
            colors::FOOTER_TEXT
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(kind).small().color(kind_color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(match row.size {
                    Some(bytes) => format_bytes_mb(bytes),
                    None => "…".to_string(),
                });
            });
        });
    }

    fn show_selection_bar(
        &mut self,
        ui: &mut egui::Ui,
        serial: Option<&str>,
        storage: &PackageStorageState,
    ) {
        let selected_bytes: u64 = self
            .selected
            .iter()
            .filter_map(|package| storage.sizes.get(package).copied())
            .sum();

        let count_s = self.selected.len().to_string();
        let size_s = format_bytes_mb(selected_bytes);
        ui.horizontal(|ui| {
            ui.label(i18n::tf(
                "cleanup.selected_summary",
                &[("count", &count_s), ("size", &size_s)],
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_uninstall = !self.busy && serial.is_some() && !self.selected.is_empty();
                let button = egui::Button::new(
                    egui::RichText::new(i18n::t("cleanup.uninstall_selected"))
                        .color(colors::OFF_WHITE),
                )
                .fill(colors::ERROR);
                if ui.add_enabled(can_uninstall, button).clicked() {
                    let mut packages: Vec<String> = self.selected.iter().cloned().collect();
                    packages.sort();
                    packages.retain(|package| !is_protected(package) && !self.active_protected.contains(package));
                    if !packages.is_empty() {
                        self.pending_system = packages
                            .iter()
                            .filter(|package| storage.is_system(package))
                            .cloned()
                            .collect();
                        self.pending = Some(packages);
                    }
                }
            });
        });
    }

    fn show_status(&mut self, ui: &mut egui::Ui) {
        if let Some(status) = &self.status {
            ui.add_space(theme::ITEM_SPACING_Y);
            let color = if status.error {
                colors::ERROR
            } else {
                colors::LOG_INFO
            };
            ui.label(egui::RichText::new(&status.text).small().color(color));
        }
        if self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new(i18n::t("cleanup.running_adb"))
                        .small()
                        .color(colors::FOOTER_TEXT),
                );
            });
        }
    }

    fn show_removed(&mut self, ui: &mut egui::Ui, serial: Option<&str>) {
        if self.removed.is_empty() {
            return;
        }

        ui.add_space(theme::ITEM_SPACING_Y);
        ui.separator();
        ui.label(
            egui::RichText::new(i18n::t("cleanup.removed_session"))
                .small()
                .color(colors::FOOTER_TEXT),
        );
        let removed = self.removed.clone();
        for (package, outcome) in removed {
            let outcome_label = match outcome {
                UninstallOutcome::Uninstalled => i18n::t("cleanup.outcome_uninstalled"),
                UninstallOutcome::Disabled => i18n::t("cleanup.outcome_disabled"),
                UninstallOutcome::Suspended => i18n::t("cleanup.outcome_suspended"),
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&package).monospace().small());
                ui.label(
                    egui::RichText::new(format!("({outcome_label})"))
                        .small()
                        .color(colors::FOOTER_TEXT),
                );
                if ui
                    .add_enabled(
                        !self.busy && serial.is_some(),
                        egui::Button::new(i18n::t("cleanup.restore")),
                    )
                    .clicked()
                {
                    if let Some(serial) = serial {
                        self.start_batch(serial.to_string(), PackageAction::Restore, vec![package]);
                    }
                }
            });
        }
    }

    fn show_hint(&self, ui: &mut egui::Ui) {
        ui.add_space(theme::ITEM_SPACING_Y);
        ui.label(
            egui::RichText::new(i18n::t("cleanup.hint"))
                .small()
                .color(colors::FOOTER_TEXT),
        );
    }

    fn show_confirmation(
        &mut self,
        ui: &mut egui::Ui,
        serial: Option<&str>,
        storage: &PackageStorageState,
    ) {
        let packages = self.pending.clone().unwrap_or_default();
        let total_bytes: u64 = packages
            .iter()
            .filter_map(|package| storage.sizes.get(package).copied())
            .sum();

        let count_s = packages.len().to_string();
        let size_s = format_bytes_mb(total_bytes);
        ui.heading(i18n::t("cleanup.confirm_title"));
        ui.label(i18n::tf(
            "cleanup.confirm_summary",
            &[("count", &count_s), ("size", &size_s)],
        ));
        ui.add_space(theme::ITEM_SPACING_Y);

        egui::ScrollArea::vertical()
            .id_salt(egui::Id::new("cleanup_confirm"))
            .max_height(260.0)
            .show(ui, |ui| {
                for package in &packages {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(package).monospace());
                        if storage.is_system(package) {
                            ui.label(
                                egui::RichText::new(i18n::t("cleanup.kind_system"))
                                    .small()
                                    .color(colors::LOG_WARNING),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                storage
                                    .sizes
                                    .get(package)
                                    .map(|bytes| format_bytes_mb(*bytes))
                                    .unwrap_or_else(|| "…".to_string()),
                            );
                        });
                    });
                }
            });

        ui.add_space(theme::ITEM_SPACING_Y);
        ui.label(
            egui::RichText::new(i18n::t("cleanup.confirm_warning"))
                .small()
                .color(colors::LOG_WARNING),
        );
        ui.add_space(theme::ITEM_SPACING_Y);

        ui.horizontal(|ui| {
            // Destructive action on the left, Cancel on the right (where the
            // "Uninstall selected…" button was). This prevents a double-click
            // from selecting a package and confirming removal in one gesture.
            let can_run = !self.busy && serial.is_some();
            let button = egui::Button::new(
                egui::RichText::new(i18n::tf("cleanup.confirm_btn", &[("count", &count_s)]))
                    .color(colors::OFF_WHITE),
            )
            .fill(colors::ERROR);
            if ui.add_enabled(can_run, button).clicked() {
                if let Some(serial) = serial {
                    self.start_batch(
                        serial.to_string(),
                        PackageAction::Uninstall,
                        packages.clone(),
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(i18n::t("common.cancel")).clicked() {
                    self.pending = None;
                    self.pending_system.clear();
                }
            });
        });
    }

    fn start_batch(&mut self, serial: String, action: PackageAction, packages: Vec<String>) {
        let (tx, rx) = unbounded();
        self.rx = Some(rx);
        self.busy = true;
        self.status = None;
        self.pending = None;

        thread::spawn(move || {
            let mut succeeded = 0;
            let mut failed = 0;
            for package in packages {
                let result = match action {
                    PackageAction::Uninstall => {
                        adb_client::apps::uninstall_package_deep(&serial, &package).map(Some)
                    }
                    PackageAction::Restore => {
                        adb_client::apps::restore_package_deep(&serial, &package).map(|()| None)
                    }
                }
                .map_err(|err| i18n::adb_error_vi(&err.user_message()));

                // Audit log: một dòng cho mỗi package đã gỡ / khôi phục.
                let audit_action = match action {
                    PackageAction::Uninstall => "app.uninstall",
                    PackageAction::Restore => "app.restore",
                };
                match &result {
                    Ok(outcome) => {
                        let detail = match outcome {
                            Some(o) => format!("outcome={o:?}"),
                            None => String::new(),
                        };
                        audit::record(
                            &serial,
                            None,
                            audit_action,
                            &package,
                            &detail,
                            true,
                            "",
                        );
                    }
                    Err(e) => {
                        audit::record(&serial, None, audit_action, &package, "", false, e);
                    }
                }

                if result.is_ok() {
                    succeeded += 1;
                } else {
                    failed += 1;
                }
                if tx
                    .send(CleanupEvent::Done {
                        action,
                        package,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = tx.send(CleanupEvent::Finished {
                action,
                succeeded,
                failed,
            });
        });
    }

    fn visible_rows(&self, storage: &PackageStorageState) -> Vec<Row> {
        let needle = self.search.trim().to_lowercase();
        let mut rows: Vec<Row> = storage
            .packages
            .iter()
            .filter(|package| !self.hidden.contains(*package))
            .filter(|package| self.show_system || !storage.is_system(package))
            .filter(|package| needle.is_empty() || package.to_lowercase().contains(&needle))
            .map(|package| Row {
                package: package.clone(),
                system: storage.is_system(package),
                size: storage.sizes.get(package).copied(),
                protected: is_protected(package) || self.active_protected.contains(package),
            })
            .collect();

        rows.sort_by(|left, right| match (left.size, right.size) {
            (Some(left_size), Some(right_size)) => right_size
                .cmp(&left_size)
                .then_with(|| left.package.cmp(&right.package)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.package.cmp(&right.package),
        });
        rows
    }
}

fn is_protected(package: &str) -> bool {
    PROTECTED_PACKAGES.contains(&package)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_packages_are_protected() {
        assert!(is_protected("com.android.systemui"));
        assert!(is_protected("com.miui.home"));
        assert!(!is_protected("com.facebook.katana"));
    }
}
