mod app;
mod audit;
mod format;
mod i18n;
mod insight;
mod insight_store;
mod layout;
mod layout_store;
mod logcat_pane;
#[cfg(target_os = "macos")]
mod macos;
mod metrics;
mod panels;
mod roster;
mod session;
mod tabs;
mod theme;
mod ui_elements;

use adb_client::{Adb, DeviceState};
use egui_tiles::Tree;

use crate::app::App;
use crate::layout::PanelId;
use crate::panels::settings::SettingsAction;
use crate::tabs::MainTab;

struct TerminalApp {
    tools: panels::advanced_tools::AdvancedTools,
    inner: App,
    overview_tree: Tree<PanelId>,
    logs_tree: Tree<PanelId>,
    layout_saver: layout_store::LayoutSaver,
    tab: MainTab,
}

impl TerminalApp {
    fn new(adb_error: Option<String>) -> Self {
        let should_refresh = adb_error.is_none();
        let tools = panels::advanced_tools::AdvancedTools::default();
        let mut inner = App::new(adb_error, insight_store::load_or_default());
        let (adb_path, fb_path) = tools.tool_paths();
        inner.flash.sync_tool_paths(adb_path, fb_path);
        inner.apk.sync_adb_path(adb_path);
        inner.hyperos.sync_adb_path(adb_path);
        if should_refresh {
            inner.refresh_devices();
        }
        let (overview_tree, logs_tree) = layout_store::load_or_default();
        Self {
            tools,
            inner,
            overview_tree,
            logs_tree,
            layout_saver: layout_store::LayoutSaver::default(),
            tab: layout_store::load_selected_tab(),
        }
    }

    /// Switches the active main tab, persisting the choice. Entering the
    /// Flash tab refreshes the fastboot device list like the old window did.
    fn switch_tab(&mut self, tab: MainTab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        layout_store::save_selected_tab(tab);
        if tab == MainTab::Flash && !self.inner.flash.is_busy() {
            self.inner.flash.refresh_fastboot_devices();
        }
    }
}

