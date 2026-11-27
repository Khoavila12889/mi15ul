//! Shared UI widgets: panel cards, footers, filter rows.

use std::time::Instant;

use eframe::egui::{self, Context, FontFamily, FontId, TextStyle, Ui};

use crate::i18n;
use crate::theme::{self, colors};

/// Show an error label using the current text color (panel body color when inside [`panel_body`]).
pub fn error_label(ui: &mut Ui, text: impl AsRef<str>) {
    ui.label(text.as_ref());
}

/// Run panel body content with a dedicated text color (headers stay on the default style).
pub fn panel_body<R>(ui: &mut Ui, color: egui::Color32, add_body: impl FnOnce(&mut Ui) -> R) -> R {
    let mut style = ui.style().as_ref().clone();
    style.visuals.override_text_color = Some(color);
    ui.scope(|ui| {
        ui.set_style(style);
        add_body(ui)
    })
    .inner
}

fn panel_separator(ui: &mut Ui) {
    let spacing = ui.spacing().item_spacing.y;
    let (rect, response) = ui.allocate_at_least(
        egui::vec2(ui.available_width(), spacing),
        egui::Sense::hover(),
    );
    if ui.is_rect_visible(rect) {
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            egui::Stroke::new(1.0, colors::PANEL_SEPARATOR),
        );
    }
    ui.advance_cursor_after_rect(response.rect);
}

/// Dark canvas behind panel cards.
pub fn shell_frame(_ctx: &Context) -> egui::Frame {
    egui::Frame::NONE.fill(colors::BG_EXTREME)
}

/// Insets panel cards from the window edge.
pub fn canvas_margin_frame() -> egui::Frame {
    egui::Frame::NONE.inner_margin(egui::Margin::same(theme::PANEL_CANVAS_MARGIN))
}

/// macOS title strip: same fill as the panel canvas; traffic lights stay native.
#[cfg(target_os = "macos")]
pub struct TitleBarAction {
    pub reset_layout: bool,
    pub open_ai_settings: bool,
}

/// macOS title strip: same fill as the panel canvas; traffic lights stay native.
#[cfg(target_os = "macos")]
pub fn title_bar(ctx: &Context, frame: &eframe::Frame) -> TitleBarAction {
    let over_traffic_lights = ctx.input(|i| {
        i.pointer.latest_pos().is_some_and(|pos| {
            pos.x < theme::TRAFFIC_LIGHTS_WIDTH && pos.y >= 0.0 && pos.y < theme::TITLE_BAR_HEIGHT
        })
    });
    crate::macos::sync_traffic_lights(frame, theme::TITLE_BAR_HEIGHT, over_traffic_lights);

    let mut action = TitleBarAction {
        reset_layout: false,
        open_ai_settings: false,
    };
    const SETTINGS_ICON: f32 = 16.0;
    const SETTINGS_STRIP: f32 = 40.0;

    egui::TopBottomPanel::top("os_title_bar")
        .exact_height(theme::TITLE_BAR_HEIGHT)
        .frame(egui::Frame::NONE.fill(colors::BG_EXTREME))
        .show_separator_line(false)
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                i18n::t("app.title"),
                FontId::new(theme::FONT_BODY, FontFamily::Proportional),
                colors::OFF_WHITE,
            );
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                egui::Stroke::new(1.0, colors::PANEL_SEPARATOR),
            );

            let settings_rect = egui::Rect::from_min_max(
                egui::pos2(rect.right() - SETTINGS_STRIP, rect.top()),
                rect.max,
            );
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(settings_rect), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(theme::PANEL_CANVAS_MARGIN as f32);
                    let image = egui::Image::new(theme::icons::cog())
                        .fit_to_exact_size(egui::vec2(SETTINGS_ICON, SETTINGS_ICON))
                        .show_loading_spinner(false)
                        .tint(colors::HEADER_ICON);
                    let menu = egui::menu::menu_custom_button(
                        ui,
                        egui::Button::image(image).frame(false),
                        |ui| {
                            if ui.button(i18n::t("ui.menu.configure_ai")).clicked() {
                                action.open_ai_settings = true;
                                ui.close_menu();
                            }
                            if ui.button(i18n::t("ui.menu.reset_layout")).clicked() {
                                action.reset_layout = true;
                                ui.close_menu();
                            }
                        },
                    );
                    menu.response.on_hover_text(i18n::t("ui.menu.settings"));
                });
            });

            let drag_rect = egui::Rect::from_min_max(
                egui::pos2(theme::TRAFFIC_LIGHTS_WIDTH, rect.top()),
                egui::pos2(settings_rect.left(), rect.bottom()),
            );
            let response = ui.interact(
                drag_rect,
                ui.id().with("title_bar_drag"),
                egui::Sense::click_and_drag(),
            );
            if response.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if response.double_clicked() {
                let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
        });
    action
}

fn panel_frame(ui: &Ui) -> egui::Frame {
    egui::Frame::default()
        .fill(colors::PANEL_BG)
        .stroke(egui::Stroke::new(1.0, colors::PANEL_BORDER))
        .corner_radius(egui::CornerRadius::same(theme::PANEL_CORNER_RADIUS))
        .inner_margin(panel_padding(ui))
}

