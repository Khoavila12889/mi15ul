"""Catalogs and presets extracted/curated for HyperOS PC Optimizer."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class ThemePreset:
    filename: str
    title: str
    description: str


@dataclass(frozen=True)
class AppCandidate:
    package: str
    name: str
    category: str
    default_selected: bool = True


@dataclass(frozen=True)
class DebloatItem:
    package: str
    name: str
    category: str
    risk: str  # "An toàn" | "Cân nhắc"
    description: str
    default_selected: bool = True


# Matches defpackage.c50 from HyperOS_Tool.apk
THEME_CATALOG: list[ThemePreset] = [
    ThemePreset(
        filename="MTZOS4.mtz",
        title="MTZVN (Chuẩn - Máy thường)",
        description="Việt hóa đầy đủ giao diện hệ thống, kể cả ngày giờ màn hình khóa.",
    ),
    ThemePreset(
        filename="MTZOS4_kinhlong.mtz",
        title="MTZVN Kính Lỏng",
        description="Giữ hiệu ứng kính lỏng; thanh trạng thái và màn hình khóa giữ chữ gốc (không Việt hóa SystemUI).",
    ),
    ThemePreset(
        filename="MTZOS4_leica_vibrant.mtz",
        title="Xiaomi 17 Ultra Leica Vibrant VH",
        description="Việt hóa đầy đủ kèm bộ icon và hình nền phong cách Xiaomi Leica Vibrant.",
    ),
    ThemePreset(
        filename="MTZOS4_fold.mtz",
        title="MTZVN Máy Gập (Fold)",
        description="Việt hóa đầy đủ như MTZVN, tối ưu riêng cho dòng máy gập Xiaomi Fold.",
    ),
    ThemePreset(
        filename="MTZOS4_fold_kinhlong.mtz",
        title="MTZVN Máy Gập Kính Lỏng",
        description="Giữ hiệu ứng kính lỏng cho máy gập; thanh trạng thái và màn hình khóa giữ chữ gốc.",
    ),
]


# Notification whitelist candidates (Google FCM + VNeID + Chat + Vietnamese Banks)
NOTIFICATION_APPS: list[AppCandidate] = [
    # Google & FCM core (from checkupdate / bc / GuardService)
    AppCandidate("com.google.android.gms", "Google Play Services (FCM)", "Dịch vụ Google / FCM", True),
    AppCandidate("com.google.android.gsf", "Google Services Framework", "Dịch vụ Google / FCM", True),
    AppCandidate("com.google.android.gm", "Gmail", "Dịch vụ Google / FCM", True),
    AppCandidate("com.google.android.apps.messaging", "Google Messages", "Dịch vụ Google / FCM", True),
    # Định danh & Dịch vụ công
    AppCandidate("vn.vneid", "VNeID (Định danh điện tử)", "Định danh & Công", True),
    AppCandidate("com.vss.vssid", "VssID (Bảo hiểm xã hội)", "Định danh & Công", True),
    AppCandidate("vn.gov.etax", "eTax Mobile", "Định danh & Công", True),
    # Nhắn tin / Mạng xã hội (từ defpackage.bc & RearNotifyListener)
    AppCandidate("com.zing.zalo", "Zalo", "Nhắn tin & MXH", True),
    AppCandidate("com.facebook.orca", "Messenger", "Nhắn tin & MXH", True),
    AppCandidate("com.facebook.katana", "Facebook", "Nhắn tin & MXH", True),
    AppCandidate("org.telegram.messenger", "Telegram", "Nhắn tin & MXH", True),
    AppCandidate("org.thunderdog.challegram", "Telegram X", "Nhắn tin & MXH", True),
    AppCandidate("com.whatsapp", "WhatsApp", "Nhắn tin & MXH", True),
    AppCandidate("com.viber.voip", "Viber", "Nhắn tin & MXH", True),
    AppCandidate("com.ss.android.ugc.trill", "TikTok (VN/Global)", "Nhắn tin & MXH", False),
    AppCandidate("com.zhiliaoapp.musically", "TikTok (Musically)", "Nhắn tin & MXH", False),
    # Ngân hàng & Ví điện tử Việt Nam (nhận biến động số dư / OTP nhanh)
    AppCandidate("com.VCB", "Vietcombank (VCB Digibank)", "Ngân hàng & Ví", True),
    AppCandidate("com.mbmobile", "MB Bank", "Ngân hàng & Ví", True),
    AppCandidate("vn.com.techcombank.bb.app", "Techcombank Mobile", "Ngân hàng & Ví", True),
    AppCandidate("com.vnpay.bidv", "BIDV SmartBanking", "Ngân hàng & Ví", True),
    AppCandidate("com.vietinbank.ipay", "VietinBank iPay", "Ngân hàng & Ví", True),
    AppCandidate("com.vnpay.Agribank3g", "Agribank Plus", "Ngân hàng & Ví", True),
    AppCandidate("com.tpb.mb.gprsandroid", "TPBank Mobile", "Ngân hàng & Ví", True),
    AppCandidate("com.vnpay.vpbankonline", "VPBank NEO", "Ngân hàng & Ví", True),
    AppCandidate("mobile.acb.com.vn", "ACB ONE", "Ngân hàng & Ví", True),
    AppCandidate("src.com.sacombank", "Sacombank Pay / mBanking", "Ngân hàng & Ví", True),
    AppCandidate("com.sacombank.ewallet", "Sacombank Pay (eWallet)", "Ngân hàng & Ví", True),
    AppCandidate("com.vib.myvib2", "MyVIB 2.0", "Ngân hàng & Ví", True),
    AppCandidate("com.vnpay.hdbank", "HDBank", "Ngân hàng & Ví", True),
    AppCandidate("com.vnpay.ocb", "OCB OMNI", "Ngân hàng & Ví", True),
    AppCandidate("com.shb.mbanking", "SHB Mobile", "Ngân hàng & Ví", True),
    AppCandidate("com.mservice.momotransfer", "MoMo", "Ngân hàng & Ví", True),
    AppCandidate("vn.com.vng.zalopay", "ZaloPay", "Ngân hàng & Ví", True),
    AppCandidate("com.bplus.vtpay", "Viettel Money", "Ngân hàng & Ví", True),
    # Thương mại & Gọi xe
    AppCandidate("com.shopee.vn", "Shopee VN", "Tiện ích", False),
    AppCandidate("com.grabtaxi.passenger", "Grab", "Tiện ích", False),
]


# Safe debloat list for HyperOS/MIUI China ROMs (no bootloop-critical packages)
DEBLOAT_PACKAGES: list[DebloatItem] = [
    DebloatItem(
        "com.miui.analytics",
        "MIUI Analytics",
        "Quảng cáo & Theo dõi",
        "An toàn",
        "Dịch vụ thu thập dữ liệu phân tích và nhật ký hành vi người dùng của Xiaomi.",
        True,
    ),
    DebloatItem(
        "com.miui.msa.global",
        "MSA (MIUI System Ads)",
        "Quảng cáo & Theo dõi",
        "An toàn",
        "Dịch vụ hiển thị quảng cáo hệ thống trong các ứng dụng mặc định MIUI/HyperOS.",
        True,
    ),
    DebloatItem(
        "com.miui.systemAdSolution",
        "System Ad Solution (China)",
        "Quảng cáo & Theo dõi",
        "An toàn",
        "Nền tảng phân phối quảng cáo hệ thống trên ROM nội địa Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.miui.daemon",
        "MIUI Daemon",
        "Quảng cáo & Theo dõi",
        "An toàn",
        "Tiến trình chạy nền thu thập thống kê chẩn đoán và gửi telemetry cho Xiaomi.",
        True,
    ),
    DebloatItem(
        "com.xiaomi.joyose",
        "Xiaomi Joyose",
        "Quảng cáo & Theo dõi",
        "Cân nhắc",
        "Quản lý cấu hình game/đám mây và quảng cáo; gỡ giúp giảm bóp hiệu năng/chạy ngầm, nhưng vài tính năng game boost sẽ tắt.",
        False,
    ),
    DebloatItem(
        "com.xiaomi.ab",
        "Xiaomi A/B Testing",
        "Quảng cáo & Theo dõi",
        "An toàn",
        "Dịch vụ thử nghiệm A/B cấu hình tính năng từ xa của Xiaomi.",
        True,
    ),
    DebloatItem(
        "com.miui.hybrid",
        "Quick Apps (Ứng dụng nhanh)",
        "App rác Trung Quốc",
        "An toàn",
        "Nền tảng chạy mini-app không cần cài đặt, hay tự bật quảng cáo và thông báo rác.",
        True,
    ),
    DebloatItem(
        "com.miui.hybrid.accessory",
        "Quick Apps Accessory",
        "App rác Trung Quốc",
        "An toàn",
        "Thành phần phụ trợ của nền tảng Quick Apps (ứng dụng nhanh).",
        True,
    ),
    DebloatItem(
        "com.xiaomi.mipicks",
        "GetApps (Kho ứng dụng Xiaomi)",
        "App rác Trung Quốc",
        "Cân nhắc",
        "Kho ứng dụng nội địa Trung Quốc; nên giữ nếu dùng để cập nhật app hệ thống hoặc gỡ nếu chỉ dùng CH Play.",
        False,
    ),
    DebloatItem(
        "com.miui.yellowpage",
        "Trang Vàng (Yellow Pages)",
        "App rác Trung Quốc",
        "An toàn",
        "Danh bạ số tổng đài và dịch vụ nội địa Trung Quốc tích hợp trong Điện thoại.",
        True,
    ),
    DebloatItem(
        "com.xiaomi.payment",
        "Mi Coin / Thanh toán Xiaomi",
        "Ví & Mua sắm TQ",
        "An toàn",
        "Cổng thanh toán Mi Coin nội địa Trung Quốc, không dùng tại Việt Nam.",
        True,
    ),
    DebloatItem(
        "com.mipay.wallet",
        "Mi Pay / Xiaomi Wallet",
        "Ví & Mua sắm TQ",
        "An toàn",
        "Ví điện tử và thẻ giao thông nội địa Trung Quốc (không dùng được ở VN).",
        True,
    ),
    DebloatItem(
        "com.xiaomi.youpin",
        "Xiaomi Youpin",
        "Ví & Mua sắm TQ",
        "An toàn",
        "Ứng dụng mua sắm thương mại điện tử Xiaomi Youpin (tiếng Trung).",
        True,
    ),
    DebloatItem(
        "com.xiaomi.shop",
        "Mi Store (China)",
        "Ví & Mua sắm TQ",
        "An toàn",
        "Ứng dụng cửa hàng Xiaomi nội địa Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.xiaomi.gamecenter",
        "Mi Game Center",
        "Giải trí & Tin tức TQ",
        "An toàn",
        "Trung tâm tải game và thông báo khuyến mãi game nội địa Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.xiaomi.migameservice",
        "Mi Game Service",
        "Giải trí & Tin tức TQ",
        "An toàn",
        "Dịch vụ tài khoản/SDK phụ trợ cho các game nội địa trên Mi Game Center.",
        True,
    ),
    DebloatItem(
        "com.miui.video",
        "Mi Video (China)",
        "Giải trí & Tin tức TQ",
        "Cân nhắc",
        "Trình phát video mặc định kèm tab phim/quảng cáo Trung Quốc (nếu gỡ hãy dùng VLC/MX Player hoặc Thư viện).",
        False,
    ),
    DebloatItem(
        "com.miui.player",
        "Mi Music (China)",
        "Giải trí & Tin tức TQ",
        "Cân nhắc",
        "Trình nghe nhạc mặc định kèm tab nhạc trực tuyến Trung Quốc.",
        False,
    ),
    DebloatItem(
        "com.miui.newmidrive",
        "Xiaomi Cloud Drive (CN)",
        "Giải trí & Tin tức TQ",
        "An toàn",
        "Ứng dụng lưu trữ đám mây nội địa Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.miui.miservice",
        "Dịch vụ & Phản hồi (Mi Service)",
        "Hệ thống phụ trợ",
        "An toàn",
        "Ứng dụng gửi phản hồi/báo lỗi và hỗ trợ bảo hành nội địa Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.miui.bugreport",
        "Báo cáo lỗi MIUI (Bug Report)",
        "Hệ thống phụ trợ",
        "An toàn",
        "Công cụ thu thập log và gửi báo cáo lỗi về máy chủ Xiaomi.",
        True,
    ),
    DebloatItem(
        "com.xiaomi.vipaccount",
        "Xiaomi Community / VIP",
        "Hệ thống phụ trợ",
        "An toàn",
        "Ứng dụng cộng đồng Xiaomi và tích điểm thành viên nội địa.",
        True,
    ),
    DebloatItem(
        "com.miui.voiceassist",
        "XiaoAI (Trợ lý ảo tiếng Trung)",
        "Trợ lý & AI Trung Quốc",
        "An toàn",
        "Trợ lý giọng nói XiaoAI chỉ hỗ trợ tiếng Trung.",
        True,
    ),
    DebloatItem(
        "com.miui.personalassistant",
        "App Vault (Màn hình trừ 1 CN)",
        "Trợ lý & AI Trung Quốc",
        "Cân nhắc",
        "Bảng tin nhanh bên trái màn hình chính (chứa thẻ dịch vụ và tin tiếng Trung).",
        False,
    ),
    DebloatItem(
        "com.miui.contentextension",
        "Chạm để nhận diện (Taplus CN)",
        "Trợ lý & AI Trung Quốc",
        "An toàn",
        "Tính năng nhấn giữ hai ngón tách chữ/ảnh phục vụ dịch vụ nội địa Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.miui.translation.kingsoft",
        "Từ điển Kingsoft",
        "Trợ lý & AI Trung Quốc",
        "An toàn",
        "Module dịch tích hợp của Kingsoft dành cho ROM Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.miui.translation.youdao",
        "Từ điển Youdao",
        "Trợ lý & AI Trung Quốc",
        "An toàn",
        "Module dịch tích hợp của Youdao dành cho ROM Trung Quốc.",
        True,
    ),
    DebloatItem(
        "com.miui.carlink",
        "CarWith (Kết nối xe nội địa)",
        "Hệ thống phụ trợ",
        "Cân nhắc",
        "Tính năng kết nối màn hình ô tô nội địa Trung Quốc (Baidu CarLife/ICCOA).",
        False,
    ),
    DebloatItem(
        "com.xiaomi.aiasst.service",
        "AI Call Assistant (XiaoAI Call)",
        "Trợ lý & AI Trung Quốc",
        "An toàn",
        "Trợ lý AI trả lời cuộc gọi tự động bằng tiếng Trung.",
        True,
    ),
]
