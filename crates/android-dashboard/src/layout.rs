use eframe::egui;
use egui_tiles::{Behavior, ResizeState, TileId, Tree, UiResponse};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::i18n;
use crate::panels;
use crate::panels::devices::DevicesAction;
use crate::tabs::MainTab;
use crate::theme;
use crate::ui_elements;

// Share is egui's term for the relative width of a panel.
const COL_SHARE: f32 = 1.0;
const GAUGES_SHARE: f32 = 1.5;
const STORAGE_DETAILS_SHARE: f32 = 2.5;
const LOGCAT_SHARE: f32 = 2.0;
const INSIGHT_SHARE: f32 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum PanelId {
    Devices,
    Ram,
    Storage,
    LogcatAll,
    LogcatErrors,
    Insight,
    SystemStats,
    Network,
    Protocols,
}

impl PanelId {
    pub(crate) const ALL: [Self; 9] = [
        Self::Devices,
        Self::Ram,
        Self::Storage,
        Self::LogcatAll,
        Self::LogcatErrors,
        Self::Insight,
        Self::SystemStats,
        Self::Network,
        Self::Protocols,
    ];

    /// Returns this pane's variant name.
    fn ident(self) -> &'static str {
        match self {
            PanelId::Devices => "Devices",
            PanelId::Ram => "Ram",
            PanelId::Storage => "Storage",
            PanelId::LogcatAll => "LogcatAll",
            PanelId::LogcatErrors => "LogcatErrors",
            PanelId::Insight => "Insight",
            PanelId::SystemStats => "SystemStats",
            PanelId::Network => "Network",
            PanelId::Protocols => "Protocols",
        }
    }

    fn title(self) -> &'static str {
        match self {
            PanelId::Devices => i18n::t("panel.devices"),
            PanelId::Ram => i18n::t("panel.ram"),
            PanelId::Storage => i18n::t("panel.storage"),
            PanelId::LogcatAll => i18n::t("panel.logcat"),
            PanelId::LogcatErrors => i18n::t("panel.logcat_errors"),
            PanelId::Insight => i18n::t("panel.insight"),
            PanelId::SystemStats => i18n::t("panel.storage_details"),
            PanelId::Network => i18n::t("panel.network"),
            PanelId::Protocols => i18n::t("panel.app_traffic"),
        }
    }

    /// Returns the header icon for this panel.
    fn icon(self) -> Option<egui::ImageSource<'static>> {
        Some(match self {
            PanelId::Ram => theme::icons::ram(),
            PanelId::Storage | PanelId::SystemStats => theme::icons::disc(),
            PanelId::Network => theme::icons::network(),
            PanelId::LogcatErrors => theme::icons::errors(),
            PanelId::LogcatAll => theme::icons::logcat(),
            PanelId::Insight => theme::icons::insight(),
            PanelId::Protocols => theme::icons::traffic(),
            PanelId::Devices => theme::icons::device(),
        })
    }
}

impl From<PanelId> for String {
    fn from(pane: PanelId) -> Self {
        pane.ident().to_owned()
    }
}

impl TryFrom<String> for PanelId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::ALL
            .into_iter()
            .find(|pane| pane.ident() == value)
            .ok_or(value)
    }
}

/// Cây ô cho tab Tổng quan: thiết bị, đồng hồ RAM/bộ nhớ, chi tiết bộ nhớ,
/// lưu lượng mạng và lưu lượng ứng dụng.
pub fn create_overview_tree() -> Tree<PanelId> {
    let mut tiles = egui_tiles::Tiles::default();

    let devices = tiles.insert_pane(PanelId::Devices);
    let ram = tiles.insert_pane(PanelId::Ram);
    let storage = tiles.insert_pane(PanelId::Storage);
    let gauges = tiles.insert_horizontal_tile(vec![ram, storage]);
    let system_stats = tiles.insert_pane(PanelId::SystemStats);
    let left_column = tiles.insert_vertical_tile(vec![devices, gauges, system_stats]);

    let network = tiles.insert_pane(PanelId::Network);
    let protocols = tiles.insert_pane(PanelId::Protocols);
    let right_column = tiles.insert_vertical_tile(vec![network, protocols]);

    let root = tiles.insert_horizontal_tile(vec![left_column, right_column]);

    set_linear_shares(
        &mut tiles,
        root,
        &[(left_column, COL_SHARE), (right_column, COL_SHARE)],
    );
    set_linear_shares(
        &mut tiles,
        left_column,
        &[
            (devices, COL_SHARE),
            (gauges, GAUGES_SHARE),
            (system_stats, STORAGE_DETAILS_SHARE),
        ],
    );
    set_linear_shares(
        &mut tiles,
        gauges,
        &[(ram, COL_SHARE), (storage, COL_SHARE)],
    );
    set_linear_shares(
        &mut tiles,
        right_column,
        &[(network, COL_SHARE), (protocols, COL_SHARE)],
    );

    Tree::new("android_dashboard_overview_tiles", root, tiles)
}