fn panel_padding(ui: &Ui) -> egui::Margin {
    ui.style().spacing.window_margin
}

/// Space below a toolbar row (filter, etc.), matching panel padding.
pub fn section_gap(ui: &mut Ui) {
    ui.add_space(panel_padding(ui).bottom as f32);
}

const FOLDER_TAB_PAD_X: f32 = 12.0;
const FOLDER_TAB_PAD_Y: f32 = 5.0;
const FOLDER_TAB_GAP: f32 = 3.0;
const FOLDER_TAB_STROKE: f32 = 1.0;
const FOLDER_TAB_TOP_RADIUS: u8 = 4;

/// Folder-style tabs: the selected tab fill merges with the content below (open bottom + rail gap).
pub fn folder_tab_bar<T: Copy + PartialEq>(ui: &mut Ui, current: &mut T, tabs: &[(T, &str)]) {
    let font_id = FontId::new(theme::FONT_BODY, FontFamily::Proportional);
    let text_height = ui.fonts(|f| f.row_height(&font_id));
    let tab_height = text_height + FOLDER_TAB_PAD_Y * 2.0;
    let row_height = tab_height + FOLDER_TAB_STROKE;

    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height),
        egui::Sense::hover(),
    );

    let stroke = egui::Stroke::new(FOLDER_TAB_STROKE, colors::PANEL_BORDER);
    let top_rounding = egui::CornerRadius {
        nw: FOLDER_TAB_TOP_RADIUS,
        ne: FOLDER_TAB_TOP_RADIUS,
        sw: 0,
        se: 0,
    };

    let tab_top = row_rect.top();
    let tab_bottom = row_rect.top() + tab_height;
    let mut x = row_rect.left();
    let mut active_x: Option<egui::Rangef> = None;

    for (index, (value, label)) in tabs.iter().enumerate() {
        let text_size = ui.fonts(|f| {
            f.layout_no_wrap(label.to_string(), font_id.clone(), colors::OFF_WHITE)
                .size()
        });
        let tab_width = text_size.x + FOLDER_TAB_PAD_X * 2.0;
        let tab_rect = egui::Rect::from_min_max(
            egui::pos2(x, tab_top),
            egui::pos2(x + tab_width, tab_bottom),
        );

        let selected = *current == *value;
        let response = ui.interact(
            tab_rect,
            ui.id().with("folder_tab").with(index),
            egui::Sense::click(),
        );
        if response.clicked() {
            *current = *value;
        }

        let fill = if selected {
            colors::PANEL_BG
        } else if response.hovered() {
            colors::SELECTION
        } else {
            egui::Color32::TRANSPARENT
        };
        let text_color = if selected {
            colors::OFF_WHITE
        } else {
            colors::FOOTER_TEXT
        };

        let painter = ui.painter();
        painter.rect_filled(tab_rect, top_rounding, fill);
        painter.text(
            tab_rect.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font_id.clone(),
            text_color,
        );

        if selected {
            // Left, top, and right only — open bottom merges with the body,
            // with a crisp blue top highlight strip.
            let left = tab_rect.left() + 0.5;
            let right = tab_rect.right() - 0.5;
            let top = tab_rect.top() + 0.5;
            let bottom = tab_rect.bottom();
            painter.vline(left, top..=bottom, stroke);
            painter.hline(left..=right, top, stroke);
            painter.hline(
                (left + 2.0)..=(right - 2.0),
                top + 1.5,
                egui::Stroke::new(2.0, colors::ACCENT_BLUE),
            );
            painter.vline(right, top..=bottom, stroke);
            active_x = Some(tab_rect.x_range());
        } else {
            painter.rect_stroke(tab_rect, top_rounding, stroke, egui::StrokeKind::Inside);
        }

        x += tab_width + FOLDER_TAB_GAP;
    }

    // Baseline rail; leave a gap under the active tab so it reads as attached to the body.
    let rail_y = tab_bottom - 0.5;
    match active_x {
        Some(active) => {
            if active.min > row_rect.left() {
                ui.painter()
                    .hline(row_rect.left()..=active.min, rail_y, stroke);
            }
            if active.max < row_rect.right() {
                ui.painter()
                    .hline(active.max..=row_rect.right(), rail_y, stroke);
            }
        }
        None => {
            ui.painter().hline(row_rect.x_range(), rail_y, stroke);
        }
    }
}

const TAG_FILTER_PAD_X: i8 = 6;
const TAG_FILTER_PAD_Y: i8 = 4;
const TAG_FILTER_STROKE: f32 = 1.0;

/// Filter row with themed spacing underneath.
pub fn filter_row(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    let font_id = FontId::new(theme::FONT_SMALL, FontFamily::Proportional);
    let row_height = ui.fonts(|f| f.row_height(&font_id))
        + f32::from(TAG_FILTER_PAD_Y) * 2.0
        + TAG_FILTER_STROKE * 2.0;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), row_height),
        egui::Layout::left_to_right(egui::Align::Center),
        add_contents,
    );
    section_gap(ui);
}

