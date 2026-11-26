//! HyperOS China ROM optimization, notification fix, MTZ packager, safe debloat,
//! and Bank/VNeID-ready security lock — all executed via PC `adb.exe` with zero
//! persistent APKs or background services on the device.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const EMBEDDED_METADATA_TEMPLATE: &str = include_str!("../../../themes/metadata_template.json");

const THEME_DATA_ROOT: &str = "/sdcard/Android/data/com.android.thememanager/files/MIUI/theme/.data";
const THEME_DATA_CONTENT_PREFIX: &str =
    "/system/../storage/emulated/0/Android/data/com.android.thememanager/files/MIUI/theme/.data/content/theme";

const MAX_MTZ_BYTES: u64 = 367_001_600;
const MAX_ENTRY_BYTES: u64 = 83_886_080;
const MAX_TOTAL_UNPACKED: u64 = 471_859_200;
const MAX_ZIP_ENTRIES: usize = 1200;

// ---------------------------------------------------------------------------
// Catalogs & Presets (ported 1:1 from hyperos-pc-tool/presets.py)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ThemePreset {
    pub filename: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

pub const THEME_CATALOG: &[ThemePreset] = &[
    ThemePreset {
        filename: "MTZOS4.mtz",
        title: "MTZVN (Chuẩn - Máy thường)",
        description: "Việt hóa đầy đủ giao diện hệ thống, kể cả ngày giờ màn hình khóa.",
    },
    ThemePreset {
        filename: "MTZOS4_kinhlong.mtz",
        title: "MTZVN Kính Lỏng",
        description: "Giữ hiệu ứng kính lỏng; thanh trạng thái và màn hình khóa giữ chữ gốc (không Việt hóa SystemUI).",
    },
    ThemePreset {
        filename: "MTZOS4_leica_vibrant.mtz",
        title: "Xiaomi 17 Ultra Leica Vibrant VH",
        description: "Việt hóa đầy đủ kèm bộ icon và hình nền phong cách Xiaomi Leica Vibrant.",
    },
    ThemePreset {
        filename: "MTZOS4_fold.mtz",
        title: "MTZVN Máy Gập (Fold)",
        description: "Việt hóa đầy đủ như MTZVN, tối ưu riêng cho dòng máy gập Xiaomi Fold.",
    },
    ThemePreset {
        filename: "MTZOS4_fold_kinhlong.mtz",
        title: "MTZVN Máy Gập Kính Lỏng",
        description: "Giữ hiệu ứng kính lỏng cho máy gập; thanh trạng thái và màn hình khóa giữ chữ gốc.",
    },
];

#[derive(Clone, Copy, Debug)]
pub struct NotificationAppPreset {
    pub package: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub default_selected: bool,
}

pub const NOTIFICATION_APPS: &[NotificationAppPreset] = &[
    NotificationAppPreset {
        package: "com.google.android.gms",
        name: "Google Play Services (FCM)",
        category: "Dịch vụ Google / FCM",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.google.android.gsf",
        name: "Google Services Framework",
        category: "Dịch vụ Google / FCM",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.google.android.gm",
        name: "Gmail",
        category: "Dịch vụ Google / FCM",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.google.android.apps.messaging",
        name: "Google Messages",
        category: "Dịch vụ Google / FCM",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "vn.vneid",
        name: "VNeID (Định danh điện tử)",
        category: "Định danh & Công",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vss.vssid",
        name: "VssID (Bảo hiểm xã hội)",
        category: "Định danh & Công",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "vn.gov.etax",
        name: "eTax Mobile",
        category: "Định danh & Công",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.zing.zalo",
        name: "Zalo",
        category: "Nhắn tin & MXH",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.facebook.orca",
        name: "Messenger",
        category: "Nhắn tin & MXH",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.facebook.katana",
        name: "Facebook",
        category: "Nhắn tin & MXH",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "org.telegram.messenger",
        name: "Telegram",
        category: "Nhắn tin & MXH",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "org.thunderdog.challegram",
        name: "Telegram X",
        category: "Nhắn tin & MXH",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.whatsapp",
        name: "WhatsApp",
        category: "Nhắn tin & MXH",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.viber.voip",
        name: "Viber",
        category: "Nhắn tin & MXH",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.ss.android.ugc.trill",
        name: "TikTok (VN/Global)",
        category: "Nhắn tin & MXH",
        default_selected: false,
    },
    NotificationAppPreset {
        package: "com.zhiliaoapp.musically",
        name: "TikTok (Musically)",
        category: "Nhắn tin & MXH",
        default_selected: false,
    },
    NotificationAppPreset {
        package: "com.VCB",
        name: "Vietcombank (VCB Digibank)",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.mbmobile",
        name: "MB Bank",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "vn.com.techcombank.bb.app",
        name: "Techcombank Mobile",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vnpay.bidv",
        name: "BIDV SmartBanking",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vietinbank.ipay",
        name: "VietinBank iPay",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vnpay.Agribank3g",
        name: "Agribank Plus",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.tpb.mb.gprsandroid",
        name: "TPBank Mobile",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vnpay.vpbankonline",
        name: "VPBank NEO",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "mobile.acb.com.vn",
        name: "ACB ONE",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "src.com.sacombank",
        name: "Sacombank Pay / mBanking",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.sacombank.ewallet",
        name: "Sacombank Pay (eWallet)",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vib.myvib2",
        name: "MyVIB 2.0",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vnpay.hdbank",
        name: "HDBank",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.vnpay.ocb",
        name: "OCB OMNI",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.shb.mbanking",
        name: "SHB Mobile",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.mservice.momotransfer",
        name: "MoMo",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "vn.com.vng.zalopay",
        name: "ZaloPay",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.bplus.vtpay",
        name: "Viettel Money",
        category: "Ngân hàng & Ví",
        default_selected: true,
    },
    NotificationAppPreset {
        package: "com.shopee.vn",
        name: "Shopee VN",
        category: "Tiện ích",
        default_selected: false,
    },
    NotificationAppPreset {
        package: "com.grabtaxi.passenger",
        name: "Grab",
        category: "Tiện ích",
        default_selected: false,
    },
];

#[derive(Clone, Copy, Debug)]
pub struct DebloatPreset {
    pub package: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub risk: &'static str,
    pub description: &'static str,
    pub default_selected: bool,
}