/// Cây ô cho tab Log & Lỗi: logcat, lỗi logcat và phân tích AI.
pub fn create_logs_tree() -> Tree<PanelId> {
    let mut tiles = egui_tiles::Tiles::default();

    let logcat_all = tiles.insert_pane(PanelId::LogcatAll);
    let logcat_errors = tiles.insert_pane(PanelId::LogcatErrors);
    let insight = tiles.insert_pane(PanelId::Insight);
    let right_column = tiles.insert_vertical_tile(vec![logcat_errors, insight]);

    let root = tiles.insert_horizontal_tile(vec![logcat_all, right_column]);

    set_linear_shares(
        &mut tiles,
        root,
        &[(logcat_all, LOGCAT_SHARE), (right_column, COL_SHARE)],
    );
    set_linear_shares(
        &mut tiles,
        right_column,
        &[(logcat_errors, LOGCAT_SHARE), (insight, INSIGHT_SHARE)],
    );

    Tree::new("android_dashboard_logs_tiles", root, tiles)
}

fn set_linear_shares(
    tiles: &mut egui_tiles::Tiles<PanelId>,
    container_id: egui_tiles::TileId,
    shares: &[(egui_tiles::TileId, f32)],
) {
    let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(linear))) =
        tiles.get_mut(container_id)
    else {
        return;
    };

    for (tile_id, share) in shares {
        linear.shares.set_share(*tile_id, *share);
    }
}

pub fn show(ui: &mut egui::Ui, tree: &mut Tree<PanelId>, app: &mut App, layout_dirty: &mut bool) {
    let mut behavior = AppTilesBehavior { app, layout_dirty };
    tree.ui(&mut behavior, ui);
}

struct AppTilesBehavior<'a> {
    app: &'a mut App,
    layout_dirty: &'a mut bool,
}