/// Draws a tag-filter text field with the panel border stroke and small placeholder text.
pub fn tag_filter_input(
    ui: &mut Ui,
    text: &mut String,
    id_salt: impl std::hash::Hash,
) -> egui::Response {
    egui::Frame::default()
        .stroke(egui::Stroke::new(TAG_FILTER_STROKE, colors::PANEL_BORDER))
        .corner_radius(egui::CornerRadius::same(theme::PANEL_CORNER_RADIUS))
        .inner_margin(egui::Margin::symmetric(TAG_FILTER_PAD_X, TAG_FILTER_PAD_Y))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text(
                        egui::RichText::new(i18n::t("ui.tag_hint"))
                            .color(colors::PLACEHOLDER_TEXT),
                    )
                    .font(FontId::new(theme::FONT_SMALL, FontFamily::Proportional))
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .desired_width(ui.available_width())
                    .id_salt(id_salt),
            )
        })
        .inner
}

/// Stable palette index for a tag name.
pub fn tag_color_index(tag: &str) -> usize {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tag.to_lowercase().hash(&mut hasher);
    hasher.finish() as usize % colors::TAG_HIGHLIGHTS.len()
}

const BADGE_CORNER_RADIUS: u8 = 4;
const BADGE_PAD_X: i8 = 6;
const BADGE_PAD_Y: i8 = 2;
const BADGE_GAP: f32 = 4.0;

/// Removable tag-filter badge. Returns `true` when the remove control is clicked.
pub fn tag_filter_badge(ui: &mut Ui, label: &str, bg: egui::Color32, fg: egui::Color32) -> bool {
    let mut remove = false;
    egui::Frame::default()
        .fill(bg)
        .corner_radius(egui::CornerRadius::same(BADGE_CORNER_RADIUS))
        .inner_margin(egui::Margin::symmetric(BADGE_PAD_X, BADGE_PAD_Y))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = BADGE_GAP;
                ui.label(egui::RichText::new(label).color(fg).monospace());
                if ui
                    .small_button("×")
                    .on_hover_text(i18n::t("ui.tag_remove_hover"))
                    .clicked()
                {
                    remove = true;
                }
            });
        });
    remove
}

/// Row of active tag-filter badges. `on_remove` is called with the badge index.
pub fn tag_filter_row(
    ui: &mut Ui,
    tags: &[crate::app::LogcatTagFilter],
    on_remove: &mut Option<usize>,
) {
    if tags.is_empty() {
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(theme::ITEM_SPACING_Y, theme::ITEM_SPACING_Y);
        for (index, filter) in tags.iter().enumerate() {
            let (bg, fg) =
                colors::TAG_HIGHLIGHTS[filter.color_index % colors::TAG_HIGHLIGHTS.len()];
            let label = format!("tag:{}", filter.tag);
            if tag_filter_badge(ui, &label, bg, fg) {
                *on_remove = Some(index);
            }
        }
    });
    section_gap(ui);
}

pub fn panel_loading(ui: &mut Ui) {
    ui.label(i18n::t("common.loading"));
}

/// One chrome for every panel: uniform padding on all four sides, title, then body.
pub fn panel<R>(
    ui: &mut Ui,
    icon: Option<egui::ImageSource<'static>>,
    title: impl Into<egui::RichText>,
    add_body: impl FnOnce(&mut Ui) -> R,
) -> R {
    panel_with_header_actions(ui, icon, title, |_| {}, add_body)
}

/// Like [`panel`], with extra widgets on the header row.
pub fn panel_with_header_actions<R>(
    ui: &mut Ui,
    icon: Option<egui::ImageSource<'static>>,
    title: impl Into<egui::RichText>,
    add_header_actions: impl FnOnce(&mut Ui),
    add_body: impl FnOnce(&mut Ui) -> R,
) -> R {
    panel_frame(ui)
        .show(ui, |ui| {
            ui.set_min_size(ui.max_rect().size());
            panel_header(ui, icon, title, add_header_actions);
            panel_separator(ui);
            add_body(ui)
        })
        .inner
}

/// Downward shift of header icons relative to the title.
const HEADER_ICON_OFFSET_Y: f32 = 2.0;
const HEADER_ICON_WIDTH_RATIO: f32 = 1.5;

/// Draws the panel title row: optional leading icon, heading, then extra header widgets.
fn panel_header(
    ui: &mut Ui,
    icon: Option<egui::ImageSource<'static>>,
    title: impl Into<egui::RichText>,
    add_header_actions: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::ITEM_SPACING_Y;
        if let Some(icon) = icon {
            let height = TextStyle::Heading.resolve(ui.style()).size;
            let image = egui::Image::new(icon)
                .fit_to_exact_size(egui::vec2(height * HEADER_ICON_WIDTH_RATIO, height))
                .max_height(height)
                .show_loading_spinner(false)
                .tint(colors::HEADER_ICON);
            let size = image.calc_size(
                ui.available_size(),
                image
                    .load_for_size(ui.ctx(), ui.available_size())
                    .ok()
                    .and_then(|texture| texture.size()),
            );
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            image.paint_at(ui, rect.translate(egui::vec2(0.0, HEADER_ICON_OFFSET_Y)));
        }
        ui.heading(title.into().color(colors::HEADER_ICON));
        add_header_actions(ui);
    });
}

