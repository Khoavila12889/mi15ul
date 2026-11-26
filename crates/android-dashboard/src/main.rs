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
mod theme;
mod ui_elements;

use adb_client::Adb;
use egui_tiles::Tree;

use crate::app::App;
use crate::layout::PanelId;
use crate::panels::settings::SettingsAction;

struct TerminalApp {
    tools: panels::advanced_tools::AdvancedTools,
    inner: App,
    layout_tree: Tree<PanelId>,
    layout_saver: layout_store::LayoutSaver,
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
        Self {
            tools,
            inner,
            layout_tree: layout_store::load_or_default(),
            layout_saver: layout_store::LayoutSaver::default(),
        }
    }
}

impl eframe::App for TerminalApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        self.inner.tick(ctx);

        // Global drag-and-drop for .apk / .apks / .xapk / .apkm when no modal drop window owns it.
        if !self.inner.flash.open && !self.inner.apk.open {
            let apk_paths: Vec<std::path::PathBuf> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .filter(|p| adb_client::apk::classify_apk_path(p).is_some())
                    .collect()
            });
            if !apk_paths.is_empty() {
                self.inner.apk.enqueue_paths(apk_paths);
                self.inner.apk.open = true;
            }
        }

        let bar_frame = eframe::egui::Frame::default()
            .fill(theme::colors::PANEL_BG)
            .inner_margin(eframe::egui::Margin::symmetric(12, 7));
        eframe::egui::TopBottomPanel::top("dashboard_actions")
            .frame(bar_frame)
            .show_separator_line(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let apk_q = self.inner.apk.queue_len();
                    let apk_label = if apk_q > 0 {
                        let count = apk_q.to_string();
                        i18n::tf("app.toolbar_install_apk_count", &[("count", &count)])
                    } else {
                        i18n::t("app.toolbar_install_apk").to_string()
                    };
                    if ui_elements::toolbar_button(
                        ui,
                        &apk_label,
                        self.inner.apk.open || self.inner.apk.is_busy(),
                        Some(theme::colors::ACCENT_GREEN),
                    )
                    .clicked()
                    {
                        self.inner.apk.open = true;
                    }
                    if ui_elements::toolbar_button(
                        ui,
                        i18n::t("app.toolbar_advanced_tools"),
                        self.tools.open,
                        None,
                    )
                    .clicked()
                    {
                        if !self.tools.open && !self.inner.flash.is_busy() {
                            self.inner.flash.refresh_fastboot_devices();
                        }
                        self.tools.open = true;
                    }
                    if ui_elements::toolbar_button(
                        ui,
                        i18n::t("app.toolbar_backup_transfer"),
                        self.inner.backup.open,
                        None,
                    )
                    .clicked()
                    {
                        self.inner.backup.open = true;
                    }
                    if ui_elements::toolbar_button(
                        ui,
                        i18n::t("app.toolbar_flash_rom"),
                        self.inner.flash.open,
                        None,
                    )
                    .clicked()
                    {
                        self.inner.flash.open = true;
                        self.inner.flash.refresh_fastboot_devices();
                    }
                    if ui_elements::toolbar_button(
                        ui,
                        i18n::t("app.toolbar_stock_flash_lock"),
                        false,
                        Some(theme::colors::ACCENT_AMBER),
                    )
                    .clicked()
                    {
                        self.inner
                            .flash
                            .open_tab(panels::flash::FlashTab::LockBootloader);
                    }
                    if ui_elements::toolbar_button(
                        ui,
                        i18n::t("app.toolbar_hyperos"),
                        self.inner.hyperos.open || self.inner.hyperos.is_busy(),
                        Some(theme::colors::ACCENT_BLUE),
                    )
                    .clicked()
                    {
                        self.inner.hyperos.open = true;
                    }
                    if ui_elements::toolbar_button(
                        ui,
                        i18n::t("app.toolbar_ai_provider"),
                        false,
                        None,
                    )
                    .clicked()
                    {
                        self.inner.settings.open_from(self.inner.insight.config());
                    }

                    ui.with_layout(
                        eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                        |ui| {
                            if let Some(serial) = &self.inner.roster.selected_serial {
                                ui_elements::status_pill(
                                    ui,
                                    &format!("● {serial}"),
                                    theme::colors::ACCENT_GREEN,
                                );
                            } else {
                                ui_elements::status_pill(
                                    ui,
                                    i18n::t("app.no_adb_device"),
                                    theme::colors::TEXT_MUTED,
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
                self.layout_tree = layout::create_default_tree();
                if layout_store::save(&self.layout_tree) {
                    self.layout_saver.clear_dirty();
                } else {
                    self.layout_saver.mark_edit();
                }
            }
            if title.open_ai_settings {
                self.inner.settings.open_from(self.inner.insight.config());
            }
        }

        if let Some(action) = panels::settings::show(ctx, &mut self.inner.settings) {
            match action {
                SettingsAction::Save(config) => {
                    insight_store::save(&config);
                    self.inner.insight.set_config(config);
                }
                SettingsAction::Cancel => {}
            }
        }

        let mut layout_dirty = false;
        eframe::egui::CentralPanel::default()
            .frame(ui_elements::shell_frame(ctx))
            .show(ctx, |ui| {
                ui_elements::canvas_margin_frame().show(ui, |ui| {
                    layout::show(
                        ui,
                        &mut self.layout_tree,
                        &mut self.inner,
                        &mut layout_dirty,
                    );
                });
            });
        self.inner.cleanup.show(
            ctx,
            self.inner.roster.selected_serial.as_deref(),
            &self.inner.metrics.app_storage,
        );
        self.inner.backup.show(ctx);
        self.inner.apk.show(ctx);
        self.inner.hyperos.show(ctx);
        self.inner
            .flash
            .show(ctx, self.inner.roster.selected_serial.as_deref());
        self.tools.show(
            ctx,
            self.inner.roster.selected_serial.as_deref(),
            &mut self.inner.flash,
            &mut self.inner.apk,
            &mut self.inner.hyperos,
        );
        if layout_dirty {
            self.layout_saver.mark_edit();
        }
        self.layout_saver.tick(ctx, &self.layout_tree);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.layout_saver.flush(&self.layout_tree);
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