pub const DEBLOAT_PACKAGES: &[DebloatPreset] = &[
    DebloatPreset {
        package: "com.miui.analytics",
        name: "MIUI Analytics",
        category: "Quảng cáo & Theo dõi",
        risk: "An toàn",
        description: "Dịch vụ thu thập dữ liệu phân tích và nhật ký hành vi người dùng của Xiaomi.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.msa.global",
        name: "MSA (MIUI System Ads)",
        category: "Quảng cáo & Theo dõi",
        risk: "An toàn",
        description: "Dịch vụ hiển thị quảng cáo hệ thống trong các ứng dụng mặc định MIUI/HyperOS.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.systemAdSolution",
        name: "System Ad Solution (China)",
        category: "Quảng cáo & Theo dõi",
        risk: "An toàn",
        description: "Nền tảng phân phối quảng cáo hệ thống trên ROM nội địa Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.daemon",
        name: "MIUI Daemon",
        category: "Quảng cáo & Theo dõi",
        risk: "An toàn",
        description: "Tiến trình chạy nền thu thập thống kê chẩn đoán và gửi telemetry cho Xiaomi.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.joyose",
        name: "Xiaomi Joyose",
        category: "Quảng cáo & Theo dõi",
        risk: "Cân nhắc",
        description: "Quản lý cấu hình game/đám mây và quảng cáo; gỡ giúp giảm chạy ngầm nhưng vài tính năng game boost sẽ tắt.",
        default_selected: false,
    },
    DebloatPreset {
        package: "com.xiaomi.ab",
        name: "Xiaomi A/B Testing",
        category: "Quảng cáo & Theo dõi",
        risk: "An toàn",
        description: "Dịch vụ thử nghiệm A/B cấu hình tính năng từ xa của Xiaomi.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.hybrid",
        name: "Quick Apps (Ứng dụng nhanh)",
        category: "App rác Trung Quốc",
        risk: "An toàn",
        description: "Nền tảng chạy mini-app không cần cài đặt, hay tự bật quảng cáo và thông báo rác.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.hybrid.accessory",
        name: "Quick Apps Accessory",
        category: "App rác Trung Quốc",
        risk: "An toàn",
        description: "Thành phần phụ trợ của nền tảng Quick Apps (ứng dụng nhanh).",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.mipicks",
        name: "GetApps (Global)",
        category: "App rác Trung Quốc",
        risk: "Cân nhắc",
        description: "Kho ứng dụng Xiaomi bản Global; giữ nếu dùng cập nhật app hệ thống.",
        default_selected: false,
    },
    DebloatPreset {
        package: "com.xiaomi.market",
        name: "GetApps (Mi Market CN)",
        category: "App rác Trung Quốc",
        risk: "Cân nhắc",
        description: "Kho ứng dụng nội địa Trung Quốc (Mi Market); có thể gỡ hoặc tắt nếu chỉ dùng CH Play.",
        default_selected: false,
    },
    DebloatPreset {
        package: "com.miui.newhome",
        name: "Content Center (Bảng tin vuốt lên)",
        category: "Trợ lý & AI Trung Quốc",
        risk: "An toàn",
        description: "Trình duyệt tin tức / nội dung khi vuốt từ dưới màn hình chính trên ROM Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.yellowpage",
        name: "Trang Vàng (Yellow Pages)",
        category: "App rác Trung Quốc",
        risk: "An toàn",
        description: "Danh bạ số tổng đài và dịch vụ nội địa Trung Quốc tích hợp trong Điện thoại.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.payment",
        name: "Mi Coin / Thanh toán Xiaomi",
        category: "Ví & Mua sắm TQ",
        risk: "An toàn",
        description: "Cổng thanh toán Mi Coin nội địa Trung Quốc, không dùng tại Việt Nam.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.mipay.wallet",
        name: "Mi Pay / Xiaomi Wallet",
        category: "Ví & Mua sắm TQ",
        risk: "An toàn",
        description: "Ví điện tử và thẻ giao thông nội địa Trung Quốc (không dùng được ở VN).",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.youpin",
        name: "Xiaomi Youpin",
        category: "Ví & Mua sắm TQ",
        risk: "An toàn",
        description: "Ứng dụng mua sắm thương mại điện tử Xiaomi Youpin (tiếng Trung).",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.shop",
        name: "Mi Store (China)",
        category: "Ví & Mua sắm TQ",
        risk: "An toàn",
        description: "Ứng dụng cửa hàng Xiaomi nội địa Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.gamecenter",
        name: "Mi Game Center",
        category: "Giải trí & Tin tức TQ",
        risk: "An toàn",
        description: "Trung tâm tải game và thông báo khuyến mãi game nội địa Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.migameservice",
        name: "Mi Game Service",
        category: "Giải trí & Tin tức TQ",
        risk: "An toàn",
        description: "Dịch vụ tài khoản/SDK phụ trợ cho các game nội địa trên Mi Game Center.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.video",
        name: "Mi Video (China)",
        category: "Giải trí & Tin tức TQ",
        risk: "Cân nhắc",
        description: "Trình phát video mặc định kèm tab phim/quảng cáo Trung Quốc.",
        default_selected: false,
    },
    DebloatPreset {
        package: "com.miui.player",
        name: "Mi Music (China)",
        category: "Giải trí & Tin tức TQ",
        risk: "Cân nhắc",
        description: "Trình nghe nhạc mặc định kèm tab nhạc trực tuyến Trung Quốc.",
        default_selected: false,
    },
    DebloatPreset {
        package: "com.miui.newmidrive",
        name: "Xiaomi Cloud Drive (CN)",
        category: "Giải trí & Tin tức TQ",
        risk: "An toàn",
        description: "Ứng dụng lưu trữ đám mây nội địa Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.miservice",
        name: "Dịch vụ & Phản hồi (Mi Service)",
        category: "Hệ thống phụ trợ",
        risk: "An toàn",
        description: "Ứng dụng gửi phản hồi/báo lỗi và hỗ trợ bảo hành nội địa Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.bugreport",
        name: "Báo cáo lỗi MIUI (Bug Report)",
        category: "Hệ thống phụ trợ",
        risk: "An toàn",
        description: "Công cụ thu thập log và gửi báo cáo lỗi về máy chủ Xiaomi.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.xiaomi.vipaccount",
        name: "Xiaomi Community / VIP",
        category: "Hệ thống phụ trợ",
        risk: "An toàn",
        description: "Ứng dụng cộng đồng Xiaomi và tích điểm thành viên nội địa.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.voiceassist",
        name: "XiaoAI (Trợ lý ảo tiếng Trung)",
        category: "Trợ lý & AI Trung Quốc",
        risk: "An toàn",
        description: "Trợ lý giọng nói XiaoAI chỉ hỗ trợ tiếng Trung.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.personalassistant",
        name: "App Vault (Màn hình trừ 1 CN)",
        category: "Trợ lý & AI Trung Quốc",
        risk: "Cân nhắc",
        description: "Bảng tin nhanh bên trái màn hình chính (chứa thẻ dịch vụ và tin tiếng Trung).",
        default_selected: false,
    },
    DebloatPreset {
        package: "com.miui.contentextension",
        name: "Chạm để nhận diện (Taplus CN)",
        category: "Trợ lý & AI Trung Quốc",
        risk: "An toàn",
        description: "Tính năng nhấn giữ hai ngón tách chữ/ảnh phục vụ dịch vụ nội địa Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.translation.kingsoft",
        name: "Từ điển Kingsoft",
        category: "Trợ lý & AI Trung Quốc",
        risk: "An toàn",
        description: "Module dịch tích hợp của Kingsoft dành cho ROM Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.translation.youdao",
        name: "Từ điển Youdao",
        category: "Trợ lý & AI Trung Quốc",
        risk: "An toàn",
        description: "Module dịch tích hợp của Youdao dành cho ROM Trung Quốc.",
        default_selected: true,
    },
    DebloatPreset {
        package: "com.miui.carlink",
        name: "CarWith (Kết nối xe nội địa)",
        category: "Hệ thống phụ trợ",
        risk: "Cân nhắc",
        description: "Tính năng kết nối màn hình ô tô nội địa Trung Quốc (Baidu CarLife/ICCOA).",
        default_selected: false,
    },
    DebloatPreset {
        package: "com.xiaomi.aiasst.service",
        name: "AI Call Assistant (XiaoAI Call)",
        category: "Trợ lý & AI Trung Quốc",
        risk: "An toàn",
        description: "Trợ lý AI trả lời cuộc gọi tự động bằng tiếng Trung.",
        default_selected: true,
    },
];

// ---------------------------------------------------------------------------
// Data Models
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct DeviceHyperOsInfo {
    pub model: String,
    pub brand: String,
    pub android: String,
    pub sdk: String,
    pub hyperos: String,
    pub system_locales: String,
    pub adb_enabled: String,
    pub dev_enabled: String,
    pub millet_raw: String,
    pub millet_count: usize,
}

#[derive(Clone, Debug)]
pub struct NotificationAppState {
    pub package: String,
    pub name: String,
    pub category: String,
    pub installed: bool,
    pub in_millet: bool,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct NotificationFixReport {
    pub millet_ok: bool,
    pub millet_total: usize,
    pub doze_ok: usize,
    pub appops_ok: usize,
    pub total_pkgs: usize,
}

#[derive(Clone, Debug, Default)]
pub struct LocaleStatus {
    pub current: String,
    pub backup: String,
    pub has_backup: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebloatStatus {
    Installed,
    DisabledUser0,
    RemovedUser0,
    NotPresent,
}

impl DebloatStatus {
    pub fn label(self) -> &'static str {
        match self {
            DebloatStatus::Installed => "Đang cài",
            DebloatStatus::DisabledUser0 => "Đã tắt (User 0)",
            DebloatStatus::RemovedUser0 => "Đã gỡ (User 0)",
            DebloatStatus::NotPresent => "Không có trên ROM",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DebloatState {
    pub package: String,
    pub name: String,
    pub category: String,
    pub risk: &'static str,
    pub description: &'static str,
    pub status: DebloatStatus,
    pub installed: bool,
    pub restorable: bool,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct StagedMtz {
    pub theme_uuid: String,
    pub title: String,
    pub module_count: usize,
    pub file_count: usize,
    pub stage_dir: PathBuf,
    pub staged_rel_files: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DeviceBackup {
    #[serde(default)]
    locale_original: Option<String>,
    #[serde(default)]
    locale_saved_at: Option<String>,
    #[serde(default)]
    millet_original: Option<String>,
    #[serde(default)]
    millet_saved_at: Option<String>,
    #[serde(default)]
    uninstalled_packages: Vec<String>,
}

// ---------------------------------------------------------------------------
// Pure-Rust MD5 (RFC 1321) & SHA-1 (FIPS 180-4) & Java UUIDv3
// ---------------------------------------------------------------------------

pub fn md5_bytes(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20,
        5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let j = i * 4;
            *word = u32::from_le_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Exact replica of `java.util.UUID.nameUUIDFromBytes(byte[])` (MD5 UUIDv3).
pub fn java_name_uuid_from_bytes(data: &[u8]) -> String {
    let mut digest = md5_bytes(data);
    digest[6] = (digest[6] & 0x0f) | 0x30;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0],
        digest[1],
        digest[2],
        digest[3],
        digest[4],
        digest[5],
        digest[6],
        digest[7],
        digest[8],
        digest[9],
        digest[10],
        digest[11],
        digest[12],
        digest[13],
        digest[14],
        digest[15]
    )
}

pub fn sha1_bytes(input: &[u8]) -> String {
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xefcd_ab89;
    let mut h2: u32 = 0x98ba_dcfe;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xc3d2_e1f0;

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            let j = i * 4;
            w[i] = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdcu32),
                _ => (b ^ c ^ d, 0xca62_c1d6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    format!("{h0:08x}{h1:08x}{h2:08x}{h3:08x}{h4:08x}")
}

pub fn sha1_file(path: &Path) -> Result<String, String> {
    let data = fs::read(path).map_err(|e| format!("Không đọc được {}: {e}", path.display()))?;
    Ok(sha1_bytes(&data))
}

// ---------------------------------------------------------------------------
// Path, ADB & Command Helpers
// ---------------------------------------------------------------------------

pub fn resolve_themes_dir() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("themes"));
            if let Some(target_dir) = dir.parent() {
                if let Some(root) = target_dir.parent() {
                    candidates.push(root.join("themes"));
                    candidates.push(root.join("hyperos-pc-tool").join("themes"));
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("themes"));
        candidates.push(cwd.join("hyperos-pc-tool").join("themes"));
    }

    for cand in &candidates {
        if cand.join("MTZOS4.mtz").is_file() {
            return cand.clone();
        }
    }
    for cand in &candidates {
        if cand.is_dir() {
            return cand.clone();
        }
    }
    candidates.into_iter().next().unwrap_or_else(|| PathBuf::from("themes"))
}

pub fn default_backups_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("android-dashboard")
        .join("hyperos-backups")
}

fn verify_adb_binary(path: &str) -> bool {
    if path.trim().is_empty() {
        return false;
    }
    let mut cmd = Command::new(path);
    cmd.arg("version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    match cmd.output() {
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            out.status.success() && text.contains("Android Debug Bridge")
        }
        Err(_) => false,
    }
}

pub fn resolve_adb_verified(adb_bin: Option<&str>) -> String {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(custom) = adb_bin.map(str::trim).filter(|s| !s.is_empty()) {
        candidates.push(custom.to_string());
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let sdk = PathBuf::from(local)
            .join("Android")
            .join("Sdk")
            .join("platform-tools")
            .join("adb.exe");
        candidates.push(sdk.display().to_string());
    }
    for env_key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(sdk) = std::env::var_os(env_key) {
            let p = PathBuf::from(sdk).join("platform-tools").join("adb.exe");
            candidates.push(p.display().to_string());
        }
    }
    candidates.push(crate::flash::resolve_adb(adb_bin));
    candidates.push("adb.exe".into());
    candidates.push("adb".into());