/// Panel card with a footer pinned to the bottom of the tile.
pub fn panel_with_custom_footer<R>(
    ui: &mut Ui,
    icon: Option<egui::ImageSource<'static>>,
    title: impl Into<egui::RichText>,
    add_header_actions: impl FnOnce(&mut Ui),
    add_body: impl FnOnce(&mut Ui) -> R,
    add_footer: impl FnOnce(&mut Ui),
) -> R {
    // Same structure as `Frame::begin`/`end`, but always paint/allocate the tile-sized
    // content rect. Overflowing body content must not push the bottom stroke outside the
    // pane clip (that drops the bottom border).
    let frame = panel_frame(ui);
    let where_to_put_background = ui.painter().add(egui::Shape::Noop);
    let outer_rect_bounds = ui.available_rect_before_wrap();
    let mut content_rect = outer_rect_bounds - frame.total_margin();
    content_rect.max.x = content_rect.max.x.max(content_rect.min.x);
    content_rect.max.y = content_rect.max.y.max(content_rect.min.y);

    let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
    content_ui.set_clip_rect(content_ui.clip_rect().intersect(content_rect));
    content_ui.set_min_size(content_rect.size());

    panel_header(&mut content_ui, icon, title, add_header_actions);
    panel_separator(&mut content_ui);

    let footer_height = panel_footer_height(&content_ui);
    let max_rect = content_ui.max_rect();
    let footer_top = (max_rect.bottom() - footer_height).max(max_rect.top());
    let footer_rect = egui::Rect::from_min_max(
        egui::pos2(max_rect.left(), footer_top),
        max_rect.right_bottom(),
    );
    let body_rect = egui::Rect::from_min_max(
        egui::pos2(max_rect.left(), content_ui.next_widget_position().y),
        egui::pos2(max_rect.right(), footer_rect.top()),
    );

    let body = content_ui
        .allocate_new_ui(egui::UiBuilder::new().max_rect(body_rect), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(body_rect));
            add_body(ui)
        })
        .inner;

    content_ui.allocate_new_ui(egui::UiBuilder::new().max_rect(footer_rect), add_footer);

    let widget_rect = frame.widget_rect(content_rect);
    if ui.is_rect_visible(widget_rect) {
        ui.painter()
            .set(where_to_put_background, frame.paint(content_rect));
    }
    ui.allocate_rect(frame.outer_rect(content_rect), egui::Sense::hover());
    body
}

/// Like [`panel_with_header_actions`], with a bottom footer inside the card.
///
/// `add_contents` receives the current auto-scroll flag and returns the number of lines
/// in the body text area. The footer is the only control that toggles auto-scroll.
/// `show_timestamps` adds a Timestamp on/off control to the left of Stream when `Some`.
pub fn panel_with_footer(
    ui: &mut Ui,
    icon: Option<egui::ImageSource<'static>>,
    title: impl Into<egui::RichText>,
    add_header_actions: impl FnOnce(&mut Ui),
    add_contents: impl FnOnce(&mut Ui, bool) -> usize,
    auto_scroll: &mut bool,
    show_timestamps: Option<&mut bool>,
) {
    let line_count = std::cell::Cell::new(0);
    let streaming = *auto_scroll;
    panel_with_custom_footer(
        ui,
        icon,
        title,
        add_header_actions,
        |ui| {
            line_count.set(add_contents(ui, streaming));
        },
        |ui| {
            panel_footer(ui, auto_scroll, show_timestamps, line_count.get());
        },
    );
}

/// Vertical padding above and below the footer status text.
const FOOTER_PAD_Y: f32 = 8.0;

/// Vertical space reserved for [`panel_footer`].
fn panel_footer_height(ui: &Ui) -> f32 {
    let text = ui.text_style_height(&TextStyle::Small);
    text + 2.0 * FOOTER_PAD_Y
}

/// Extra inset from the footer’s left and right edges for status text.
const FOOTER_LABEL_INSET_X: f32 = 8.0;

/// Downward shift of footer labels inside the bar.
const FOOTER_LABEL_OFFSET_Y: f32 = 3.0;

