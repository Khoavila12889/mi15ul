use adb_client::ProtocolStats;
use eframe::egui;

use crate::format::format_bytes_mb;
use crate::i18n;
use crate::metrics::MetricStore;
use crate::theme;
use crate::ui_elements;

pub fn protocols_panel(ui: &mut egui::Ui, metrics: &MetricStore) {
    if let Some(error) = &metrics.protocol_error {
        ui_elements::error_label(ui, i18n::adb_error_vi(error));
    }

    ui_elements::panel_body(ui, theme::colors::APP_TRAFFIC_BODY, |ui| {
        let Some(stats) = &metrics.protocol_stats else {
            if metrics.protocol_error.is_none() {
                ui.label(i18n::t("traffic.fetching"));
            }
            return;
        };

        show_protocol_stats(ui, stats);
    });
}

fn show_protocol_stats(ui: &mut egui::Ui, stats: &ProtocolStats) {
    if stats.apps.is_empty() {
        ui.label(i18n::t("traffic.empty"));
        return;
    }

    egui::ScrollArea::both()
        .id_salt(egui::Id::new("app_traffic_scroll"))
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            egui::Grid::new("app_traffic")
                .num_columns(6)
                .spacing(theme::GRID_SPACING)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(i18n::t("traffic.col_package"));
                    ui.label(i18n::t("traffic.col_total"));
                    ui.label(i18n::t("traffic.col_foreground"));
                    ui.label(i18n::t("traffic.col_background"));
                    ui.label(i18n::t("traffic.col_wifi"));
                    ui.label(i18n::t("traffic.col_mobile"));
                    ui.end_row();

                    for app in &stats.apps {
                        ui.label(&app.package);
                        ui.label(format_bytes_mb(app.total_bytes));
                        ui.label(format_bytes_mb(app.foreground_bytes));
                        ui.label(format_bytes_mb(app.background_bytes));
                        ui.label(format_bytes_mb(app.wifi_bytes));
                        ui.label(format_bytes_mb(app.mobile_bytes));
                        ui.end_row();
                    }
                });
        });
}