    for cand in candidates {
        if verify_adb_binary(&cand) {
            return cand;
        }
    }
    crate::flash::resolve_adb(adb_bin)
}

fn run_command_capture(program: &str, args: &[String], timeout: Duration) -> Result<(bool, String), String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Không thể chạy '{program}': {e}"))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stdout_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stderr_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(35)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = t_out.join();
                let _ = t_err.join();
                return Err(format!("Lệnh '{program}' quá thời gian chờ ({}s).", timeout.as_secs()));
            }
        }
    };

    let out_bytes = t_out.join().unwrap_or_default();
    let err_bytes = t_err.join().unwrap_or_default();
    let out_str = String::from_utf8_lossy(&out_bytes).trim().to_string();
    let err_str = String::from_utf8_lossy(&err_bytes).trim().to_string();

    let combined = match (out_str.is_empty(), err_str.is_empty()) {
        (false, false) => format!("{out_str}\n{err_str}"),
        (false, true) => out_str,
        (true, false) => err_str,
        (true, true) => String::new(),
    };
    Ok((status.success(), combined))
}

fn run_adb_shell(adb: &str, serial: &str, script: &str, timeout: Duration) -> Result<(bool, String), String> {
    let args = vec![
        "-s".to_string(),
        serial.to_string(),
        "shell".to_string(),
        script.to_string(),
    ];
    run_command_capture(adb, &args, timeout)
}

fn run_adb_push(adb: &str, serial: &str, local: &Path, remote: &str, timeout: Duration) -> Result<(bool, String), String> {
    let local_str = local
        .to_str()
        .ok_or_else(|| format!("Đường dẫn không hợp lệ: {}", local.display()))?
        .to_string();
    let args = vec![
        "-s".to_string(),
        serial.to_string(),
        "push".to_string(),
        local_str,
        remote.to_string(),
    ];
    run_command_capture(adb, &args, timeout)
}

// ---------------------------------------------------------------------------
// Validation & Backups
// ---------------------------------------------------------------------------

pub fn is_valid_package(pkg: &str) -> bool {
    let p = pkg.trim();
    p.contains('.')
        && !p.is_empty()
        && p.len() <= 200
        && p.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

pub fn is_valid_locale_tag(tag: &str) -> bool {
    let re = Regex::new(r"^[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8})*(?:,[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8})*)*$")
        .expect("locale regex");
    !tag.is_empty() && tag.len() <= 120 && re.is_match(tag)
}

fn safe_serial(serial: &str) -> String {
    serial
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect()
}

fn backup_file_path(backups_dir: &Path, serial: &str) -> PathBuf {
    backups_dir.join(format!("{}_backup.json", safe_serial(serial)))
}

fn load_backup(backups_dir: &Path, serial: &str) -> DeviceBackup {
    let path = backup_file_path(backups_dir, serial);
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<DeviceBackup>(&s).ok())
        .unwrap_or_default()
}

fn save_backup(backups_dir: &Path, serial: &str, data: &DeviceBackup) {
    let _ = fs::create_dir_all(backups_dir);
    let path = backup_file_path(backups_dir, serial);
    if let Ok(json_bytes) = serde_json::to_vec_pretty(data) {
        let _ = fs::write(path, json_bytes);
    }
}

fn now_timestamp_label() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

fn get_setting(adb: &str, serial: &str, namespace: &str, key: &str) -> String {
    let cmd = format!("settings get {namespace} {key}");
    match run_adb_shell(adb, serial, &cmd, Duration::from_secs(10)) {
        Ok((true, out)) => {
            let v = out.trim();
            if v == "null" { String::new() } else { v.to_string() }
        }
        _ => String::new(),
    }
}

fn list_packages_cmd(adb: &str, serial: &str, cmd: &str) -> Result<HashSet<String>, String> {
    let (ok, out) = run_adb_shell(adb, serial, cmd, Duration::from_secs(20))?;
    if !ok && out.is_empty() {
        return Err(format!("Không lấy được danh sách gói từ `{cmd}`."));
    }
    let mut pkgs = HashSet::new();
    for line in out.lines() {
        let trimmed = line.trim();
        if let Some(pkg) = trimmed.strip_prefix("package:") {
            let p = pkg.trim();
            if !p.is_empty() {
                pkgs.insert(p.to_string());
            }
        }
    }
    Ok(pkgs)
}

// ---------------------------------------------------------------------------
// Device Info & Feature 1: Fix Notifications
// ---------------------------------------------------------------------------

pub fn parse_millet_list(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() || raw.trim() == "null" {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for part in raw.split(',') {
        let pkg = part.trim();
        if !pkg.is_empty() && seen.insert(pkg.to_string()) {
            items.push(pkg.to_string());
        }
    }
    items
}

pub fn fetch_device_info(adb_bin: Option<&str>, serial: &str) -> Result<DeviceHyperOsInfo, String> {
    let adb = resolve_adb_verified(adb_bin);
    let script = concat!(
        "echo MODEL=$(getprop ro.product.model); ",
        "echo BRAND=$(getprop ro.product.brand); ",
        "echo ANDROID=$(getprop ro.build.version.release); ",
        "echo SDK=$(getprop ro.build.version.sdk); ",
        "echo OS_VER=$(getprop ro.mi.os.version.name); ",
        "echo MIUI_VER=$(getprop ro.miui.ui.version.name); ",
        "echo LOCALE=$(settings get system system_locales); ",
        "echo ADB_EN=$(settings get global adb_enabled); ",
        "echo DEV_EN=$(settings get global development_settings_enabled); ",
        "echo MILLET=$(settings get system MILLET_NO_RESTRICT_APP)"
    );
    let (ok, out) = run_adb_shell(&adb, serial, script, Duration::from_secs(15))?;
    if !ok && out.is_empty() {
        return Err("Không đọc được thông tin từ thiết bị ADB.".into());
    }

    let mut info = DeviceHyperOsInfo::default();
    let mut miui_ver = String::new();
    for line in out.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let val = v.trim();
            let clean = if val == "null" { "" } else { val }.to_string();
            match k.trim() {
                "MODEL" => info.model = clean,
                "BRAND" => info.brand = clean,
                "ANDROID" => info.android = clean,
                "SDK" => info.sdk = clean,
                "OS_VER" => info.hyperos = clean,
                "MIUI_VER" => miui_ver = clean,
                "LOCALE" => info.system_locales = clean,
                "ADB_EN" => info.adb_enabled = clean,
                "DEV_EN" => info.dev_enabled = clean,
                "MILLET" => info.millet_raw = clean,
                _ => {}
            }
        }
    }
    if info.hyperos.is_empty() {
        info.hyperos = miui_ver;
    }
    info.millet_count = parse_millet_list(&info.millet_raw).len();
    Ok(info)
}

pub fn scan_notification_apps(adb_bin: Option<&str>, serial: &str) -> Result<Vec<NotificationAppState>, String> {
    let adb = resolve_adb_verified(adb_bin);
    let installed = list_packages_cmd(&adb, serial, "pm list packages --user 0")
        .or_else(|_| list_packages_cmd(&adb, serial, "pm list packages"))?;
    let millet_raw = get_setting(&adb, serial, "system", "MILLET_NO_RESTRICT_APP");
    let millet_set: HashSet<String> = parse_millet_list(&millet_raw).into_iter().collect();

    Ok(NOTIFICATION_APPS
        .iter()
        .map(|app| {
            let is_inst = installed.contains(app.package);
            NotificationAppState {
                package: app.package.to_string(),
                name: app.name.to_string(),
                category: app.category.to_string(),
                installed: is_inst,
                in_millet: millet_set.contains(app.package),
                selected: is_inst && app.default_selected,
            }
        })
        .collect())
}