/// Draws the panel footer: left-aligned line count, right-aligned Stream and optional Timestamp.
///
/// Omits Lines first, then Timestamp, when they do not fit beside Stream.
pub fn panel_footer(
    ui: &mut Ui,
    auto_scroll: &mut bool,
    show_timestamps: Option<&mut bool>,
    line_count: usize,
) {
    footer_bar(ui, |ui| {
        let stream_label = if *auto_scroll {
            i18n::t("ui.footer.stream_active")
        } else {
            i18n::t("ui.footer.stream_paused")
        };
        let timestamp_label = show_timestamps.as_ref().map(|show| {
            if **show {
                i18n::t("ui.footer.timestamp_on")
            } else {
                i18n::t("ui.footer.timestamp_off")
            }
        });
        let count_str = line_count.to_string();
        let lines_label = i18n::tf("ui.footer.lines", &[("count", &count_str)]);

        let gap = ui.spacing().item_spacing.x;
        let timestamp_w = timestamp_label.map(|label| footer_label_width(ui, label));
        let lines_w = footer_label_width(ui, &lines_label);

        let mut remaining = ui.max_rect().width() - footer_label_width(ui, stream_label);
        let show_timestamp = timestamp_w.is_some_and(|w| remaining >= gap + w);
        if show_timestamp {
            if let Some(w) = timestamp_w {
                remaining -= gap + w;
            }
        }
        let show_lines = remaining >= gap + lines_w;

        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            if show_lines {
                ui.label(
                    egui::RichText::new(lines_label)
                        .small()
                        .color(colors::FOOTER_TEXT),
                );
            }
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), ui.available_height()),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    footer_toggle(
                        ui,
                        stream_label,
                        i18n::t("ui.footer.stream_hover"),
                        auto_scroll,
                    );
                    if show_timestamp {
                        if let (Some(label), Some(show_timestamps)) =
                            (timestamp_label, show_timestamps)
                        {
                            footer_toggle(
                                ui,
                                label,
                                i18n::t("ui.footer.timestamp_hover"),
                                show_timestamps,
                            );
                        }
                    }
                },
            );
        });
    });
}

/// Footer with last-refresh status and a right-aligned Refresh control.
/// Returns `true` when Refresh is clicked.
pub fn devices_footer(
    ui: &mut Ui,
    refreshed_at: Option<Instant>,
    list_job_in_flight: bool,
) -> bool {
    let mut refresh = false;
    footer_bar(ui, |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            if list_job_in_flight {
                ui.label(
                    egui::RichText::new(i18n::t("ui.devices.refreshing"))
                        .small()
                        .color(colors::FOOTER_TEXT),
                );
            } else if let Some(at) = refreshed_at {
                ui.label(
                    egui::RichText::new(refresh_age_label(at))
                        .small()
                        .color(colors::FOOTER_TEXT),
                );
            }
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), ui.available_height()),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    refresh = footer_action(
                        ui,
                        i18n::t("ui.devices.check_adb"),
                        i18n::t("ui.devices.check_adb_hover"),
                    );
                },
            );
        });
    });
    refresh
}

fn refresh_age_label(at: Instant) -> String {
    let secs = at.elapsed().as_secs();
    match secs {
        0 => i18n::t("ui.devices.updated_now").to_string(),
        1 => i18n::t("ui.devices.updated_1s").to_string(),
        s if s < 60 => {
            let n = s.to_string();
            i18n::tf("ui.devices.updated_secs", &[("n", &n)])
        }
        s => {
            let n = (s / 60).to_string();
            i18n::tf("ui.devices.updated_mins", &[("n", &n)])
        }
    }
}

/// Separator and inset content area shared by panel footers.
fn footer_bar(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    let rect = ui.max_rect();
    ui.allocate_rect(rect, egui::Sense::hover());

    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, colors::PANEL_SEPARATOR),
    );

    let content_rect = egui::Rect::from_min_max(
        egui::pos2(
            rect.left() + FOOTER_LABEL_INSET_X,
            rect.top() + FOOTER_LABEL_OFFSET_Y,
        ),
        egui::pos2(
            rect.right() - FOOTER_LABEL_INSET_X,
            rect.bottom() + FOOTER_LABEL_OFFSET_Y,
        ),
    );

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), add_contents);
}

/// Unwrapped width of footer status text in the Small style.
fn footer_label_width(ui: &Ui, text: &str) -> f32 {
    egui::WidgetText::from(egui::RichText::new(text).small().color(colors::FOOTER_TEXT))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            TextStyle::Small,
        )
        .size()
        .x
}

/// Small text action for a panel header row. Returns `true` when clicked.
pub fn header_action(ui: &mut Ui, label: &str, hover: &str) -> bool {
    ui.add(
        egui::Label::new(
            egui::RichText::new(label)
                .small()
                .color(colors::FOOTER_TEXT),
        )
        .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .on_hover_text(hover)
    .clicked()
}

fn footer_action(ui: &mut Ui, label: &str, hover: &str) -> bool {
    ui.add(
        egui::Label::new(
            egui::RichText::new(label)
                .small()
                .color(colors::FOOTER_TEXT),
        )
        .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .on_hover_text(hover)
    .clicked()
}

fn footer_toggle(ui: &mut Ui, label: &str, hover: &str, flag: &mut bool) {
    if footer_action(ui, label, hover) {
        *flag = !*flag;
    }
}

/// Elevated dark-blue card container for tool windows.
pub fn tool_card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::default()
        .fill(colors::SURFACE_CARD)
        .stroke(egui::Stroke::new(1.0, colors::BORDER_SUBTLE))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, add)
        .inner
}

