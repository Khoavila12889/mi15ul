//! Các tab chính của cửa sổ duy nhất.
//!
//! Mọi tính năng sống trong đúng một tab; nội dung tab render trọn trong
//! khung nội dung của tab đó thay vì mở cửa sổ nổi riêng.

use crate::i18n;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MainTab {
    Overview,
    Logs,
    Apps,
    HyperOs,
    Flash,
    Tools,
}

impl MainTab {
    pub const ALL: [Self; 6] = [
        Self::Overview,
        Self::Logs,
        Self::Apps,
        Self::HyperOs,
        Self::Flash,
        Self::Tools,
    ];

    /// Tên định danh ổn định dùng khi lưu cấu hình.
    pub fn ident(self) -> &'static str {
        match self {
            MainTab::Overview => "Overview",
            MainTab::Logs => "Logs",
            MainTab::Apps => "Apps",
            MainTab::HyperOs => "HyperOs",
            MainTab::Flash => "Flash",
            MainTab::Tools => "Tools",
        }
    }

    pub fn from_ident(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|tab| tab.ident() == value)
    }

    /// Nhãn tab đã Việt hóa qua i18n tập trung.
    pub fn title(self) -> &'static str {
        match self {
            MainTab::Overview => i18n::t("tab.overview"),
            MainTab::Logs => i18n::t("tab.logs"),
            MainTab::Apps => i18n::t("tab.apps"),
            MainTab::HyperOs => i18n::t("tab.hyperos"),
            MainTab::Flash => i18n::t("tab.flash"),
            MainTab::Tools => i18n::t("tab.tools"),
        }
    }
}