pub fn read_millet_list(adb_bin: Option<&str>, serial: &str) -> Vec<String> {
    let adb = resolve_adb_verified(adb_bin);
    let raw = get_setting(&adb, serial, "system", "MILLET_NO_RESTRICT_APP");
    parse_millet_list(&raw)
}

pub fn apply_notification_fix(
    adb_bin: Option<&str>,
    serial: &str,
    packages: &[String],
    backups_dir: &Path,
    send_fcm_ping: bool,
    mut on_log: impl FnMut(String),
) -> Result<NotificationFixReport, String> {
    let adb = resolve_adb_verified(adb_bin);
    let mut clean_pkgs = Vec::new();
    for p in packages {
        let pkg = p.trim();
        if pkg.is_empty() {
            continue;
        }
        if !is_valid_package(pkg) {
            return Err(format!("Tên package không hợp lệ: {pkg}"));
        }
        if !clean_pkgs.iter().any(|x| x == pkg) {
            clean_pkgs.push(pkg.to_string());
        }
    }
    if clean_pkgs.is_empty() {
        return Err("Chưa chọn ứng dụng nào để tối ưu thông báo.".into());
    }

    let current_raw = get_setting(&adb, serial, "system", "MILLET_NO_RESTRICT_APP");
    let mut backup = load_backup(backups_dir, serial);
    if backup.millet_original.is_none() {
        backup.millet_original = Some(current_raw.clone());
        backup.millet_saved_at = Some(now_timestamp_label());
        save_backup(backups_dir, serial, &backup);
        on_log(format!(
            "Đã sao lưu MILLET_NO_RESTRICT_APP gốc ({} mục).",
            parse_millet_list(&current_raw).len()
        ));
    }

    let mut merged = parse_millet_list(&current_raw);
    let mut added = 0usize;
    for pkg in &clean_pkgs {
        if !merged.iter().any(|x| x == pkg) {
            merged.push(pkg.clone());
            added += 1;
        }
    }
    let millet_csv = merged.join(",");
    let (put_ok, put_out) = run_adb_shell(
        &adb,
        serial,
        &format!("settings put system MILLET_NO_RESTRICT_APP '{millet_csv}'"),
        Duration::from_secs(15),
    )?;
    let verify_list = parse_millet_list(&get_setting(&adb, serial, "system", "MILLET_NO_RESTRICT_APP"));
    let millet_ok = put_ok && clean_pkgs.iter().all(|p| verify_list.contains(p));
    if millet_ok {
        on_log(format!(
            "[MILLET] Đã cập nhật MILLET_NO_RESTRICT_APP (+{added} mới, tổng {} gói).",
            merged.len()
        ));
    } else {
        on_log(format!(
            "[MILLET] Cảnh báo khi ghi MILLET_NO_RESTRICT_APP: {}",
            if put_out.is_empty() { "ROM từ chối ghi" } else { &put_out }
        ));
    }

    let mut doze_ok = 0usize;
    let mut appops_ok = 0usize;
    for pkg in &clean_pkgs {
        let r_doze = run_adb_shell(
            &adb,
            serial,
            &format!("dumpsys deviceidle whitelist +{pkg}"),
            Duration::from_secs(10),
        )
        .map(|(ok, _)| ok)
        .unwrap_or(false);
        if r_doze {
            doze_ok += 1;
        }
        let r_ops = run_adb_shell(
            &adb,
            serial,
            &format!("cmd appops set {pkg} RUN_ANY_IN_BACKGROUND allow"),
            Duration::from_secs(10),
        )
        .map(|(ok, _)| ok)
        .unwrap_or(false);
        if r_ops {
            appops_ok += 1;
        }
        on_log(format!(
            "  • {pkg}: Doze={} | AppOps={}",
            if r_doze { "OK" } else { "Lỗi" },
            if r_ops { "OK" } else { "Lỗi" }
        ));
    }

    if send_fcm_ping {
        for target_pkg in ["com.google.android.gms", "com.google.android.gsf"] {
            for action in [
                "com.google.android.intent.action.GTALK_HEARTBEAT",
                "com.google.android.intent.action.MCS_HEARTBEAT",
            ] {
                let _ = run_adb_shell(
                    &adb,
                    serial,
                    &format!("am broadcast -a {action} -p {target_pkg}"),
                    Duration::from_secs(10),
                );
            }
        }
        on_log("[FCM] Đã gửi tín hiệu đánh thức FCM một lần (GTALK/MCS_HEARTBEAT), không cài dịch vụ ngầm.".into());
    }

    Ok(NotificationFixReport {
        millet_ok,
        millet_total: merged.len(),
        doze_ok,
        appops_ok,
        total_pkgs: clean_pkgs.len(),
    })
}

pub fn restore_millet_backup(
    adb_bin: Option<&str>,
    serial: &str,
    backups_dir: &Path,
    mut on_log: impl FnMut(String),
) -> Result<String, String> {
    let adb = resolve_adb_verified(adb_bin);
    let backup = load_backup(backups_dir, serial);
    let orig = backup
        .millet_original
        .ok_or_else(|| "Chưa có bản sao lưu MILLET_NO_RESTRICT_APP cho thiết bị này.".to_string())?;
    let cmd = if orig.is_empty() {
        "settings delete system MILLET_NO_RESTRICT_APP".to_string()
    } else {
        format!("settings put system MILLET_NO_RESTRICT_APP '{orig}'")
    };
    let (ok, out) = run_adb_shell(&adb, serial, &cmd, Duration::from_secs(15))?;
    let msg = if ok {
        "Đã khôi phục MILLET_NO_RESTRICT_APP gốc.".to_string()
    } else {
        format!("Khôi phục MILLET thất bại: {out}")
    };
    on_log(msg.clone());
    if ok { Ok(msg) } else { Err(msg) }
}

// ---------------------------------------------------------------------------
// Feature 2A: Locale
// ---------------------------------------------------------------------------

pub fn get_locale_status(adb_bin: Option<&str>, serial: &str, backups_dir: &Path) -> LocaleStatus {
    let adb = resolve_adb_verified(adb_bin);
    let current = get_setting(&adb, serial, "system", "system_locales");
    let backup = load_backup(backups_dir, serial);
    LocaleStatus {
        current: if current.is_empty() {
            "(chưa đặt / mặc định ROM)".into()
        } else {
            current
        },
        backup: backup
            .locale_original
            .clone()
            .unwrap_or_else(|| "(chưa sao lưu)".into()),
        has_backup: backup.locale_original.is_some(),
    }
}

pub fn set_system_locale(
    adb_bin: Option<&str>,
    serial: &str,
    locale_tag: &str,
    backups_dir: &Path,
    mut on_log: impl FnMut(String),
) -> Result<String, String> {
    let tag = locale_tag.trim().replace('_', "-");
    if !is_valid_locale_tag(&tag) {
        return Err(format!("Mã ngôn ngữ không hợp lệ: {locale_tag}"));
    }

    let adb = resolve_adb_verified(adb_bin);
    let current = get_setting(&adb, serial, "system", "system_locales");
    let mut backup = load_backup(backups_dir, serial);
    if backup.locale_original.is_none() {
        backup.locale_original = Some(current.clone());
        backup.locale_saved_at = Some(now_timestamp_label());
        save_backup(backups_dir, serial, &backup);
        on_log(format!(
            "Đã sao lưu system_locales gốc: '{}'.",
            if current.is_empty() { "(trống)" } else { &current }
        ));
    }

    let (ok, out) = run_adb_shell(
        &adb,
        serial,
        &format!("settings put system system_locales '{tag}'"),
        Duration::from_secs(15),
    )?;
    let after = get_setting(&adb, serial, "system", "system_locales");
    if ok && after == tag {
        let msg = format!(
            "Đã đặt system_locales = '{after}'. Khởi động lại máy hoặc nạp Theme MTZ để Việt hóa sâu hơn."
        );
        on_log(msg.clone());
        Ok(msg)
    } else {
        let err = format!(
            "Không đặt được system_locales: {}",
            if out.is_empty() {
                format!("đọc lại được '{after}'")
            } else {
                out
            }
        );
        on_log(err.clone());
        Err(err)
    }
}

pub fn restore_locale_backup(
    adb_bin: Option<&str>,
    serial: &str,
    backups_dir: &Path,
    mut on_log: impl FnMut(String),
) -> Result<String, String> {
    let adb = resolve_adb_verified(adb_bin);
    let backup = load_backup(backups_dir, serial);
    let orig = backup
        .locale_original
        .ok_or_else(|| "Chưa có bản sao lưu ngôn ngữ gốc cho thiết bị này.".to_string())?;
    let cmd = if orig.is_empty() {
        "settings delete system system_locales".to_string()
    } else {
        format!("settings put system system_locales '{orig}'")
    };
    let (ok, out) = run_adb_shell(&adb, serial, &cmd, Duration::from_secs(15))?;
    if ok {
        let msg = format!(
            "Đã khôi phục system_locales về '{}'.",
            if orig.is_empty() { "(mặc định ROM)" } else { &orig }
        );
        on_log(msg.clone());
        Ok(msg)
    } else {
        let err = format!("Khôi phục system_locales thất bại: {out}");
        on_log(err.clone());
        Err(err)
    }
}