/// Darker inset card container for drop zones, summaries, and nested blocks.
pub fn tool_subcard<R>(ui: &mut Ui, border: Option<egui::Color32>, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::default()
        .fill(colors::SURFACE_SUBCARD)
        .stroke(egui::Stroke::new(1.0, border.unwrap_or(colors::BORDER_SUBTLE)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(9))
        .show(ui, add)
        .inner
}

/// Smooth sine-wave breathing factor in `0.0..=1.0`, capped at ~30 FPS so pulsing
/// never pegs the CPU. Call only when a pulsing widget is actively drawn.
pub fn pulse_factor(ctx: &Context, period_secs: f64) -> f32 {
    let t = ctx.input(|i| i.time);
    let phase = 0.5 - 0.5 * (t * std::f64::consts::TAU / period_secs.max(0.2)).cos();
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
    (phase as f32).clamp(0.0, 1.0)
}

/// Linearly interpolates two colors (RGB and alpha) with `t` clamped to `0.0..=1.0`.
pub fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 { ((x as f32) + ((y as f32) - (x as f32)) * t).round() as u8 };
    egui::Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// Tinted pill badge for device status, ABI, queue items, or warnings.
pub fn status_pill(ui: &mut Ui, text: &str, color: egui::Color32) {
    pulsing_status_pill(ui, text, color, false);
}

/// Status pill with an optional gentle breathing highlight when active/live.
pub fn pulsing_status_pill(ui: &mut Ui, text: &str, color: egui::Color32, pulse: bool) {
    let p = if pulse {
        pulse_factor(ui.ctx(), 1.8)
    } else {
        0.0
    };
    let fill_alpha = if pulse { (28.0 + 36.0 * p) as u8 } else { 36 };
    let stroke_alpha = if pulse { (90.0 + 130.0 * p) as u8 } else { 120 };
    let fill = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), fill_alpha);
    let stroke = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), stroke_alpha);
    egui::Frame::default()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(color).strong());
        });
}

/// Section title + muted subtitle inside tool windows.
pub fn section_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(egui::RichText::new(title).color(colors::OFF_WHITE).strong());
    if !subtitle.is_empty() {
        ui.label(egui::RichText::new(subtitle).small().color(colors::TEXT_MUTED));
    }
}

/// Primary semantic CTA button (blue, green, amber, or red).
pub fn accent_button(ui: &mut Ui, label: &str, fill: egui::Color32) -> egui::Response {
    let stroke_color = lerp_color(fill, egui::Color32::WHITE, 0.22);
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE).strong())
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke_color))
            .corner_radius(egui::CornerRadius::same(7)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Semantic CTA button that gently pulses with a breathing glow halo when `pulse`
/// is true and the widget is enabled, guiding the user to the next step.
pub fn pulsing_cta_button(
    ui: &mut Ui,
    label: &str,
    fill: egui::Color32,
    pulse: bool,
) -> egui::Response {
    if !pulse || !ui.is_enabled() {
        return accent_button(ui, label, fill);
    }

    let p = pulse_factor(ui.ctx(), 1.45);
    let active_fill = lerp_color(fill, egui::Color32::WHITE, 0.08 + 0.16 * p);
    let border_color = lerp_color(fill, egui::Color32::WHITE, 0.35 + 0.45 * p);
    let response = ui
        .add(
            egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE).strong())
                .fill(active_fill)
                .stroke(egui::Stroke::new(1.5 + 0.8 * p, border_color))
                .corner_radius(egui::CornerRadius::same(7)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(response.rect) {
        let halo_alpha = (45.0 + 115.0 * p) as u8;
        let halo_color =
            egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), halo_alpha);
        ui.painter().rect_stroke(
            response.rect.expand(2.0 + 3.0 * p),
            egui::CornerRadius::same(10),
            egui::Stroke::new(2.0, halo_color),
            egui::StrokeKind::Outside,
        );
    }

    response
}

/// Full-width live progress banner with spinner, breathing border, and a sleek
/// 3px sliding highlight bar across the bottom edge while an operation is running.
pub fn busy_banner(ui: &mut Ui, label: &str) {
    let p = pulse_factor(ui.ctx(), 1.45);
    let border = lerp_color(colors::ACCENT_BLUE, colors::ACCENT_CYAN, p);
    let fill = egui::Color32::from_rgba_unmultiplied(
        colors::ACCENT_BLUE.r(),
        colors::ACCENT_BLUE.g(),
        colors::ACCENT_BLUE.b(),
        (22.0 + 18.0 * p) as u8,
    );
    let resp = egui::Frame::default()
        .fill(fill)
        .stroke(egui::Stroke::new(1.2, border))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new(label)
                        .color(colors::OFF_WHITE)
                        .strong(),
                );
            });
            ui.add_space(3.0);
        });

    let rect = resp.response.rect;
    if ui.is_rect_visible(rect) && rect.width() > 20.0 {
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 8.0, rect.bottom() - 4.0),
            egui::pos2(rect.right() - 8.0, rect.bottom() - 1.0),
        );
        ui.painter().rect_filled(
            track,
            egui::CornerRadius::same(2),
            egui::Color32::from_rgba_unmultiplied(colors::ACCENT_BLUE.r(), colors::ACCENT_BLUE.g(), colors::ACCENT_BLUE.b(), 40),
        );
        let seg_w = (track.width() * 0.28).max(32.0);
        let travel = (track.width() - seg_w).max(0.0);
        let x0 = track.left() + travel * p;
        let active = egui::Rect::from_min_max(
            egui::pos2(x0, track.top()),
            egui::pos2((x0 + seg_w).min(track.right()), track.bottom()),
        );
        ui.painter().rect_filled(
            active,
            egui::CornerRadius::same(2),
            colors::ACCENT_CYAN,
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    Active,
    Next,
    Pending,
}

