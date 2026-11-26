use ai_insight::{InsightConfig, InsightEngine};
use eframe::egui;

use crate::i18n;
use crate::insight_store;
use crate::theme::{self, colors};

/// Label column + spacing + text field width, matching the grid below.
const AI_PROVIDER_DIALOG_WIDTH: f32 = 540.0;
const AI_PROVIDER_FIELD_WIDTH: f32 = 340.0;

/// Gap from the window top/right edges to the dialog.
const AI_PROVIDER_DIALOG_EDGE: f32 = 24.0;

const OPENCODE_API_URL: &str = "https://opencode.ai/inference/openai/v1";
const OPENAI_URL: &str = "https://api.openai.com/v1";
const DEEPSEEK_URL: &str = "https://api.deepseek.com";

pub enum SettingsAction {
    Save(InsightConfig),
    Cancel,
}

/// One-click presets for the providers used with Codex and OpenCode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Preset {
    Codex,
    OpenCode,
    OpenCodeApi,
    OpenAi,
    DeepSeek,
    #[default]
    Custom,
}

impl Preset {
    const ALL: [Self; 6] = [
        Self::Codex,
        Self::OpenCode,
        Self::OpenCodeApi,
        Self::OpenAi,
        Self::DeepSeek,
        Self::Custom,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Codex => i18n::t("settings.preset_codex"),
            Self::OpenCode => i18n::t("settings.preset_opencode"),
            Self::OpenCodeApi => i18n::t("settings.preset_opencode_api"),
            Self::OpenAi => i18n::t("settings.preset_openai"),
            Self::DeepSeek => i18n::t("settings.preset_deepseek"),
            Self::Custom => i18n::t("settings.preset_custom"),
        }
    }

    fn engine(self) -> InsightEngine {
        match self {
            Self::Codex => InsightEngine::CodexCli,
            Self::OpenCode => InsightEngine::OpenCodeCli,
            Self::OpenCodeApi | Self::OpenAi | Self::DeepSeek | Self::Custom => InsightEngine::Http,
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::OpenCodeApi => OPENCODE_API_URL,
            Self::OpenAi => OPENAI_URL,
            Self::DeepSeek => DEEPSEEK_URL,
            Self::Codex | Self::OpenCode | Self::Custom => "",
        }
    }

    fn model(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode/big-pickle",
            Self::OpenCodeApi => "glm-5.3-flash",
            Self::OpenAi => "gpt-5.4-mini",
            Self::DeepSeek => "deepseek-chat",
            Self::Codex | Self::Custom => "",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Codex => i18n::t("settings.desc_codex"),
            Self::OpenCode => i18n::t("settings.desc_opencode"),
            Self::OpenCodeApi => i18n::t("settings.desc_opencode_api"),
            Self::OpenAi => i18n::t("settings.desc_openai"),
            Self::DeepSeek => i18n::t("settings.desc_deepseek"),
            Self::Custom => i18n::t("settings.desc_custom"),
        }
    }

    fn from_config(config: &InsightConfig) -> Self {
        match config.engine {
            InsightEngine::CodexCli => return Self::Codex,
            InsightEngine::OpenCodeCli => return Self::OpenCode,
            InsightEngine::Http => {}
        }
        let base = config.base_url.trim().trim_end_matches('/');
        if base == OPENCODE_API_URL {
            Self::OpenCodeApi
        } else if base == OPENAI_URL {
            Self::OpenAi
        } else if base == DEEPSEEK_URL {
            Self::DeepSeek
        } else {
            Self::Custom
        }
    }
}

struct Status {
    text: String,
    ok: bool,
}

/// Draft fields for the AI provider dialog.
#[derive(Default)]
pub struct SettingsDialog {
    open: bool,
    preset: Preset,
    base_url: String,
    model: String,
    api_key: String,
    status: Option<Status>,
}

impl SettingsDialog {
    /// Copies live config into drafts and opens the dialog.
    pub fn open_from(&mut self, config: &InsightConfig) {
        self.preset = Preset::from_config(config);
        self.base_url = config.base_url.clone();
        self.model = config.model.clone();
        self.api_key = config.api_key.clone();
        self.status = None;
        self.open = true;
    }