// ---------------------------------------------------------------------------
// Feature 2B: MTZ Packager & Deployer (exact defpackage.d6 + defpackage.lstate)
// ---------------------------------------------------------------------------

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn new_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{prefix}_{}_{nonce}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("Không tạo được thư mục tạm: {e}"))?;
    Ok(dir)
}

fn localized_str(v: &str) -> Value {
    json!({ "fallback": v, "zh_CN": v })
}

fn localized_list(items: &[String]) -> Value {
    json!({
        "fallback": items,
        "zh_CN": items,
        "en_US": items,
        "vi_VN": items,
    })
}

fn res_ref(local_id: &str, resource_code: &str) -> Value {
    json!({
        "localId": local_id,
        "resourceCode": resource_code,
        "extraMeta": {},
        "metaPath": Value::Null,
        "contentPath": Value::Null,
    })
}

fn apply_common_meta(
    obj: &mut Value,
    platform: i64,
    author: &str,
    designer: &str,
    title: &str,
    description: &str,
    local_id: &str,
    file_hash: &str,
    size: u64,
) {
    if let Some(map) = obj.as_object_mut() {
        map.insert("localId".into(), json!(local_id));
        map.insert("hash".into(), json!(file_hash));
        map.insert("size".into(), json!(size));
        map.insert("platform".into(), json!(platform));
        map.insert("authors".into(), localized_str(author));
        map.insert("designers".into(), localized_str(designer));
        map.insert("titles".into(), localized_str(title));
        map.insert("descriptions".into(), localized_str(description));
    }
}

fn map_module_name(name: &str) -> &str {
    match name {
        "com.miui.home" => "launcher",
        "com.android.systemui" => "statusbar",
        "com.android.contacts" => "contact",
        other => other,
    }
}

fn is_valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn validate_archive_entries(mtz_path: &Path) -> Result<(), String> {
    let mtz_str = mtz_path
        .to_str()
        .ok_or("Đường dẫn MTZ không phải Unicode hợp lệ.")?
        .to_string();

    #[cfg(windows)]
    let (prog, args) = ("tar", vec!["-tf".to_string(), mtz_str]);
    #[cfg(not(windows))]
    let (prog, args) = ("unzip", vec!["-Z1".to_string(), mtz_str]);

    let (ok, out) = run_command_capture(prog, &args, Duration::from_secs(60))?;
    if !ok {
        return Err(format!("Không đọc được danh sách tệp trong MTZ ({prog}): {out}"));
    }

    let mut seen = HashSet::new();
    let mut file_count = 0usize;
    for raw in out.lines() {
        let norm = raw.trim_end_matches('\r').replace('\\', "/");
        if norm.is_empty() {
            continue;
        }
        if norm.starts_with('/') || norm.starts_with("./") || norm.contains('\0') {
            return Err(format!("Đường dẫn không hợp lệ trong MTZ: {raw}"));
        }
        let parts: Vec<&str> = norm.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() || parts.iter().any(|p| *p == ".." || *p == ".") {
            return Err(format!("Đường dẫn không an toàn trong MTZ: {raw}"));
        }
        if norm.ends_with('/') {
            continue;
        }
        file_count += 1;
        if file_count > MAX_ZIP_ENTRIES {
            return Err("MTZ chứa quá nhiều tệp.".into());
        }
        if !seen.insert(norm.clone()) {
            return Err(format!("Trùng tệp trong MTZ: {norm}"));
        }
    }
    Ok(())
}

fn extract_archive(mtz_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let mtz_str = mtz_path
        .to_str()
        .ok_or("Đường dẫn MTZ không phải Unicode hợp lệ.")?
        .to_string();
    let dest_str = dest_dir
        .to_str()
        .ok_or("Đường dẫn tạm không phải Unicode hợp lệ.")?
        .to_string();

    #[cfg(windows)]
    let (prog, args) = (
        "tar",
        vec!["-xf".to_string(), mtz_str, "-C".to_string(), dest_str],
    );
    #[cfg(not(windows))]
    let (prog, args) = (
        "unzip",
        vec!["-o".to_string(), "-q".to_string(), mtz_str, "-d".to_string(), dest_str],
    );

    let (ok, out) = run_command_capture(prog, &args, Duration::from_secs(120))?;
    if !ok {
        return Err(format!("Không giải nén được MTZ ({prog}): {out}"));
    }
    Ok(())
}

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let pat = format!(r"(?s)<{tag}\b[^>]*>(.*?)</{tag}>");
    let re = Regex::new(&pat).ok()?;
    let cap = re.captures(xml)?;
    let inner = cap.get(1)?.as_str().trim();
    Some(unescape_xml(inner))
}

fn write_staged_bytes(
    stage_dir: &Path,
    rel_path: &str,
    data: &[u8],
    staged_files: &mut Vec<(String, String)>,
) -> Result<String, String> {
    let rel_norm = rel_path.replace('\\', "/").trim_start_matches('/').to_string();
    let parts: Vec<&str> = rel_norm.split('/').collect();
    if parts.is_empty() || !is_valid_segment(parts[0]) || parts.iter().any(|p| *p == ".." || *p == ".") {
        return Err(format!("Đường dẫn đích không an toàn: {rel_path}"));
    }
    let mut dest = stage_dir.to_path_buf();
    for p in &parts {
        dest.push(p);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Không tạo được thư mục staged: {e}"))?;
    }
    fs::write(&dest, data).map_err(|e| format!("Không ghi được {}: {e}", dest.display()))?;
    let hash = sha1_bytes(data);
    staged_files.push((rel_norm, hash.clone()));
    Ok(hash)
}

#[cfg(windows)]
fn convert_png_file_to_jpg_bytes(png_path: &Path) -> Option<Vec<u8>> {
    use std::os::windows::process::CommandExt;

    let tmp_jpg = png_path.with_extension("converted_cover.jpg");
    let src_escaped = png_path.display().to_string().replace('\'', "''");
    let dst_escaped = tmp_jpg.display().to_string().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; \
         $img = [System.Drawing.Image]::FromFile('{src_escaped}'); \
         $img.Save('{dst_escaped}', [System.Drawing.Imaging.ImageFormat]::Jpeg); \
         $img.Dispose()"
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .output()
        .ok()?;
    let bytes = if out.status.success() {
        fs::read(&tmp_jpg).ok()
    } else {
        None
    };
    let _ = fs::remove_file(&tmp_jpg);
    bytes
}