pub struct StepItem<'a> {
    pub number: u8,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub state: StepState,
}

/// Interactive horizontal workflow stepper with connected step cards.
/// Highlights completed (`✓`), current, and pulsing next steps. Returns the
/// clicked step index when any step is clicked.
pub fn workflow_stepper(ui: &mut Ui, steps: &[StepItem<'_>]) -> Option<usize> {
    if steps.is_empty() {
        return None;
    }
    let mut clicked = None;
    let has_next = steps.iter().any(|s| s.state == StepState::Next);
    let p = if has_next {
        pulse_factor(ui.ctx(), 1.45)
    } else {
        0.0
    };

    let avail = ui.available_width();
    let count = steps.len() as f32;
    let connector_w = 14.0;
    let card_w = ((avail - connector_w * (count - 1.0)) / count).max(120.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut card_centers: Vec<(egui::Rect, StepState)> = Vec::with_capacity(steps.len());
        for (idx, step) in steps.iter().enumerate() {
            if idx > 0 {
                let (conn_rect, _) = ui.allocate_exact_size(
                    egui::vec2(connector_w, 54.0),
                    egui::Sense::hover(),
                );
                let line_color = if steps[idx - 1].state == StepState::Done {
                    colors::ACCENT_GREEN
                } else {
                    colors::BORDER_SUBTLE
                };
                ui.painter().hline(
                    (conn_rect.left() + 2.0)..=(conn_rect.right() - 2.0),
                    conn_rect.center().y,
                    egui::Stroke::new(2.0, line_color),
                );
            }

            let (fill, border, badge_fill, badge_txt) = match step.state {
                StepState::Done => (
                    egui::Color32::from_rgba_unmultiplied(
                        colors::ACCENT_GREEN.r(),
                        colors::ACCENT_GREEN.g(),
                        colors::ACCENT_GREEN.b(),
                        24,
                    ),
                    colors::ACCENT_GREEN,
                    colors::ACCENT_GREEN,
                    "✓".to_string(),
                ),
                StepState::Active => (
                    egui::Color32::from_rgba_unmultiplied(
                        colors::ACCENT_BLUE.r(),
                        colors::ACCENT_BLUE.g(),
                        colors::ACCENT_BLUE.b(),
                        44,
                    ),
                    colors::ACCENT_CYAN,
                    colors::ACCENT_BLUE,
                    step.number.to_string(),
                ),
                StepState::Next => (
                    egui::Color32::from_rgba_unmultiplied(
                        colors::ACCENT_CYAN.r(),
                        colors::ACCENT_CYAN.g(),
                        colors::ACCENT_CYAN.b(),
                        (18.0 + 30.0 * p) as u8,
                    ),
                    lerp_color(colors::ACCENT_CYAN, colors::ACCENT_GREEN, p),
                    colors::ACCENT_CYAN,
                    "→".to_string(),
                ),
                StepState::Pending => (
                    colors::SURFACE_SUBCARD,
                    colors::BORDER_SUBTLE,
                    colors::BG_WIDGET,
                    step.number.to_string(),
                ),
            };

            let (card_rect, resp) = ui.allocate_exact_size(
                egui::vec2(card_w, 54.0),
                egui::Sense::click(),
            );
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let hovered = resp.hovered();
            if resp.clicked() {
                clicked = Some(idx);
            }

            let bg = if hovered && step.state != StepState::Active {
                lerp_color(fill, colors::BG_HOVER, 0.45)
            } else {
                fill
            };
            let stroke_w = match step.state {
                StepState::Active => 1.6,
                StepState::Next => 1.3 + 0.7 * p,
                _ => 1.0,
            };
            ui.painter()
                .rect_filled(card_rect, egui::CornerRadius::same(8), bg);
            ui.painter().rect_stroke(
                card_rect,
                egui::CornerRadius::same(8),
                egui::Stroke::new(stroke_w, border),
                egui::StrokeKind::Inside,
            );

            let badge_center = egui::pos2(card_rect.left() + 18.0, card_rect.center().y);
            ui.painter().circle_filled(badge_center, 11.0, badge_fill);
            let badge_fg = if step.state == StepState::Pending {
                colors::TEXT_MUTED
            } else {
                egui::Color32::WHITE
            };
            ui.painter().text(
                badge_center,
                egui::Align2::CENTER_CENTER,
                &badge_txt,
                FontId::new(11.0, FontFamily::Proportional),
                badge_fg,
            );

            let text_left = card_rect.left() + 36.0;
            let title_color = match step.state {
                StepState::Active | StepState::Next => colors::OFF_WHITE,
                StepState::Done => lerp_color(colors::OFF_WHITE, colors::ACCENT_GREEN, 0.25),
                StepState::Pending => colors::HEADER_ICON,
            };
            ui.painter().text(
                egui::pos2(text_left, card_rect.top() + 17.0),
                egui::Align2::LEFT_CENTER,
                step.title,
                FontId::new(theme::FONT_BODY, FontFamily::Proportional),
                title_color,
            );
            ui.painter().text(
                egui::pos2(text_left, card_rect.top() + 37.0),
                egui::Align2::LEFT_CENTER,
                step.subtitle,
                FontId::new(theme::FONT_SMALL, FontFamily::Proportional),
                colors::TEXT_MUTED,
            );

            card_centers.push((card_rect, step.state));
        }
        let _ = card_centers;
    });
    clicked
}