    /// Applies the preset's endpoint/model, keeping any typed API key.
    fn set_preset(&mut self, preset: Preset) {
        self.preset = preset;
        if preset == Preset::Custom {
            return;
        }
        self.base_url = preset.base_url().to_string();
        self.model = preset.model().to_string();
    }

    /// Fills fields from Codex/OpenCode auth files on this machine.
    fn detect_local(&mut self) {
        let found = insight_store::detect_local_credentials();

        if let Some(key) = found.opencode_api_key.clone() {
            self.api_key = key;
        } else if let Some(key) = found.codex_api_key.clone() {
            self.api_key = key;
        }

        // Prefer the engine that already works without extra setup.
        if found.codex_chatgpt {
            self.set_preset(Preset::Codex);
        } else if found.opencode_api_key.is_some() {
            self.set_preset(Preset::OpenCodeApi);
        } else if found.codex_api_key.is_some() {
            self.set_preset(Preset::OpenAi);
        }

        self.status = Some(Status {
            text: found.summary(),
            ok: found.any_found(),
        });
    }
}

/// Draws the AI provider window when open. Returns Save or Cancel when the user finishes.
pub fn show(ctx: &egui::Context, dialog: &mut SettingsDialog) -> Option<SettingsAction> {
    if !dialog.open {
        return None;
    }

    let screen = ctx.screen_rect();
    let pos = egui::pos2(
        screen.max.x - AI_PROVIDER_DIALOG_EDGE - AI_PROVIDER_DIALOG_WIDTH,
        theme::TITLE_BAR_HEIGHT + AI_PROVIDER_DIALOG_EDGE,
    );

    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    let mut detect = false;
    egui::Window::new(i18n::t("settings.title"))
        .id(egui::Id::new("ai_provider_settings"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .fixed_pos(pos)
        .default_width(AI_PROVIDER_DIALOG_WIDTH)
        .min_width(AI_PROVIDER_DIALOG_WIDTH)
        .max_width(AI_PROVIDER_DIALOG_WIDTH)
        .default_height(0.0)
        .min_height(0.0)
        .frame(
            egui::Frame::default()
                .fill(colors::PANEL_BG)
                .stroke(egui::Stroke::new(1.0, colors::PANEL_BORDER))
                .corner_radius(egui::CornerRadius::same(theme::PANEL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(theme::PANEL_INNER_PADDING)),
        )
        .show(ctx, |ui| {
            egui::Grid::new("ai_provider_fields")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label(i18n::t("settings.provider"));
                    let mut preset = dialog.preset;
                    egui::ComboBox::from_id_salt("ai_provider_preset")
                        .selected_text(preset.label())
                        .width(AI_PROVIDER_FIELD_WIDTH)
                        .show_ui(ui, |ui| {
                            for option in Preset::ALL {
                                ui.selectable_value(&mut preset, option, option.label());
                            }
                        });
                    if preset != dialog.preset {
                        dialog.set_preset(preset);
                    }
                    ui.end_row();

                    ui.label(match dialog.preset {
                        Preset::Codex => i18n::t("settings.model_optional"),
                        _ => i18n::t("settings.model"),
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.model)
                            .desired_width(AI_PROVIDER_FIELD_WIDTH)
                            .hint_text(
                                egui::RichText::new(match dialog.preset {
                                    Preset::Codex => i18n::t("settings.hint_empty_codex"),
                                    Preset::OpenCode => i18n::t("settings.hint_provider_model"),
                                    _ => i18n::t("settings.hint_model_name"),
                                })
                                .color(colors::PLACEHOLDER_TEXT),
                            ),
                    );
                    ui.end_row();

                    if dialog.preset.engine() == InsightEngine::Http {
                        ui.label(i18n::t("settings.base_url"));
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.base_url)
                                .desired_width(AI_PROVIDER_FIELD_WIDTH)
                                .hint_text(
                                    egui::RichText::new("https://api.example.com")
                                        .color(colors::PLACEHOLDER_TEXT),
                                ),
                        );
                        ui.end_row();

                        ui.label(i18n::t("settings.api_key"));
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.api_key)
                                .desired_width(AI_PROVIDER_FIELD_WIDTH)
                                .password(true)
                                .hint_text(
                                    egui::RichText::new("sk-…").color(colors::PLACEHOLDER_TEXT),
                                ),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(theme::ITEM_SPACING_Y);
            ui.label(
                egui::RichText::new(dialog.preset.hint())
                    .small()
                    .color(colors::FOOTER_TEXT),
            );

            if let Some(status) = &dialog.status {
                ui.add_space(theme::ITEM_SPACING_Y);
                let color = if status.ok {
                    colors::LOG_INFO
                } else {
                    colors::LOG_WARNING
                };
                ui.label(egui::RichText::new(&status.text).small().color(color));
            }

            ui.add_space(theme::ITEM_SPACING_Y);
            ui.separator();
            ui.add_space(theme::ITEM_SPACING_Y);
            ui.scope(|ui| {
                style_dialog_buttons(ui);
                ui.horizontal(|ui| {
                    if ui.button(i18n::t("settings.detect_local")).clicked() {
                        detect = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if ui.button(i18n::t("common.save")).clicked() {
                            save = true;
                        }
                        if ui.button(i18n::t("common.cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        });

    if detect {
        dialog.detect_local();
    }
    if save {
        dialog.open = false;
        return Some(SettingsAction::Save(InsightConfig {
            engine: dialog.preset.engine(),
            base_url: dialog.base_url.clone(),
            model: dialog.model.clone(),
            api_key: dialog.api_key.clone(),
        }));
    }
    if cancel || !open {
        dialog.open = false;
        return Some(SettingsAction::Cancel);
    }
    None
}

/// Panel-matched button chrome: body text, panel fill/border, selection fill when pressed.
fn style_dialog_buttons(ui: &mut egui::Ui) {
    ui.style_mut().text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(theme::FONT_BODY, egui::FontFamily::Proportional),
    );

    let stroke = egui::Stroke::new(1.0, colors::PANEL_BORDER);
    let widgets = &mut ui.visuals_mut().widgets;
    for style in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
    ] {
        style.weak_bg_fill = colors::PANEL_BG;
        style.bg_fill = colors::PANEL_BG;
        style.bg_stroke = stroke;
        style.expansion = 0.0;
    }
    widgets.active.weak_bg_fill = colors::SELECTION;
    widgets.active.bg_fill = colors::SELECTION;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_config(base_url: &str) -> InsightConfig {
        InsightConfig {
            engine: InsightEngine::Http,
            base_url: base_url.into(),
            ..InsightConfig::default()
        }
    }

    #[test]
    fn preset_detects_cli_engines() {
        let mut config = http_config("");
        config.engine = InsightEngine::CodexCli;
        assert_eq!(Preset::from_config(&config), Preset::Codex);
        config.engine = InsightEngine::OpenCodeCli;
        assert_eq!(Preset::from_config(&config), Preset::OpenCode);
    }

    #[test]
    fn preset_detects_known_http_hosts() {
        assert_eq!(
            Preset::from_config(&http_config(OPENCODE_API_URL)),
            Preset::OpenCodeApi
        );
        assert_eq!(
            Preset::from_config(&http_config("https://api.openai.com/v1/")),
            Preset::OpenAi
        );
        assert_eq!(
            Preset::from_config(&http_config(DEEPSEEK_URL)),
            Preset::DeepSeek
        );
        assert_eq!(
            Preset::from_config(&http_config("https://example.com/v1")),
            Preset::Custom
        );
    }

    #[test]
    fn set_preset_fills_endpoint_and_keeps_key() {
        let mut dialog = SettingsDialog {
            api_key: "sk-keep".into(),
            ..Default::default()
        };
        dialog.set_preset(Preset::OpenCodeApi);
        assert_eq!(dialog.base_url, OPENCODE_API_URL);
        assert_eq!(dialog.model, "glm-5.3-flash");
        assert_eq!(dialog.api_key, "sk-keep");
        assert_eq!(dialog.preset.engine(), InsightEngine::Http);
    }

    #[test]
    fn custom_preset_keeps_typed_values() {
        let mut dialog = SettingsDialog {
            base_url: "https://example.com/v1".into(),
            model: "my-model".into(),
            ..SettingsDialog::default()
        };
        dialog.set_preset(Preset::Custom);
        assert_eq!(dialog.base_url, "https://example.com/v1");
        assert_eq!(dialog.model, "my-model");
        assert_eq!(dialog.preset.engine(), InsightEngine::Http);
    }
}