#[cfg(not(windows))]
fn convert_png_file_to_jpg_bytes(_png_path: &Path) -> Option<Vec<u8>> {
    None
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

pub fn prepare_mtz_stage(
    mtz_path: &Path,
    template_path_opt: Option<&Path>,
    stage_dir: &Path,
    mut on_log: impl FnMut(String),
) -> Result<StagedMtz, String> {
    if !mtz_path.is_file() {
        return Err(format!("Không tìm thấy file MTZ: {}", mtz_path.display()));
    }
    let mtz_size = fs::metadata(mtz_path)
        .map_err(|e| format!("Không đọc được metadata MTZ: {e}"))?
        .len();
    if mtz_size > MAX_MTZ_BYTES {
        return Err("File MTZ vượt giới hạn 350 MB.".into());
    }

    let template_json = match template_path_opt {
        Some(p) if p.is_file() => {
            fs::read_to_string(p).map_err(|e| format!("Không đọc được {}: {e}", p.display()))?
        }
        _ => EMBEDDED_METADATA_TEMPLATE.to_string(),
    };
    let templates: Value = serde_json::from_str(&template_json)
        .map_err(|e| format!("metadata_template.json không hợp lệ: {e}"))?;

    let mtz_sha1 = sha1_file(mtz_path)?;
    let theme_uuid = java_name_uuid_from_bytes(format!("mtztool-theme:{mtz_sha1}").as_bytes());
    on_log(format!(
        "MTZ SHA-1: {}... -> UUID: {theme_uuid}",
        &mtz_sha1[..12]
    ));

    validate_archive_entries(mtz_path)?;
    let unpack_dir = new_temp_dir("mtz_unpack")?;
    let _unpack_guard = TempDirGuard(unpack_dir.clone());
    extract_archive(mtz_path, &unpack_dir)?;

    let desc_path = unpack_dir.join("description.xml");
    if !desc_path.is_file() {
        return Err("MTZ thiếu description.xml.".into());
    }
    let desc_bytes = fs::read(&desc_path).map_err(|e| format!("Không đọc được description.xml: {e}"))?;
    if desc_bytes.len() > 1_048_576 {
        return Err("description.xml trong MTZ quá lớn.".into());
    }
    let desc_str = String::from_utf8_lossy(&desc_bytes);

    let title_raw = extract_xml_tag(&desc_str, "title").unwrap_or_default();
    let title = {
        let t = truncate_chars(title_raw.trim(), 200);
        if t.is_empty() { "Chủ đề MTZ".to_string() } else { t }
    };
    let author = truncate_chars(
        extract_xml_tag(&desc_str, "author").unwrap_or_default().trim(),
        120,
    );
    let designer_raw = truncate_chars(
        extract_xml_tag(&desc_str, "designer").unwrap_or_default().trim(),
        120,
    );
    let designer = if designer_raw.is_empty() { author.clone() } else { designer_raw };
    let description = truncate_chars(
        extract_xml_tag(&desc_str, "description").unwrap_or_default().trim(),
        4000,
    );
    let ui_version = extract_xml_tag(&desc_str, "uiVersion")
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&v| (1..1000).contains(&v))
        .unwrap_or(17);

    // Collect root-level modules & preview/ files
    let mut modules: Vec<(String, PathBuf)> = Vec::new();
    let mut seen_mapped = HashSet::new();
    let top_entries = fs::read_dir(&unpack_dir).map_err(|e| format!("Không đọc thư mục giải nén: {e}"))?;
    for entry in top_entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname == "description.xml" {
                continue;
            }
            if !is_valid_segment(&fname) {
                return Err(format!("Tên module không hợp lệ: {fname}"));
            }
            let fsize = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if fsize > MAX_ENTRY_BYTES {
                return Err(format!("Tệp trong MTZ quá lớn: {fname}"));
            }
            let mapped = map_module_name(&fname).to_string();
            if !seen_mapped.insert(mapped.clone()) {
                return Err(format!("Tên module bị trùng sau khi ánh xạ: {mapped}"));
            }
            modules.push((mapped, path));
        }
    }
    if modules.is_empty() {
        return Err("MTZ không chứa module nào có thể nạp.".into());
    }
    modules.sort_by(|a, b| a.0.cmp(&b.0));

    let mut previews: Vec<(String, PathBuf)> = Vec::new();
    let preview_dir = unpack_dir.join("preview");
    if preview_dir.is_dir() {
        if let Ok(rd) = fs::read_dir(&preview_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    let low = fname.to_ascii_lowercase();
                    if is_valid_segment(&fname)
                        && (low.ends_with(".jpg") || low.ends_with(".jpeg") || low.ends_with(".png"))
                    {
                        previews.push((fname, p));
                    }
                }
            }
        }
    }
    previews.sort_by(|a, b| a.0.cmp(&b.0));

    fs::create_dir_all(stage_dir).map_err(|e| format!("Không tạo được stage_dir: {e}"))?;
    let mut staged_files: Vec<(String, String)> = Vec::new();
    let mut total_unpacked: u64 = 0;

    // 1. Empty theme .mrc
    write_staged_bytes(
        stage_dir,
        &format!("content/theme/{theme_uuid}.mrc"),
        &[],
        &mut staged_files,
    )?;

    // 2. Theme root JSON
    let mut theme_meta = templates
        .get("theme")
        .cloned()
        .ok_or("metadata_template.json thiếu mục 'theme'.")?;
    apply_common_meta(
        &mut theme_meta,
        ui_version,
        &author,
        &designer,
        &title,
        &description,
        &theme_uuid,
        &mtz_sha1,
        mtz_size,
    );
    if let Some(m) = theme_meta.as_object_mut() {
        m.insert(
            "contentPath".into(),
            json!(format!("{THEME_DATA_CONTENT_PREFIX}/{theme_uuid}.mrc")),
        );
    }

    // 3. Modules
    let mut sub_resources = Vec::new();
    for (mod_name, mod_path) in &modules {
        let mod_bytes = fs::read(mod_path).map_err(|e| format!("Không đọc module {mod_name}: {e}"))?;
        total_unpacked += mod_bytes.len() as u64;
        if total_unpacked > MAX_TOTAL_UNPACKED {
            return Err("Tổng dung lượng giải nén MTZ vượt giới hạn an toàn.".into());
        }

        let mod_uuid = java_name_uuid_from_bytes(
            format!("mtztool-module:{mtz_sha1}:{mod_name}").as_bytes(),
        );
        sub_resources.push(res_ref(&mod_uuid, mod_name));

        let mod_sha1 = write_staged_bytes(
            stage_dir,
            &format!("content/{mod_name}/{mod_uuid}.mrc"),
            &mod_bytes,
            &mut staged_files,
        )?;

        let tmpl_key = if mod_name == "launcher" { "launcher" } else { "component" };
        let mut mod_meta = templates
            .get(tmpl_key)
            .cloned()
            .ok_or_else(|| format!("metadata_template.json thiếu mục '{tmpl_key}'."))?;
        apply_common_meta(
            &mut mod_meta,
            ui_version,
            &author,
            &designer,
            &title,
            &description,
            &mod_uuid,
            &mod_sha1,
            mod_bytes.len() as u64,
        );
        if let Some(m) = mod_meta.as_object_mut() {
            m.insert("parentResources".into(), json!([res_ref(&theme_uuid, "theme")]));
            m.insert("subResources".into(), json!([]));

            if mod_name == "launcher" {
                let launcher_previews: Vec<String> = previews
                    .iter()
                    .filter(|(n, _)| n.to_ascii_lowercase().starts_with("preview_launcher_"))
                    .map(|(n, _)| n.clone())
                    .collect();
                let thumb: Vec<String> = launcher_previews.iter().take(1).cloned().collect();
                m.insert("builtInThumbnails".into(), localized_list(&thumb));
                m.insert("builtInPreviews".into(), localized_list(&launcher_previews));
                for lp in &launcher_previews {
                    if let Some((_, p_path)) = previews.iter().find(|(n, _)| n == lp) {
                        let lp_bytes = fs::read(p_path).map_err(|e| e.to_string())?;
                        total_unpacked += lp_bytes.len() as u64;
                        if total_unpacked > MAX_TOTAL_UNPACKED {
                            return Err("Tổng dung lượng giải nén MTZ vượt giới hạn an toàn.".into());
                        }
                        write_staged_bytes(
                            stage_dir,
                            &format!("preview/launcher/{mod_uuid}/{lp}"),
                            &lp_bytes,
                            &mut staged_files,
                        )?;
                    }
                }
            }
        }

        let mod_mrm = serde_json::to_string_pretty(&mod_meta).map_err(|e| e.to_string())?;
        write_staged_bytes(
            stage_dir,
            &format!("meta/{mod_name}/{mod_uuid}.mrm"),
            mod_mrm.as_bytes(),
            &mut staged_files,
        )?;
    }

    on_log(format!(
        "Đã đóng gói {} module cho \"{title}\" (platform={ui_version}).",
        modules.len()
    ));

    // 4. Previews & cover image
    let preview_keys: Vec<String> = previews.iter().map(|(n, _)| n.clone()).collect();
    let has_exact_cover = preview_keys.iter().any(|k| k == "preview_cover_0.jpg");
    let mut cover_src_key: Option<String> = if has_exact_cover {
        Some("preview_cover_0.jpg".into())
    } else {
        None
    };

    if !has_exact_cover && !preview_keys.is_empty() {
        for pfx in [
            "preview_cover_0.",
            "preview_launcher_0.",
            "preview_launcher_",
            "preview_0.",
            "preview_lockscreen_",
        ] {
            if let Some(found) = preview_keys
                .iter()
                .find(|k| k.to_ascii_lowercase().starts_with(pfx))
            {
                cover_src_key = Some(found.clone());
                break;
            }
        }
        if cover_src_key.is_none() {
            cover_src_key = preview_keys.first().cloned();
        }
        if let Some(ref src_name) = cover_src_key {
            if let Some((_, src_path)) = previews.iter().find(|(n, _)| n == src_name) {
                let cover_bytes = if src_name.to_ascii_lowercase().ends_with(".png") {
                    convert_png_file_to_jpg_bytes(src_path)
                        .unwrap_or_else(|| fs::read(src_path).unwrap_or_default())
                } else {
                    fs::read(src_path).map_err(|e| e.to_string())?
                };
                total_unpacked += cover_bytes.len() as u64;
                write_staged_bytes(
                    stage_dir,
                    &format!("preview/theme/{theme_uuid}/preview_cover_0.jpg"),
                    &cover_bytes,
                    &mut staged_files,
                )?;
            }
        }
    }

    let mut preview_list: Vec<String> = Vec::new();
    if has_exact_cover || cover_src_key.is_some() {
        preview_list.push("preview_cover_0.jpg".into());
    }
    for pk in &preview_keys {
        if pk != "preview_cover_0.jpg" {
            preview_list.push(pk.clone());
        }
    }

    for (pk, p_path) in &previews {
        let pk_bytes = fs::read(p_path).map_err(|e| e.to_string())?;
        total_unpacked += pk_bytes.len() as u64;
        if total_unpacked > MAX_TOTAL_UNPACKED {
            return Err("Tổng dung lượng giải nén MTZ vượt giới hạn an toàn.".into());
        }
        write_staged_bytes(
            stage_dir,
            &format!("preview/theme/{theme_uuid}/{pk}"),
            &pk_bytes,
            &mut staged_files,
        )?;
    }

    if let Some(m) = theme_meta.as_object_mut() {
        m.insert("subResources".into(), Value::Array(sub_resources));
        let thumb: Vec<String> = preview_list.iter().take(1).cloned().collect();
        m.insert("builtInThumbnails".into(), localized_list(&thumb));
        m.insert("builtInPreviews".into(), localized_list(&preview_list));
    }

    let theme_mrm = serde_json::to_string_pretty(&theme_meta).map_err(|e| e.to_string())?;
    write_staged_bytes(
        stage_dir,
        &format!("meta/theme/{theme_uuid}.mrm"),
        theme_mrm.as_bytes(),
        &mut staged_files,
    )?;

    Ok(StagedMtz {
        theme_uuid,
        title,
        module_count: modules.len(),
        file_count: staged_files.len(),
        stage_dir: stage_dir.to_path_buf(),
        staged_rel_files: staged_files,
    })
}