pub struct NavTabItem<'a> {
    pub label: &'a str,
    pub icon: &'a str,
    pub badge: Option<String>,
    pub busy: bool,
}

/// Modern horizontal pill navigation bar with active bottom highlight, optional
/// count badges, and a pulsing dot for tabs running background tasks.
pub fn main_nav_bar<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    items: &[(T, NavTabItem<'_>)],
) {
    let any_busy = items.iter().any(|(_, it)| it.busy);
    let p = if any_busy {
        pulse_factor(ui.ctx(), 1.3)
    } else {
        0.0
    };

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (idx, (val, item)) in items.iter().enumerate() {
            let selected = *current == *val;
            let text = if let Some(b) = &item.badge {
                format!("{} {} ({b})", item.icon, item.label)
            } else {
                format!("{} {}", item.icon, item.label)
            };
            let fg = if selected {
                colors::OFF_WHITE
            } else {
                colors::TEXT_MUTED
            };
            let bg = if selected {
                colors::BG_WIDGET
            } else {
                egui::Color32::TRANSPARENT
            };
            let border = if selected {
                egui::Stroke::new(1.0, colors::ACCENT_BLUE)
            } else {
                egui::Stroke::new(1.0, egui::Color32::TRANSPARENT)
            };

            let mut rich = egui::RichText::new(text).color(fg);
            if selected {
                rich = rich.strong();
            }

            let resp = ui
                .push_id(idx, |ui| {
                    ui.add(
                        egui::Button::new(rich)
                            .fill(bg)
                            .stroke(border)
                            .corner_radius(egui::CornerRadius::same(7)),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                })
                .inner;

            if resp.clicked() {
                *current = *val;
            }
            if selected {
                ui.painter().hline(
                    (resp.rect.left() + 8.0)..=(resp.rect.right() - 8.0),
                    resp.rect.bottom() - 1.5,
                    egui::Stroke::new(2.5, colors::ACCENT_CYAN),
                );
            }
            if item.busy {
                let dot_alpha = (110.0 + 145.0 * p) as u8;
                let dot_color = egui::Color32::from_rgba_unmultiplied(
                    colors::ACCENT_CYAN.r(),
                    colors::ACCENT_CYAN.g(),
                    colors::ACCENT_CYAN.b(),
                    dot_alpha,
                );
                ui.painter().circle_filled(
                    egui::pos2(resp.rect.right() - 6.0, resp.rect.top() + 6.0),
                    3.5,
                    dot_color,
                );
            }
        }
    });
}

/// Horizontal pill selector for secondary mode tabs inside a tool panel.
pub fn sub_tab_pills<T: Copy + PartialEq>(ui: &mut Ui, current: &mut T, items: &[(T, &str)]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        for (val, label) in items {
            let selected = *current == *val;
            let fill = if selected {
                egui::Color32::from_rgba_unmultiplied(
                    colors::ACCENT_BLUE.r(),
                    colors::ACCENT_BLUE.g(),
                    colors::ACCENT_BLUE.b(),
                    52,
                )
            } else {
                colors::SURFACE_SUBCARD
            };
            let stroke = if selected {
                colors::ACCENT_BLUE
            } else {
                colors::BORDER_SUBTLE
            };
            let fg = if selected {
                colors::OFF_WHITE
            } else {
                colors::TEXT_MUTED
            };
            let mut rich = egui::RichText::new(*label).color(fg);
            if selected {
                rich = rich.strong();
            }
            if ui
                .add(
                    egui::Button::new(rich)
                        .fill(fill)
                        .stroke(egui::Stroke::new(1.0, stroke))
                        .corner_radius(egui::CornerRadius::same(14)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                *current = *val;
            }
        }
    });
}

/// Opens a folder in the OS file explorer (creates it first if missing).
pub fn open_folder_in_os(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
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