impl Behavior<PanelId> for AppTilesBehavior<'_> {
    fn tab_title_for_pane(&mut self, pane: &PanelId) -> egui::WidgetText {
        pane.title().into()
    }

    fn on_edit(&mut self, _action: egui_tiles::EditAction) {
        *self.layout_dirty = true;
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        theme::PANEL_GAP
    }

    fn resize_stroke(&self, _style: &egui::Style, resize_state: ResizeState) -> egui::Stroke {
        match resize_state {
            ResizeState::Idle => egui::Stroke::NONE,
            ResizeState::Hovering | ResizeState::Dragging => {
                egui::Stroke::new(1.0, theme::colors::PANEL_SPLITTER_HOVER)
            }
        }
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut PanelId) -> UiResponse {
        let device_selected = self.app.roster.selected_serial.is_some();
        match pane {
            PanelId::Devices => {
                if let Some(action) =
                    panels::devices::devices_panel(ui, &self.app.roster, pane.icon())
                {
                    match action {
                        DevicesAction::Refresh => self.app.refresh_devices(),
                        DevicesAction::Select(serial) => self.app.select_device(serial),
                    }
                }
            }
            PanelId::Ram => {
                ui_elements::panel(ui, pane.icon(), pane.title(), |ui| {
                    if !device_selected {
                        ui_elements::panel_loading(ui);
                    } else {
                        panels::ram::ram_gauge_panel(ui, &self.app.metrics);
                    }
                });
            }
            PanelId::Storage => {
                ui_elements::panel(ui, pane.icon(), pane.title(), |ui| {
                    if !device_selected {
                        ui_elements::panel_loading(ui);
                    } else {
                        panels::storage::storage_gauge_panel(ui, &self.app.metrics);
                    }
                });
            }
            PanelId::LogcatAll => {
                let mut show_timestamps = self.app.logcat.show_timestamps;
                let mut auto_scroll = self.app.logcat.auto_update_feed;
                ui_elements::panel_with_footer(
                    ui,
                    pane.icon(),
                    pane.title(),
                    |_| {},
                    |ui, auto_scroll| {
                        if !device_selected {
                            ui_elements::panel_loading(ui);
                            0
                        } else {
                            panels::logcat::logcat_all_panel(ui, &mut self.app.logcat, auto_scroll)
                        }
                    },
                    &mut auto_scroll,
                    Some(&mut show_timestamps),
                );
                self.app.logcat.show_timestamps = show_timestamps;
                self.app.logcat.auto_update_feed = auto_scroll;
            }
            PanelId::Insight => {
                let mut auto_scroll = self.app.insight.auto_update_feed;
                ui_elements::panel_with_footer(
                    ui,
                    pane.icon(),
                    pane.title(),
                    |ui| {
                        if ui_elements::header_action(
                            ui,
                            i18n::t("panel.action.configure"),
                            i18n::t("panel.action.configure_hover"),
                        ) {
                            self.app.settings.open_from(self.app.insight.config());
                        }
                    },
                    |ui, auto_scroll| {
                        if !device_selected {
                            ui_elements::panel_loading(ui);
                            0
                        } else {
                            panels::insight::insight_panel(ui, &self.app.insight, auto_scroll)
                        }
                    },
                    &mut auto_scroll,
                    None,
                );
                self.app.insight.auto_update_feed = auto_scroll;
            }
            PanelId::LogcatErrors => {
                let mut show_timestamps = self.app.logcat_errors.show_timestamps;
                let mut auto_scroll = self.app.logcat_errors.auto_update_feed;
                ui_elements::panel_with_footer(
                    ui,
                    pane.icon(),
                    pane.title(),
                    |_| {},
                    |ui, auto_scroll| {
                        if !device_selected {
                            ui_elements::panel_loading(ui);
                            0
                        } else {
                            panels::logcat::logcat_errors_panel(
                                ui,
                                &mut self.app.logcat_errors,
                                auto_scroll,
                            )
                        }
                    },
                    &mut auto_scroll,
                    Some(&mut show_timestamps),
                );
                self.app.logcat_errors.show_timestamps = show_timestamps;
                self.app.logcat_errors.auto_update_feed = auto_scroll;
            }
            PanelId::SystemStats => {
                ui_elements::panel_with_header_actions(
                    ui,
                    pane.icon(),
                    pane.title(),
                    |ui| {
                        if ui_elements::header_action(
                            ui,
                            i18n::t("panel.action.backup"),
                            i18n::t("panel.action.backup_hover"),
                        ) {
                            self.app.requested_tab = Some(MainTab::Tools);
                        }
                        if ui_elements::header_action(
                            ui,
                            i18n::t("panel.action.cleanup"),
                            i18n::t("panel.action.cleanup_hover"),
                        ) {
                            self.app.cleanup.open();
                            self.app.requested_tab = Some(MainTab::Apps);
                        }
                    },
                    |ui| {
                        if !device_selected {
                            ui_elements::panel_loading(ui);
                        } else {
                            panels::storage::storage_usage_panel(ui, &self.app.metrics);
                        }
                    },
                );
            }
            PanelId::Network => {
                ui_elements::panel(ui, pane.icon(), pane.title(), |ui| {
                    if !device_selected {
                        ui_elements::panel_loading(ui);
                    } else {
                        panels::network::network_panel(ui, &self.app.metrics);
                    }
                });
            }
            PanelId::Protocols => {
                ui_elements::panel(ui, pane.icon(), pane.title(), |ui| {
                    if !device_selected {
                        ui_elements::panel_loading(ui);
                    } else {
                        panels::traffic::protocols_panel(ui, &self.app.metrics);
                    }
                });
            }
        }

        UiResponse::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_trees_json_round_trip() {
        for original in [create_overview_tree(), create_logs_tree()] {
            let json = serde_json::to_string(&original).expect("serialize default tree");
            let restored: Tree<PanelId> =
                serde_json::from_str(&json).expect("deserialize default tree");
            assert_eq!(original, restored);
        }
    }

    #[test]
    fn panel_id_json_is_the_variant_name() {
        let json = serde_json::to_string(&PanelId::LogcatErrors).expect("serialize");
        assert_eq!(json, "\"LogcatErrors\"");
        let restored: PanelId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, PanelId::LogcatErrors);
    }

    #[test]
    fn default_tree_pretty_json_has_panes_and_shares() {
        let json = serde_json::to_string_pretty(&create_overview_tree()).expect("pretty json");
        assert!(
            json.contains("\"Pane\": \"Devices\"") || json.contains("\"Pane\":\"Devices\""),
            "missing Devices pane: {json}"
        );
        assert!(
            json.contains("\"Linear\""),
            "missing Linear container: {json}"
        );
        assert!(json.contains("\"shares\""), "missing shares: {json}");
    }
}