pub fn open_theme_apply_screen(
    adb_bin: Option<&str>,
    serial: &str,
    theme_uuid: &str,
    mut on_log: impl FnMut(String),
) -> bool {
    let adb = resolve_adb_verified(adb_bin);
    let cmd = format!(
        "am start -S --user 0 -a android.intent.action.VIEW \
         -n com.android.thememanager/.module.detail.view.ThemeDetailActivity \
         -d 'ViewLocalResource://view.local.resource#{theme_uuid}' \
         --es REQUEST_RESOURCE_CODE theme --ei REQUEST_SOURCE_TYPE 1"
    );
    match run_adb_shell(&adb, serial, &cmd, Duration::from_secs(20)) {
        Ok((ok, out)) if ok && !out.contains("Error") && !out.contains("Exception") => {
            on_log("Đã mở trang chi tiết Theme trên điện thoại — hãy bấm nút [Áp dụng] thủ công trên màn hình.".into());
            true
        }
        Ok((_, out)) => {
            on_log(format!(
                "Cảnh báo: không mở thẳng được ThemeDetailActivity ({out}). Hãy mở ứng dụng Chủ đề (Themes) trên điện thoại để áp dụng."
            ));
            false
        }
        Err(e) => {
            on_log(format!("Cảnh báo khi mở ThemeDetailActivity: {e}"));
            false
        }
    }
}

pub fn deploy_mtz_via_adb(
    adb_bin: Option<&str>,
    serial: &str,
    mtz_path: &Path,
    template_path_opt: Option<&Path>,
    overwrite: bool,
    open_apply_screen: bool,
    mut on_log: impl FnMut(String),
) -> Result<StagedMtz, String> {
    let adb = resolve_adb_verified(adb_bin);
    let temp_root = new_temp_dir("mtz_pc_root")?;
    let _guard = TempDirGuard(temp_root.clone());

    // First compute UUID inside stage directory named mtz_stage_<uuid> after staging
    let pre_stage = temp_root.join("stage");
    let staged = prepare_mtz_stage(mtz_path, template_path_opt, &pre_stage, |m| on_log(m))?;
    let uid = &staged.theme_uuid;

    if !overwrite {
        let chk_cmd = format!(
            "if test -e '{THEME_DATA_ROOT}/meta/theme/{uid}.mrm' && test -e '{THEME_DATA_ROOT}/content/theme/{uid}.mrc'; then echo MTZ_ALREADY; else echo MTZ_NEW; fi"
        );
        if let Ok((_, out)) = run_adb_shell(&adb, serial, &chk_cmd, Duration::from_secs(15)) {
            if out.contains("MTZ_ALREADY") {
                on_log(format!(
                    "Theme \"{}\" ({uid}) đã có sẵn trong Xiaomi Themes.",
                    staged.title
                ));
                if open_apply_screen {
                    self::open_theme_apply_screen(Some(&adb), serial, uid, |m| on_log(m));
                }
                return Ok(staged);
            }
        }
    }

    let remote_tmp = format!("/data/local/tmp/mtz_stage_{uid}");
    let named_local = temp_root.join(format!("mtz_stage_{uid}"));
    fs::rename(&pre_stage, &named_local)
        .map_err(|e| format!("Không chuẩn bị được thư mục đẩy: {e}"))?;

    let _ = run_adb_shell(
        &adb,
        serial,
        &format!("rm -rf '{remote_tmp}'"),
        Duration::from_secs(15),
    );
    on_log(format!(
        "Đang đẩy {} tệp lên vùng tạm ADB trên điện thoại...",
        staged.file_count
    ));
    let (push_ok, push_out) = run_adb_push(
        &adb,
        serial,
        &named_local,
        "/data/local/tmp/",
        Duration::from_secs(180),
    )?;
    if !push_ok {
        return Err(format!("adb push thất bại: {push_out}"));
    }

    let mut lines = vec![
        "set -eu".to_string(),
        format!("SRC='{remote_tmp}'"),
        format!("DST='{THEME_DATA_ROOT}'"),
        "mkdir -p \"$DST\"".to_string(),
    ];
    for (rel_path, expected_sha1) in &staged.staged_rel_files {
        if let Some(parent_rel) = Path::new(rel_path).parent().and_then(|p| p.to_str()) {
            let parent_norm = parent_rel.replace('\\', "/");
            if !parent_norm.is_empty() {
                lines.push(format!("mkdir -p \"$DST/{parent_norm}\""));
            }
        }
        lines.push(format!(
            "cp -f \"$SRC/{rel_path}\" \"$DST/{rel_path}\" && chmod 0660 \"$DST/{rel_path}\" 2>/dev/null || true"
        ));
        lines.push(format!(
            "test \"$(sha1sum \"$DST/{rel_path}\" | cut -d' ' -f1)\" = '{expected_sha1}' || {{ echo 'MTZ_SHA1_ERR:{rel_path}'; exit 1; }}"
        ));
    }
    lines.push(format!("rm -rf '{remote_tmp}'"));
    lines.push("echo MTZ_DEPLOY_OK".to_string());

    let local_script = temp_root.join("deploy.sh");
    fs::write(&local_script, format!("{}\n", lines.join("\n")))
        .map_err(|e| format!("Không ghi được script nạp theme: {e}"))?;

    let remote_script = format!("/data/local/tmp/mtz_deploy_{uid}.sh");
    let (sh_push_ok, sh_push_out) =
        run_adb_push(&adb, serial, &local_script, &remote_script, Duration::from_secs(30))?;
    if !sh_push_ok {
        return Err(format!("Không đẩy được script cài theme: {sh_push_out}"));
    }

    on_log("Đang nạp và xác minh SHA-1 vào thư mục Xiaomi Themes...".into());
    let (run_ok, run_out) = run_adb_shell(
        &adb,
        serial,
        &format!("sh '{remote_script}'; rc=$?; rm -f '{remote_script}'; exit $rc"),
        Duration::from_secs(120),
    )?;
    if !run_ok || !run_out.contains("MTZ_DEPLOY_OK") {
        let _ = run_adb_shell(
            &adb,
            serial,
            &format!("rm -rf '{remote_tmp}' '{remote_script}'"),
            Duration::from_secs(15),
        );
        return Err(format!("Nạp theme thất bại: {run_out}"));
    }

    on_log(format!(
        "Đã nạp thành công \"{}\" ({} tệp, khớp SHA-1 100%).",
        staged.title, staged.file_count
    ));
    if open_apply_screen {
        self::open_theme_apply_screen(Some(&adb), serial, uid, |m| on_log(m));
    }
    Ok(staged)
}

// ---------------------------------------------------------------------------
// Feature 3: Safe Debloat (User 0)
// ---------------------------------------------------------------------------

pub fn scan_debloat_packages(adb_bin: Option<&str>, serial: &str) -> Result<Vec<DebloatState>, String> {
    let adb = resolve_adb_verified(adb_bin);
    let enabled_user0 = list_packages_cmd(&adb, serial, "pm list packages -e --user 0")
        .or_else(|_| list_packages_cmd(&adb, serial, "pm list packages --user 0"))?;
    let disabled_user0 = list_packages_cmd(&adb, serial, "pm list packages -d --user 0")
        .unwrap_or_default();
    let all_with_uninstalled = list_packages_cmd(&adb, serial, "pm list packages -u")
        .unwrap_or_else(|_| enabled_user0.clone());

    Ok(DEBLOAT_PACKAGES
        .iter()
        .map(|item| {
            let is_enabled = enabled_user0.contains(item.package);
            let is_disabled = !is_enabled && disabled_user0.contains(item.package);
            let exists_uninstalled = !is_enabled && !is_disabled && all_with_uninstalled.contains(item.package);

            let status = if is_enabled {
                DebloatStatus::Installed
            } else if is_disabled {
                DebloatStatus::DisabledUser0
            } else if exists_uninstalled {
                DebloatStatus::RemovedUser0
            } else {
                DebloatStatus::NotPresent
            };
            DebloatState {
                package: item.package.to_string(),
                name: item.name.to_string(),
                category: item.category.to_string(),
                risk: item.risk,
                description: item.description,
                status,
                installed: is_enabled,
                restorable: is_disabled || exists_uninstalled,
                selected: is_enabled && item.default_selected,
            }
        })
        .collect())
}