impl eframe::App for TerminalApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        self.inner.tick(ctx);

        if let Some(tab) = self.inner.requested_tab.take() {
            self.switch_tab(tab);
        }

        // Global drag-and-drop routing: APK bundles go to the Apps tab,
        // anything else (ROM folders, images, manifests) to the Flash tab,
        // whose own drop handler consumes the files this same frame.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            let (apk_paths, rest): (Vec<_>, Vec<_>) = dropped
                .into_iter()
                .partition(|p| adb_client::apk::classify_apk_path(p).is_some());
            if !apk_paths.is_empty() {
                self.inner.apk.enqueue_paths(apk_paths);
                self.switch_tab(MainTab::Apps);
            } else if !rest.is_empty() {
                self.switch_tab(MainTab::Flash);
            }
        }

        let bar_frame = eframe::egui::Frame::default()
            .fill(theme::colors::PANEL_BG)
            .inner_margin(eframe::egui::Margin::symmetric(12, 8));
        eframe::egui::TopBottomPanel::top("dashboard_actions")
            .frame(bar_frame)
            .show_separator_line(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        eframe::egui::RichText::new("◆ Android Dashboard")
                            .strong()
                            .color(theme::colors::ACCENT_CYAN),
                    );
                    ui.separator();
                    ui.label(
                        eframe::egui::RichText::new(i18n::t("panel.devices"))
                            .strong()
                            .color(theme::colors::OFF_WHITE),
                    );
                    // Shared device selector: same roster/selection flow the
                    // Devices panel uses, available from every tab.
                    let mut picked: Option<String> = None;
                    let selected_text = match &self.inner.roster.selected_serial {
                        Some(serial) => {
                            let model = self
                                .inner
                                .roster
                                .devices
                                .iter()
                                .find(|d| &d.serial == serial)
                                .map(|d| d.model.clone())
                                .filter(|m| !m.is_empty());
                            match model {
                                Some(model) => format!("{model} ({serial})"),
                                None => serial.clone(),
                            }
                        }
                        None => i18n::t("app.no_adb_device").to_string(),
                    };
                    eframe::egui::ComboBox::from_id_salt("device_selector")
                        .selected_text(selected_text)
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            if self.inner.roster.devices.is_empty() {
                                ui.label(i18n::t("devices.none_found"));
                            }
                            for device in &self.inner.roster.devices {
                                let name = if device.model.is_empty() {
                                    device.serial.clone()
                                } else {
                                    format!("{} ({})", device.model, device.serial)
                                };
                                if device.state == DeviceState::Device {
                                    let is_selected = self.inner.roster.selected_serial
                                        .as_deref()
                                        == Some(device.serial.as_str());
                                    if ui.selectable_label(is_selected, name).clicked()
                                        && !is_selected
                                    {
                                        picked = Some(device.serial.clone());
                                    }
                                } else {
                                    ui.add_enabled_ui(false, |ui| {
                                        ui.label(format!(
                                            "{name} — {}",
                                            i18n::device_state(&device.state)
                                        ));
                                    });
                                }
                            }
                        });
                    if let Some(serial) = picked {
                        self.inner.select_device(serial);
                    }
                    if self.inner.roster.list_job_in_flight {
                        ui.spinner();
                        ui.label(i18n::t("ui.devices.refreshing"));
                    } else if ui_elements::pulsing_cta_button(
                        ui,
                        &format!("↻ {}", i18n::t("app.refresh_devices")),
                        theme::colors::ACCENT_BLUE,
                        self.inner.roster.selected_serial.is_none(),
                    )
                    .clicked()
                    {
                        self.inner.refresh_devices();
                    }

                    ui.with_layout(
                        eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                        |ui| {
                            let audit_dir = audit::audit_dir();
                            if ui
                                .button("Audit Log")
                                .on_hover_text(format!("Mở thư mục nhật ký kiểm toán: {}", audit_dir.display()))
                                .clicked()
                            {
                                ui_elements::open_folder_in_os(&audit_dir);
                            }
                            if let Some(serial) = &self.inner.roster.selected_serial {
                                ui_elements::pulsing_status_pill(
                                    ui,
                                    &format!("● {serial}"),
                                    theme::colors::ACCENT_GREEN,
                                    true,
                                );
                            } else {
                                ui_elements::pulsing_status_pill(
                                    ui,
                                    i18n::t("app.no_adb_device"),
                                    theme::colors::ACCENT_AMBER,
                                    true,
                                );
                            }
                        },
                    );
                });
            });

        #[cfg(target_os = "macos")]
        {
            let title = ui_elements::title_bar(ctx, frame);
            if title.reset_layout {
                self.overview_tree = layout::create_overview_tree();
                self.logs_tree = layout::create_logs_tree();
                if layout_store::save(&self.overview_tree, &self.logs_tree) {
                    self.layout_saver.clear_dirty();
                } else {
                    self.layout_saver.mark_edit();
                }
            }
            if title.open_ai_settings {
                self.inner.settings.open_from(self.inner.insight.config());
                self.switch_tab(MainTab::Logs);
            }
        }

        let tab_frame = eframe::egui::Frame::default()
            .fill(theme::colors::PANEL_BG)
            .inner_margin(eframe::egui::Margin::symmetric(12, 6));
        eframe::egui::TopBottomPanel::top("dashboard_tabs")
            .frame(tab_frame)
            .show_separator_line(true)
            .show(ctx, |ui| {
                let apk_q = self.inner.apk.queue_len();
                let items: Vec<(MainTab, ui_elements::NavTabItem<'_>)> = MainTab::ALL
                    .iter()
                    .map(|&tab| {
                        let (badge, busy) = match tab {
                            MainTab::Apps => (
                                (apk_q > 0).then(|| apk_q.to_string()),
                                self.inner.apk.is_busy(),
                            ),
                            MainTab::HyperOs => (None, self.inner.hyperos.is_busy()),
                            MainTab::Flash => (None, self.inner.flash.is_busy()),
                            _ => (None, false),
                        };
                        (
                            tab,
                            ui_elements::NavTabItem {
                                label: tab.title(),
                                icon: tab.icon(),
                                badge,
                                busy,
                            },
                        )
                    })
                    .collect();
                let mut current = self.tab;
                ui_elements::main_nav_bar(ui, &mut current, &items);
                if current != self.tab {
                    self.switch_tab(current);
                }
            });

        // AI provider settings live as a docked sheet inside the Log & Lỗi
        // tab (opened from the Insight panel's Configure action).
        if self.tab == MainTab::Logs && self.inner.settings.is_open() {
            eframe::egui::TopBottomPanel::bottom("ai_settings_sheet")
                .frame(bar_frame)
                .resizable(false)
                .show(ctx, |ui| {
                    eframe::egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            ui_elements::tool_card(ui, |ui| {
                                ui_elements::section_header(ui, i18n::t("settings.title"), "");
                                ui.add_space(4.0);
                                if let Some(action) =
                                    panels::settings::show_in(ui, &mut self.inner.settings)
                                {
                                    match action {
                                        SettingsAction::Save(config) => {
                                            insight_store::save(&config);
                                            self.inner.insight.set_config(config);
                                        }
                                        SettingsAction::Cancel => {}
                                    }
                                }
                            });
                        });
                });
        }

        let mut layout_dirty = false;
        let mut goto: Option<MainTab> = None;
        eframe::egui::CentralPanel::default()
            .frame(ui_elements::shell_frame(ctx))
            .show(ctx, |ui| {
                ui_elements::canvas_margin_frame().show(ui, |ui| match self.tab {
                    MainTab::Overview => {
                        layout::show(
                            ui,
                            &mut self.overview_tree,
                            &mut self.inner,
                            &mut layout_dirty,
                        );
                    }
                    MainTab::Logs => {
                        layout::show(
                            ui,
                            &mut self.logs_tree,
                            &mut self.inner,
                            &mut layout_dirty,
                        );
                    }
                    MainTab::Apps => {
                        eframe::egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let serial = self.inner.roster.selected_serial.clone();
                                self.inner.cleanup.show_in(
                                    ui,
                                    serial.as_deref(),
                                    &self.inner.metrics.app_storage,
                                );
                                ui.add_space(8.0);
                                self.inner.apk.show_in(ui, ctx);
                            });
                    }
                    MainTab::HyperOs => {
                        eframe::egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                self.inner.hyperos.show_in(ui, ctx);
                            });
                    }
                    MainTab::Flash => {
                        eframe::egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let serial = self.inner.roster.selected_serial.clone();
                                self.inner.flash.show_in(ui, ctx, serial.as_deref());
                            });
                    }
                    MainTab::Tools => {
                        eframe::egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let serial = self.inner.roster.selected_serial.clone();
                                if let Some(tab) = self.tools.show_in(
                                    ui,
                                    ctx,
                                    serial.as_deref(),
                                    &mut self.inner.flash,
                                    &mut self.inner.apk,
                                    &mut self.inner.hyperos,
                                ) {
                                    goto = Some(tab);
                                }
                                ui.add_space(8.0);
                                self.inner.backup.show_in(ui, ctx);
                            });
                    }
                });
            });
        if let Some(tab) = goto {
            self.switch_tab(tab);
        }
        if layout_dirty {
            self.layout_saver.mark_edit();
        }
        self.layout_saver
            .tick(ctx, &self.overview_tree, &self.logs_tree);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.layout_saver
            .flush(&self.overview_tree, &self.logs_tree);
        self.inner.shutdown();
    }
}

fn main() -> eframe::Result<()> {
    init_tracing();

    let adb_error = Adb::check_available().err().map(|e| e.to_string());

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/app.png"))
        .expect("app icon");

    #[allow(unused_mut)]
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size(theme::DEFAULT_WINDOW_SIZE)
        .with_title(i18n::t("app.title"))
        .with_icon(icon);
    #[cfg(target_os = "macos")]
    {
        // Content draws under the traffic lights; we paint a dark grey title strip.
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        i18n::t("app.title"),
        options,
        Box::new(|cc| {
            theme::configure(&cc.egui_ctx);

            Ok(Box::new(TerminalApp::new(adb_error)))
        }),
    )
}

/// Installs a stderr `tracing` subscriber.
/// Uses `RUST_LOG` when set; otherwise `ai_insight=info,android_dashboard=info`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("ai_insight=info,android_dashboard=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .init();
}
