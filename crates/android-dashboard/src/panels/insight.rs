use std::collections::VecDeque;

use eframe::egui;

use crate::app::{InsightController, InsightStatus};
use crate::i18n;
use crate::insight::InsightReply;
use crate::theme;
use crate::ui_elements;

const REPLY_GAP: f32 = 8.0;
const REPLY_SEPARATOR_GAP: f32 = 4.0;

/// Draws the insight body and returns the number of text lines in the reply area.
pub fn insight_panel(ui: &mut egui::Ui, insight: &InsightController, auto_scroll: bool) -> usize {
    ui_elements::panel_body(ui, theme::colors::INSIGHT_BODY, |ui| {
        if let Some(path) = ai_insight::history_path() {
            ui.horizontal_wrapped(|ui| {
                ui.small(i18n::t("insight.history_label"))
                    .on_hover_text(path.display().to_string());
                if ui.small_button(i18n::t("insight.copy_log_path")).clicked() {
                    ui.ctx().copy_text(path.display().to_string());
                }
            });
        }
        if insight.state.replies.is_empty() {
            match insight.state.status {
                InsightStatus::RequestFailed => {
                    ui.label(
                        insight
                            .state
                            .last_failure
                            .as_deref()
                            .unwrap_or(i18n::t("insight.default_error")),
                    );
                }
                InsightStatus::Idle | InsightStatus::RequestSent => {
                    if insight.config().is_configured() {
                        let engine = i18n::insight_engine_label(insight.config().engine);
                        ui.label(i18n::tf("insight.waiting", &[("engine", engine)]));
                    } else {
                        ui.label(i18n::t("insight.not_configured"));
                    }
                }
            }
            return 0;
        }

        egui::ScrollArea::vertical()
            .id_salt(egui::Id::new("insight_scroll"))
            .stick_to_bottom(auto_scroll)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for (index, reply) in insight.state.replies.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(REPLY_GAP);
                        ui.separator();
                        ui.add_space(REPLY_SEPARATOR_GAP);
                    }
                    ui.label(i18n::tf("insight.response_at", &[("time", &reply.received_at)]));
                    if !reply.body.is_empty() {
                        ui.label(reply.body.as_str());
                    }
                }
            });
        insight_reply_line_count(&insight.state.replies)
    })
}

/// Counts newline-separated lines across insight replies, including the timestamp line.
fn insight_reply_line_count(replies: &VecDeque<InsightReply>) -> usize {
    replies
        .iter()
        .map(|reply| {
            let body_lines = if reply.body.is_empty() {
                0
            } else {
                reply.body.lines().count().max(1)
            };
            1 + body_lines
        })
        .sum()
}