pub fn uninstall_packages(
    adb_bin: Option<&str>,
    serial: &str,
    packages: &[String],
    backups_dir: &Path,
    mut on_log: impl FnMut(String),
) -> (usize, usize) {
    let adb = resolve_adb_verified(adb_bin);
    let mut backup = load_backup(backups_dir, serial);
    let mut hist: HashSet<String> = backup.uninstalled_packages.iter().cloned().collect();
    let (mut ok_cnt, mut fail_cnt) = (0usize, 0usize);

    for raw in packages {
        let pkg = raw.trim();
        if !is_valid_package(pkg) {
            on_log(format!("Bỏ qua package không hợp lệ: {pkg}"));
            fail_cnt += 1;
            continue;
        }
        // Tier 1: pm uninstall -k --user 0
        let r1 = run_adb_shell(
            &adb,
            serial,
            &format!("pm uninstall -k --user 0 {pkg}"),
            Duration::from_secs(20),
        );
        if matches!(&r1, Ok((true, out)) if out.contains("Success")) {
            ok_cnt += 1;
            hist.insert(pkg.to_string());
            on_log(format!("[GỠ OK] {pkg}"));
            continue;
        }
        let first_err = match &r1 {
            Ok((_, out)) => out.clone(),
            Err(e) => e.clone(),
        };
        if first_err.contains("not installed for 0") {
            hist.insert(pkg.to_string());
            on_log(format!("[ĐÃ GỠ SẴN] {pkg} (không còn cài cho User 0)"));
            continue;
        }

        // Tier 2: pm uninstall --user 0 (without -k, works for some /data/app preloads)
        let r2 = run_adb_shell(
            &adb,
            serial,
            &format!("pm uninstall --user 0 {pkg}"),
            Duration::from_secs(20),
        );
        if matches!(&r2, Ok((true, out)) if out.contains("Success")) {
            ok_cnt += 1;
            hist.insert(pkg.to_string());
            on_log(format!("[GỠ OK] {pkg}"));
            continue;
        }
        let second_err = match &r2 {
            Ok((_, out)) => out.clone(),
            Err(e) => e.clone(),
        };
        if second_err.contains("not installed for 0") {
            hist.insert(pkg.to_string());
            on_log(format!("[ĐÃ GỠ SẴN] {pkg} (không còn cài cho User 0)"));
            continue;
        }

        // Tier 3: pm disable-user --user 0 (fallback when HyperOS blocks uninstall)
        let r3 = run_adb_shell(
            &adb,
            serial,
            &format!("pm disable-user --user 0 {pkg}"),
            Duration::from_secs(20),
        );
        match r3 {
            Ok((true, out)) if out.to_ascii_lowercase().contains("disabled") || out.to_ascii_lowercase().contains("new state") => {
                ok_cnt += 1;
                hist.insert(pkg.to_string());
                on_log(format!("[VÔ HIỆU HÓA OK] {pkg} (ROM chặn gỡ, đã tắt cho User 0)"));
            }
            Ok((_, out)) => {
                fail_cnt += 1;
                on_log(format!("[THẤT BẠI] {pkg}: gỡ ({second_err}) | tắt ({out})"));
            }
            Err(e) => {
                fail_cnt += 1;
                on_log(format!("[LỖI] {pkg}: gỡ ({second_err}) | tắt ({e})"));
            }
        }
    }

    let mut sorted: Vec<String> = hist.into_iter().collect();
    sorted.sort();
    backup.uninstalled_packages = sorted;
    save_backup(backups_dir, serial, &backup);
    (ok_cnt, fail_cnt)
}

pub fn restore_packages(
    adb_bin: Option<&str>,
    serial: &str,
    packages: &[String],
    backups_dir: &Path,
    mut on_log: impl FnMut(String),
) -> (usize, usize) {
    let adb = resolve_adb_verified(adb_bin);
    let mut backup = load_backup(backups_dir, serial);
    let mut hist: HashSet<String> = backup.uninstalled_packages.iter().cloned().collect();
    let (mut ok_cnt, mut fail_cnt) = (0usize, 0usize);

    for raw in packages {
        let pkg = raw.trim();
        if !is_valid_package(pkg) {
            on_log(format!("Bỏ qua package không hợp lệ: {pkg}"));
            fail_cnt += 1;
            continue;
        }
        let mut res = run_adb_shell(
            &adb,
            serial,
            &format!("cmd package install-existing --user 0 {pkg}"),
            Duration::from_secs(20),
        );
        let first_ok = matches!(&res, Ok((true, out)) if out.to_ascii_lowercase().contains("installed"));
        if !first_ok {
            res = run_adb_shell(
                &adb,
                serial,
                &format!("cmd package install-existing {pkg}"),
                Duration::from_secs(20),
            );
        }
        let installed_ok = matches!(&res, Ok((true, out)) if out.to_ascii_lowercase().contains("installed"));
        let en_res = run_adb_shell(
            &adb,
            serial,
            &format!("pm enable --user 0 {pkg}"),
            Duration::from_secs(15),
        );
        let enabled_ok = matches!(&en_res, Ok((true, out)) if out.to_ascii_lowercase().contains("enabled") || out.to_ascii_lowercase().contains("new state"));

        if installed_ok || enabled_ok {
            ok_cnt += 1;
            hist.remove(pkg);
            on_log(format!("[KHÔI PHỤC OK] {pkg}"));
        } else {
            fail_cnt += 1;
            let detail = match res {
                Ok((_, out)) => out,
                Err(e) => e,
            };
            on_log(format!("[KHÔNG KHÔI PHỤC ĐƯỢC] {pkg}: {detail}"));
        }
    }

    let mut sorted: Vec<String> = hist.into_iter().collect();
    sorted.sort();
    backup.uninstalled_packages = sorted;
    save_backup(backups_dir, serial, &backup);
    (ok_cnt, fail_cnt)
}

// ---------------------------------------------------------------------------
// Feature 4: Bank-Ready Lock
// ---------------------------------------------------------------------------

pub fn lock_developer_and_adb(
    adb_bin: Option<&str>,
    serial: &str,
    mut on_log: impl FnMut(String),
) -> Result<String, String> {
    let adb = resolve_adb_verified(adb_bin);
    // CRITICAL: disable development_settings_enabled FIRST, then adb_enabled=0 in one shell call.
    let cmd = "settings put global development_settings_enabled 0 && settings put global adb_enabled 0";
    match run_adb_shell(&adb, serial, cmd, Duration::from_secs(10)) {
        Ok((true, _)) => {
            let msg = "Đã tắt Tùy chọn nhà phát triển và Gỡ lỗi USB (ADB). Máy đã sẵn sàng mở VNeID & App Ngân hàng!".to_string();
            on_log(msg.clone());
            Ok(msg)
        }
        Ok((false, out)) => {
            let msg = format!(
                "Đã gửi lệnh tắt Tùy chọn nhà phát triển & ADB (kết nối ADB ngắt ngay sau khi tắt là bình thường: {out}). Hãy rút cáp và kiểm tra."
            );
            on_log(msg.clone());
            Ok(msg)
        }
        Err(e) => {
            let msg = format!(
                "Đã gửi lệnh tắt Tùy chọn nhà phát triển & ADB (ADB ngắt kết nối: {e}). Hãy rút cáp USB và mở thử VNeID / App Ngân hàng."
            );
            on_log(msg.clone());
            Ok(msg)
        }
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_and_sha1_match_rfc_vectors() {
        assert_eq!(hex(&md5_bytes(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5_bytes(b"message digest")),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            sha1_bytes(b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(sha1_bytes(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn millet_and_validators_work() {
        assert_eq!(
            parse_millet_list("com.zing.zalo, com.VCB , ,com.zing.zalo"),
            vec!["com.zing.zalo", "com.VCB"]
        );
        assert!(parse_millet_list("null").is_empty());
        assert!(is_valid_package("vn.vneid"));
        assert!(!is_valid_package("com.app;reboot"));
        assert!(is_valid_locale_tag("vi-VN"));
        assert!(is_valid_locale_tag("vi-VN,en-US"));
        assert!(!is_valid_locale_tag("vi_VN;rm"));
    }

    #[test]
    fn prepare_mtz_stage_matches_python_reference() {
        let workspace_themes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes");
        let mtz_path = workspace_themes.join("MTZOS4.mtz");
        if !mtz_path.is_file() {
            return;
        }
        let stage_dir = new_temp_dir("rust_mtz_test").expect("temp dir");
        let _guard = TempDirGuard(stage_dir.clone());

        let staged = prepare_mtz_stage(&mtz_path, None, &stage_dir, |_| {})
            .expect("prepare_mtz_stage should succeed");

        assert_eq!(staged.theme_uuid, "090655d1-680c-3fed-82bd-1ee4be4c4e14");
        assert_eq!(staged.module_count, 14);
        assert_eq!(staged.file_count, 36);

        let root_mrm = stage_dir
            .join("meta")
            .join("theme")
            .join(format!("{}.mrm", staged.theme_uuid));
        let root_mrc = stage_dir
            .join("content")
            .join("theme")
            .join(format!("{}.mrc", staged.theme_uuid));
        let cover = stage_dir
            .join("preview")
            .join("theme")
            .join(&staged.theme_uuid)
            .join("preview_cover_0.jpg");

        assert!(root_mrm.is_file());
        assert_eq!(fs::metadata(&root_mrc).unwrap().len(), 0);
        assert!(cover.is_file());

        let parsed: Value = serde_json::from_str(&fs::read_to_string(&root_mrm).unwrap()).unwrap();
        let subs = parsed["subResources"].as_array().unwrap();
        assert_eq!(subs.len(), 14);
        assert!(subs.iter().any(|s| s["resourceCode"] == "statusbar"));
        assert!(subs.iter().any(|s| s["resourceCode"] == "icons"));
    }
}
