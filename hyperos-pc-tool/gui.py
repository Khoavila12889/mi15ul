"""Tkinter GUI for HyperOS PC Optimizer — Bank & VNeID Safe."""

from __future__ import annotations

import os
import threading
import tkinter as tk
from datetime import datetime
from tkinter import filedialog, messagebox, scrolledtext, ttk
from typing import Callable, Optional

from adb_client import AdbClient, DeviceEntry
from mtz_packager import deploy_mtz_via_adb
from optimizer import (
    apply_notification_fix,
    check_bank_ready_status,
    get_locale_status,
    lock_developer_and_adb,
    parse_millet_list,
    restore_locale_backup,
    restore_millet_backup,
    restore_packages,
    scan_debloat_packages,
    scan_notification_apps,
    set_system_locale,
    uninstall_packages,
)
from presets import DEBLOAT_PACKAGES, NOTIFICATION_APPS, THEME_CATALOG

APP_TITLE = "HyperOS PC Optimizer — Tối ưu ROM Nội địa (An toàn Bank & VNeID)"


class ScrollableFrame(ttk.Frame):
    """Vertical scrollable container for long checkbox lists."""

    def __init__(self, parent: tk.Widget, height: int = 300) -> None:
        super().__init__(parent)
        self.canvas = tk.Canvas(self, highlightthickness=0, background="#ffffff", height=height)
        self.scrollbar = ttk.Scrollbar(self, orient="vertical", command=self.canvas.yview)
        self.inner = ttk.Frame(self.canvas)

        self.inner.bind(
            "<Configure>",
            lambda _e: self.canvas.configure(scrollregion=self.canvas.bbox("all")),
        )
        self._win_id = self.canvas.create_window((0, 0), window=self.inner, anchor="nw")
        self.canvas.bind(
            "<Configure>",
            lambda e: self.canvas.itemconfigure(self._win_id, width=e.width),
        )
        self.canvas.configure(yscrollcommand=self.scrollbar.set)

        self.canvas.pack(side="left", fill="both", expand=True)
        self.scrollbar.pack(side="right", fill="y")

        self.canvas.bind("<Enter>", self._bind_wheel)
        self.canvas.bind("<Leave>", self._unbind_wheel)

    def _on_wheel(self, event: tk.Event) -> None:
        if event.delta:
            self.canvas.yview_scroll(int(-1 * (event.delta / 120)), "units")

    def _bind_wheel(self, _e: tk.Event) -> None:
        self.canvas.bind_all("<MouseWheel>", self._on_wheel)

    def _unbind_wheel(self, _e: tk.Event) -> None:
        self.canvas.unbind_all("<MouseWheel>")


class HyperOSOptimizerApp:
    """Main desktop GUI controller."""

    def __init__(self, root: tk.Tk, base_dir: str) -> None:
        self.root = root
        self.base_dir = base_dir
        self.themes_dir = os.path.join(base_dir, "themes")
        self.backups_dir = os.path.join(base_dir, "backups")
        self.template_path = os.path.join(self.themes_dir, "metadata_template.json")

        self.root.title(APP_TITLE)
        self.root.geometry("1040x760")
        self.root.minsize(920, 660)

        self.adb: Optional[AdbClient] = None
        self.devices: list[DeviceEntry] = []
        self.busy = False
        self.action_buttons: list[ttk.Button] = []

        self.noti_vars: dict[str, tk.BooleanVar] = {}
        self.noti_rows: list[dict] = []

        self.debloat_vars: dict[str, tk.BooleanVar] = {}
        self.debloat_rows: list[dict] = []

        self._configure_styles()
        self._build_ui()
        self._init_adb_and_refresh()

    def _configure_styles(self) -> None:
        style = ttk.Style(self.root)
        available = style.theme_names()
        if "vista" in available:
            style.theme_use("vista")
        elif "clam" in available:
            style.theme_use("clam")

        style.configure("Header.TLabel", font=("Segoe UI", 12, "bold"))
        style.configure("SubHeader.TLabel", font=("Segoe UI", 9), foreground="#475569")
        style.configure("Section.TLabelframe.Label", font=("Segoe UI", 10, "bold"))
        style.configure("Primary.TButton", font=("Segoe UI", 9, "bold"), padding=(10, 5))
        style.configure("Danger.TButton", font=("Segoe UI", 10, "bold"), padding=(12, 7))
        style.configure("BadgeSafe.TLabel", foreground="#15803d", font=("Segoe UI", 8, "bold"))
        style.configure("BadgeWarn.TLabel", foreground="#b45309", font=("Segoe UI", 8, "bold"))

    def _build_ui(self) -> None:
        main = ttk.Frame(self.root, padding=10)
        main.pack(fill="both", expand=True)

        # --- Top Device Bar ---
        top_frame = ttk.LabelFrame(main, text="Kết nối thiết bị (Chỉ chạy qua cáp ADB từ PC — Không cài app lên máy)", style="Section.TLabelframe", padding=8)
        top_frame.pack(fill="x", pady=(0, 8))

        row1 = ttk.Frame(top_frame)
        row1.pack(fill="x")

        ttk.Label(row1, text="Thiết bị:", font=("Segoe UI", 9, "bold")).pack(side="left", padx=(0, 6))
        self.device_var = tk.StringVar()
        self.device_combo = ttk.Combobox(row1, textvariable=self.device_var, state="readonly", width=46)
        self.device_combo.pack(side="left", padx=(0, 8))
        self.device_combo.bind("<<ComboboxSelected>>", lambda _e: self._refresh_device_info())

        btn_refresh = ttk.Button(row1, text="Làm mới thiết bị", command=self._refresh_devices)
        btn_refresh.pack(side="left", padx=(0, 6))
        self.action_buttons.append(btn_refresh)

        btn_pick_adb = ttk.Button(row1, text="Chọn adb.exe...", command=self._browse_adb)
        btn_pick_adb.pack(side="left", padx=(0, 10))
        self.action_buttons.append(btn_pick_adb)

        self.adb_path_var = tk.StringVar(value="ADB: Đang tìm...")
        ttk.Label(row1, textvariable=self.adb_path_var, style="SubHeader.TLabel").pack(side="left", fill="x", expand=True)

        self.device_summary_var = tk.StringVar(value="Chưa kết nối thiết bị. Hãy cắm cáp USB, bật USB Debugging trên điện thoại và bấm 'Làm mới thiết bị'.")
        ttk.Label(top_frame, textvariable=self.device_summary_var, foreground="#0f172a", font=("Segoe UI", 9)).pack(anchor="w", pady=(6, 0))

        # --- Tabs ---
        self.notebook = ttk.Notebook(main)
        self.notebook.pack(fill="both", expand=True, pady=(0, 8))

        self._build_tab_notifications()
        self._build_tab_vietnamese()
        self._build_tab_debloat()
        self._build_tab_bank_lock()

        # --- Bottom Log Console ---
        log_frame = ttk.LabelFrame(main, text="Nhật ký thao tác", style="Section.TLabelframe", padding=6)
        log_frame.pack(fill="both", expand=False)

        log_bar = ttk.Frame(log_frame)
        log_bar.pack(fill="x", pady=(0, 4))
        ttk.Button(log_bar, text="Xóa nhật ký", command=self._clear_log).pack(side="right", padx=(4, 0))
        ttk.Button(log_bar, text="Sao chép nhật ký", command=self._copy_log).pack(side="right")
        self.status_var = tk.StringVar(value="Sẵn sàng.")
        ttk.Label(log_bar, textvariable=self.status_var, style="SubHeader.TLabel").pack(side="left")

        self.log_text = scrolledtext.ScrolledText(
            log_frame,
            height=8,
            font=("Consolas", 9),
            state="disabled",
            background="#0f172a",
            foreground="#e2e8f0",
            wrap="word",
        )
        self.log_text.pack(fill="both", expand=True)

    # ------------------------------------------------------------------
    # Tab 1: Fix Thong bao
    # ------------------------------------------------------------------
    def _build_tab_notifications(self) -> None:
        tab = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(tab, text="1. Fix Thông báo (FCM)")

        info = ttk.Label(
            tab,
            text=(
                "Cơ chế sạch 100%: Ghi danh sách app vào MILLET_NO_RESTRICT_APP, đưa vào Doze whitelist "
                "(deviceidle) và cấp RUN_ANY_IN_BACKGROUND qua ADB — không cài dịch vụ ngầm."
            ),
            style="SubHeader.TLabel",
            wraplength=960,
        )
        info.pack(anchor="w", pady=(0, 8))

        toolbar = ttk.Frame(tab)
        toolbar.pack(fill="x", pady=(0, 6))

        b_scan = ttk.Button(toolbar, text="Quét app trên máy", style="Primary.TButton", command=self._on_scan_noti)
        b_scan.pack(side="left", padx=(0, 6))
        self.action_buttons.append(b_scan)

        b_all = ttk.Button(toolbar, text="Chọn app đã cài", command=self._select_installed_noti)
        b_all.pack(side="left", padx=(0, 6))

        b_none = ttk.Button(toolbar, text="Bỏ chọn hết", command=lambda: self._set_all_vars(self.noti_vars, False))
        b_none.pack(side="left", padx=(0, 12))

        self.fcm_ping_var = tk.BooleanVar(value=True)
        ttk.Checkbutton(
            toolbar,
            text="Gửi nhịp tim đánh thức FCM 1 lần (GTALK/MCS_HEARTBEAT)",
            variable=self.fcm_ping_var,
        ).pack(side="left")

        self.noti_scroll = ScrollableFrame(tab, height=230)
        self.noti_scroll.pack(fill="both", expand=True, pady=(0, 8))
        self._render_noti_rows(
            [
                {
                    "package": a.package,
                    "name": a.name,
                    "category": a.category,
                    "installed": False,
                    "in_millet": False,
                    "selected": a.default_selected,
                }
                for a in NOTIFICATION_APPS
            ],
            scanned=False,
        )

        custom_row = ttk.Frame(tab)
        custom_row.pack(fill="x", pady=(0, 8))
        ttk.Label(custom_row, text="Thêm package khác (ngăn cách dấu phẩy):").pack(side="left", padx=(0, 6))
        self.custom_noti_var = tk.StringVar()
        ttk.Entry(custom_row, textvariable=self.custom_noti_var, width=52).pack(side="left", fill="x", expand=True)

        bottom = ttk.Frame(tab)
        bottom.pack(fill="x")
        b_apply = ttk.Button(bottom, text="Áp dụng Fix Thông báo", style="Primary.TButton", command=self._on_apply_noti)
        b_apply.pack(side="left", padx=(0, 8))
        self.action_buttons.append(b_apply)

        b_view = ttk.Button(bottom, text="Xem MILLET_NO_RESTRICT_APP hiện tại", command=self._on_view_millet)
        b_view.pack(side="left", padx=(0, 8))
        self.action_buttons.append(b_view)

        b_rest = ttk.Button(bottom, text="Khôi phục MILLET gốc", command=self._on_restore_millet)
        b_rest.pack(side="left")
        self.action_buttons.append(b_rest)

    def _render_noti_rows(self, rows: list[dict], scanned: bool = True) -> None:
        for w in self.noti_scroll.inner.winfo_children():
            w.destroy()
        self.noti_rows = rows
        self.noti_vars.clear()

        last_cat = None
        for idx, row in enumerate(rows):
            cat = row["category"]
            if cat != last_cat:
                lbl = ttk.Label(self.noti_scroll.inner, text=f"▸ {cat}", font=("Segoe UI", 9, "bold"), foreground="#1e3a8a")
                lbl.pack(anchor="w", padx=6, pady=(8 if last_cat else 2, 2))
                last_cat = cat

            var = tk.BooleanVar(value=bool(row["selected"]))
            self.noti_vars[row["package"]] = var

            item_f = ttk.Frame(self.noti_scroll.inner)
            item_f.pack(fill="x", padx=12, pady=1)

            cb = ttk.Checkbutton(item_f, text=f"{row['name']}  ({row['package']})", variable=var)
            cb.pack(side="left")

            if scanned:
                status_parts = []
                if row["installed"]:
                    status_parts.append("Đã cài")
                else:
                    status_parts.append("Chưa cài")
                if row["in_millet"]:
                    status_parts.append("Đã có trong MILLET")
                badge_text = " | ".join(status_parts)
                color = "#15803d" if row["installed"] else "#94a3b8"
                ttk.Label(item_f, text=badge_text, foreground=color, font=("Segoe UI", 8)).pack(side="right", padx=6)

    # ------------------------------------------------------------------
    # Tab 2: Viet hoa nhanh (Locale + Theme MTZ)
    # ------------------------------------------------------------------
    def _build_tab_vietnamese(self) -> None:
        tab = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(tab, text="2. Việt hóa (Locale & Theme MTZ)")

        # Section A: Locale
        loc_box = ttk.LabelFrame(tab, text="A. Ngôn ngữ hệ thống / App (system_locales)", style="Section.TLabelframe", padding=10)
        loc_box.pack(fill="x", pady=(0, 10))

        self.locale_status_var = tk.StringVar(value="Ngôn ngữ hiện tại: (chưa đọc) | Bản sao lưu: (chưa có)")
        ttk.Label(loc_box, textvariable=self.locale_status_var, font=("Segoe UI", 9, "bold")).pack(anchor="w", pady=(0, 8))

        loc_btns = ttk.Frame(loc_box)
        loc_btns.pack(fill="x")

        for text, tag, primary in [
            ("Sao lưu & Bật Tiếng Việt (vi-VN)", "vi-VN", True),
            ("Bật vi-VN,en-US", "vi-VN,en-US", False),
            ("Đưa về Tiếng Anh (en-US)", "en-US", False),
        ]:
            b = ttk.Button(
                loc_btns,
                text=text,
                style="Primary.TButton" if primary else "TButton",
                command=lambda t=tag: self._on_set_locale(t),
            )
            b.pack(side="left", padx=(0, 8))
            self.action_buttons.append(b)

        b_loc_restore = ttk.Button(loc_btns, text="Khôi phục ngôn ngữ gốc đã sao lưu", command=self._on_restore_locale)
        b_loc_restore.pack(side="left", padx=(0, 8))
        self.action_buttons.append(b_loc_restore)

        b_loc_check = ttk.Button(loc_btns, text="Đọc lại", command=self._on_check_locale)
        b_loc_check.pack(side="left")
        self.action_buttons.append(b_loc_check)

        ttk.Label(
            loc_box,
            text="Ghi chú: Trên HyperOS China ROM gốc, chỉnh system_locales giúp app bên thứ 3 nhận Tiếng Việt; để Việt hóa sâu giao diện hệ thống, hãy nạp thêm Theme MTZ bên dưới.",
            style="SubHeader.TLabel",
            wraplength=940,
        ).pack(anchor="w", pady=(6, 0))

        # Section B: MTZ Theme
        mtz_box = ttk.LabelFrame(tab, text="B. Nạp Theme Việt hóa (.mtz) từ PC (Tạo .mrm/.mrc chuẩn & Mở trang Áp dụng)", style="Section.TLabelframe", padding=10)
        mtz_box.pack(fill="both", expand=True)

        self.theme_choice_var = tk.StringVar(value=THEME_CATALOG[0].filename)
        for preset in THEME_CATALOG:
            r_frame = ttk.Frame(mtz_box)
            r_frame.pack(fill="x", pady=2)
            ttk.Radiobutton(
                r_frame,
                text=f"{preset.title}  [{preset.filename}]",
                value=preset.filename,
                variable=self.theme_choice_var,
            ).pack(side="left")
            ttk.Label(r_frame, text=f"— {preset.description}", style="SubHeader.TLabel").pack(side="left", padx=(8, 0))

        custom_mtz_row = ttk.Frame(mtz_box)
        custom_mtz_row.pack(fill="x", pady=(8, 6))
        self.custom_mtz_var = tk.StringVar()
        ttk.Radiobutton(
            custom_mtz_row,
            text="Hoặc chọn file .mtz riêng trên PC:",
            value="__CUSTOM__",
            variable=self.theme_choice_var,
        ).pack(side="left", padx=(0, 6))
        ttk.Entry(custom_mtz_row, textvariable=self.custom_mtz_var, width=50).pack(side="left", fill="x", expand=True, padx=(0, 6))
        b_browse_mtz = ttk.Button(custom_mtz_row, text="Chọn file .mtz...", command=self._browse_mtz)
        b_browse_mtz.pack(side="left")
        self.action_buttons.append(b_browse_mtz)

        mtz_act_row = ttk.Frame(mtz_box)
        mtz_act_row.pack(fill="x", pady=(6, 0))
        self.mtz_overwrite_var = tk.BooleanVar(value=False)
        ttk.Checkbutton(mtz_act_row, text="Ghi đè nếu Theme đã tồn tại trong Chủ đề", variable=self.mtz_overwrite_var).pack(side="left", padx=(0, 12))

        b_push_mtz = ttk.Button(
            mtz_act_row,
            text="Nạp Theme lên máy & Mở màn hình Áp dụng",
            style="Primary.TButton",
            command=self._on_deploy_mtz,
        )
        b_push_mtz.pack(side="left")
        self.action_buttons.append(b_push_mtz)

        ttk.Label(
            mtz_box,
            text="Sau khi nạp xong, tool tự mở trang chi tiết Theme trong Xiaomi Themes — bạn chỉ cần bấm [Áp dụng] trực tiếp trên màn hình điện thoại (không dùng auto-click ngầm).",
            style="SubHeader.TLabel",
            wraplength=940,
        ).pack(anchor="w", pady=(8, 0))

    # ------------------------------------------------------------------
    # Tab 3: Safe Debloat
    # ------------------------------------------------------------------
    def _build_tab_debloat(self) -> None:
        tab = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(tab, text="3. Gỡ App rác TQ (Safe Debloat)")

        ttk.Label(
            tab,
            text=(
                "Chỉ gỡ cho User 0 (pm uninstall -k --user 0) các gói quảng cáo/telemetry/dịch vụ nội địa TQ; "
                "không động vào gói lõi (SecurityCenter, FindDevice, ThemeManager, Xiaomi Account) để tránh bootloop."
            ),
            style="SubHeader.TLabel",
            wraplength=960,
        ).pack(anchor="w", pady=(0, 8))

        bar = ttk.Frame(tab)
        bar.pack(fill="x", pady=(0, 6))

        b_scan = ttk.Button(bar, text="Quét App rác trên máy", style="Primary.TButton", command=self._on_scan_debloat)
        b_scan.pack(side="left", padx=(0, 6))
        self.action_buttons.append(b_scan)

        ttk.Button(bar, text="Chọn gói An toàn đang cài", command=self._select_safe_debloat).pack(side="left", padx=(0, 6))
        ttk.Button(bar, text="Bỏ chọn hết", command=lambda: self._set_all_vars(self.debloat_vars, False)).pack(side="left", padx=(0, 14))

        ttk.Label(bar, text="Lọc:").pack(side="left", padx=(0, 4))
        self.debloat_filter_var = tk.StringVar(value="Tất cả")
        filter_cb = ttk.Combobox(
            bar,
            textvariable=self.debloat_filter_var,
            state="readonly",
            values=["Tất cả", "Đang cài", "Đã gỡ (User 0)"],
            width=18,
        )
        filter_cb.pack(side="left")
        filter_cb.bind("<<ComboboxSelected>>", lambda _e: self._render_debloat_rows(self.debloat_rows, scanned=bool(self.debloat_rows)))

        self.debloat_scroll = ScrollableFrame(tab, height=250)
        self.debloat_scroll.pack(fill="both", expand=True, pady=(0, 8))
        self._render_debloat_rows(
            [
                {
                    "package": d.package,
                    "name": d.name,
                    "category": d.category,
                    "risk": d.risk,
                    "description": d.description,
                    "status": "Chưa quét",
                    "installed": False,
                    "restorable": False,
                    "selected": d.default_selected,
                }
                for d in DEBLOAT_PACKAGES
            ],
            scanned=False,
        )

        act = ttk.Frame(tab)
        act.pack(fill="x")
        b_uninst = ttk.Button(act, text="Gỡ các gói đã chọn (User 0)", style="Primary.TButton", command=self._on_uninstall_debloat)
        b_uninst.pack(side="left", padx=(0, 8))
        self.action_buttons.append(b_uninst)

        b_restore = ttk.Button(act, text="Khôi phục app đã gỡ (install-existing)", command=self._on_restore_debloat)
        b_restore.pack(side="left")
        self.action_buttons.append(b_restore)

    def _render_debloat_rows(self, rows: list[dict], scanned: bool = True) -> None:
        for w in self.debloat_scroll.inner.winfo_children():
            w.destroy()
        self.debloat_rows = rows
        prev_selected = {k: v.get() for k, v in self.debloat_vars.items()}
        self.debloat_vars.clear()

        flt = self.debloat_filter_var.get()
        for row in rows:
            if scanned and flt != "Tất cả" and row["status"] != flt:
                continue

            pkg = row["package"]
            default_val = prev_selected.get(pkg, bool(row["selected"]))
            var = tk.BooleanVar(value=default_val)
            self.debloat_vars[pkg] = var

            card = ttk.Frame(self.debloat_scroll.inner, padding=(6, 3))
            card.pack(fill="x", padx=4, pady=1)

            top_line = ttk.Frame(card)
            top_line.pack(fill="x")
            ttk.Checkbutton(top_line, text=f"{row['name']}  ({pkg})", variable=var).pack(side="left")

            risk_style = "BadgeSafe.TLabel" if row["risk"] == "An toàn" else "BadgeWarn.TLabel"
            ttk.Label(top_line, text=f"[{row['risk']}]", style=risk_style).pack(side="left", padx=(8, 4))

            if scanned:
                st_color = "#dc2626" if row["status"] == "Đã gỡ (User 0)" else ("#15803d" if row["installed"] else "#94a3b8")
                ttk.Label(top_line, text=row["status"], foreground=st_color, font=("Segoe UI", 8, "bold")).pack(side="right", padx=6)

            ttk.Label(card, text=f"    {row['description']}", style="SubHeader.TLabel", wraplength=880).pack(anchor="w")

    # ------------------------------------------------------------------
    # Tab 4: Bank-Ready Lock
    # ------------------------------------------------------------------
    def _build_tab_bank_lock(self) -> None:
        tab = ttk.Frame(self.notebook, padding=14)
        self.notebook.add(tab, text="4. Khóa Bảo mật (Bank-Ready)")

        box = ttk.LabelFrame(tab, text="Kiểm tra & Khóa môi trường sạch cho VNeID / App Ngân hàng", style="Section.TLabelframe", padding=14)
        box.pack(fill="both", expand=True)

        self.bank_status_var = tk.StringVar(value="Bấm 'Kiểm tra trạng thái bảo mật' để xem tình trạng ADB & Tùy chọn nhà phát triển.")
        ttk.Label(box, textvariable=self.bank_status_var, font=("Segoe UI", 10, "bold"), foreground="#0f172a").pack(anchor="w", pady=(0, 10))

        checklist_text = (
            "Cam kết môi trường sạch:\n"
            "  • Không cài đặt bất kỳ APK lạ hay công cụ hook/root nào lên điện thoại.\n"
            "  • Không để lại dịch vụ ngầm (ForegroundService / app_process) chạy trên điện thoại.\n"
            "  • Các file MTZ tạm trên /data/local/tmp/ đều được dọn sạch ngay sau khi nạp.\n\n"
            "Hành động của nút 'Hoàn tất & Khóa bảo mật':\n"
            "  1. Chạy `settings put global development_settings_enabled 0` (tắt Tùy chọn nhà phát triển).\n"
            "  2. Chạy `settings put global adb_enabled 0` (tắt Gỡ lỗi USB / ADB).\n"
            "  3. Sau khi bấm, kết nối ADB với PC sẽ tự ngắt — bạn chỉ cần rút cáp USB và mở VNeID / App Ngân hàng."
        )
        ttk.Label(box, text=checklist_text, justify="left", font=("Segoe UI", 9)).pack(anchor="w", pady=(0, 16))

        btn_row = ttk.Frame(box)
        btn_row.pack(fill="x")

        b_chk = ttk.Button(btn_row, text="Kiểm tra trạng thái bảo mật", command=self._on_check_bank_ready)
        b_chk.pack(side="left", padx=(0, 10))
        self.action_buttons.append(b_chk)

        b_lock = ttk.Button(
            btn_row,
            text="Hoàn tất & Khóa bảo mật (Tắt Developer Options + ADB)",
            style="Danger.TButton",
            command=self._on_lock_bank_ready,
        )
        b_lock.pack(side="left")
        self.action_buttons.append(b_lock)

    # ------------------------------------------------------------------
    # Helpers & Async Worker
    # ------------------------------------------------------------------
    def log(self, msg: str) -> None:
        ts = datetime.now().strftime("%H:%M:%S")
        line = f"[{ts}] {msg}\n"

        def _append() -> None:
            self.log_text.configure(state="normal")
            self.log_text.insert("end", line)
            self.log_text.see("end")
            self.log_text.configure(state="disabled")

        self.root.after(0, _append)

    def _clear_log(self) -> None:
        self.log_text.configure(state="normal")
        self.log_text.delete("1.0", "end")
        self.log_text.configure(state="disabled")

    def _copy_log(self) -> None:
        content = self.log_text.get("1.0", "end").strip()
        self.root.clipboard_clear()
        self.root.clipboard_append(content)
        self.status_var.set("Đã sao chép nhật ký vào clipboard.")

    def _set_busy(self, busy: bool, status: str = "") -> None:
        def _apply() -> None:
            self.busy = busy
            state = "disabled" if busy else "normal"
            for btn in self.action_buttons:
                btn.configure(state=state)
            if status:
                self.status_var.set(status)

        self.root.after(0, _apply)

    def _set_all_vars(self, mapping: dict[str, tk.BooleanVar], val: bool) -> None:
        for v in mapping.values():
            v.set(val)

    def _select_installed_noti(self) -> None:
        for row in self.noti_rows:
            if row["package"] in self.noti_vars:
                self.noti_vars[row["package"]].set(bool(row.get("installed")))

    def _select_safe_debloat(self) -> None:
        for row in self.debloat_rows:
            pkg = row["package"]
            if pkg in self.debloat_vars:
                self.debloat_vars[pkg].set(bool(row.get("installed")) and row.get("risk") == "An toàn")

    def _selected_serial(self) -> Optional[str]:
        idx = self.device_combo.current()
        if idx < 0 or idx >= len(self.devices):
            return None
        dev = self.devices[idx]
        if dev.state != "device":
            messagebox.showwarning(
                "Thiết bị chưa sẵn sàng",
                f"Thiết bị {dev.serial} đang ở trạng thái '{dev.state}'. Hãy kiểm tra lại pop-up 'Allow USB debugging' trên điện thoại.",
            )
            return None
        return dev.serial

    def _run_async(self, status_label: str, worker: Callable[[], None]) -> None:
        if self.busy:
            return
        self._set_busy(True, status_label)

        def _runner() -> None:
            try:
                worker()
            except Exception as exc:
                self.log(f"LỖI: {exc}")
                self.root.after(0, lambda e=exc: messagebox.showerror("Lỗi thực thi", str(e)))
            finally:
                self._set_busy(False, "Sẵn sàng.")

        threading.Thread(target=_runner, daemon=True).start()

    # ------------------------------------------------------------------
    # ADB & Action Handlers
    # ------------------------------------------------------------------
    def _init_adb_and_refresh(self) -> None:
        try:
            self.adb = AdbClient()
            self.adb_path_var.set(f"ADB: {self.adb.adb_path}")
            self.log(f"Đã nhận ADB: {self.adb.version()}")
            self._refresh_devices()
        except Exception as exc:
            self.adb_path_var.set("ADB: Không tìm thấy")
            self.log(f"Cảnh báo ADB: {exc}")

    def _browse_adb(self) -> None:
        picked = filedialog.askopenfilename(
            title="Chọn file adb.exe",
            filetypes=[("ADB executable", "adb.exe"), ("All files", "*.*")],
        )
        if not picked:
            return
        try:
            self.adb = AdbClient(picked)
            self.adb_path_var.set(f"ADB: {self.adb.adb_path}")
            self.log(f"Đã chuyển sang ADB: {self.adb.version()}")
            self._refresh_devices()
        except Exception as exc:
            messagebox.showerror("ADB không hợp lệ", str(exc))

    def _browse_mtz(self) -> None:
        picked = filedialog.askopenfilename(
            title="Chọn file Theme .mtz",
            initialdir=self.themes_dir,
            filetypes=[("Xiaomi Theme", "*.mtz"), ("All files", "*.*")],
        )
        if picked:
            self.custom_mtz_var.set(picked)
            self.theme_choice_var.set("__CUSTOM__")

    def _refresh_devices(self) -> None:
        if not self.adb:
            messagebox.showwarning("Thiếu ADB", "Chưa tìm thấy adb.exe hợp lệ.")
            return

        def _work() -> None:
            assert self.adb is not None
            devs = self.adb.list_devices()
            self.devices = devs

            def _update_ui() -> None:
                labels = [d.display_label for d in devs]
                self.device_combo.configure(values=labels)
                if labels:
                    self.device_combo.current(0)
                    self._refresh_device_info()
                else:
                    self.device_var.set("")
                    self.device_summary_var.set(
                        "Không thấy thiết bị nào. Hãy bật Tùy chọn nhà phát triển -> Gỡ lỗi USB trên điện thoại rồi cắm cáp lại."
                    )

            self.root.after(0, _update_ui)

        self._run_async("Đang quét thiết bị ADB...", _work)

    def _refresh_device_info(self) -> None:
        serial = self._selected_serial()
        if not serial or not self.adb:
            return

        def _work() -> None:
            assert self.adb is not None
            info = self.adb.get_device_info(serial)
            loc_st = get_locale_status(self.adb, serial, self.backups_dir)
            summary = (
                f"Máy: {info['brand']} {info['model']} | "
                f"OS: {info['hyperos'] or 'Android'} (Android {info['android']} / SDK {info['sdk']}) | "
                f"Locale: {info['system_locales'] or 'mặc định'} | "
                f"MILLET: {info['millet_count']} app | "
                f"ADB={info['adb_enabled'] or '?'} | Dev={info['dev_enabled'] or '?'}"
            )
            loc_txt = f"Ngôn ngữ hiện tại: {loc_st['current']}   |   Bản sao lưu gốc: {loc_st['backup']}"
            bank_txt = (
                f"Trạng thái hiện tại: ADB = {'BẬT (1)' if info['adb_enabled'] == '1' else 'TẮT (0)'}   |   "
                f"Tùy chọn nhà phát triển = {'BẬT (1)' if info['dev_enabled'] == '1' else 'TẮT (0)'}"
            )

            def _apply() -> None:
                self.device_summary_var.set(summary)
                self.locale_status_var.set(loc_txt)
                self.bank_status_var.set(bank_txt)

            self.root.after(0, _apply)

        self._run_async("Đang đọc thông tin máy...", _work)

    def _require_ready(self) -> Optional[tuple[AdbClient, str]]:
        if not self.adb:
            messagebox.showwarning("Thiếu ADB", "Chưa có adb.exe hợp lệ.")
            return None
        serial = self._selected_serial()
        if not serial:
            messagebox.showwarning("Chưa chọn thiết bị", "Vui lòng kết nối điện thoại và bấm 'Làm mới thiết bị'.")
            return None
        return self.adb, serial

    def _on_scan_noti(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            rows = scan_notification_apps(adb, serial)
            inst_count = sum(1 for r in rows if r["installed"])
            self.log(f"Đã quét xong: phát hiện {inst_count}/{len(rows)} app phổ biến đang cài trên máy.")
            self.root.after(0, lambda: self._render_noti_rows(rows, scanned=True))

        self._run_async("Đang quét app trên điện thoại...", _work)

    def _on_apply_noti(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx
        chosen = [pkg for pkg, var in self.noti_vars.items() if var.get()]
        custom_raw = self.custom_noti_var.get().strip()
        if custom_raw:
            chosen.extend(p.strip() for p in custom_raw.split(",") if p.strip())
        if not chosen:
            messagebox.showwarning("Chưa chọn app", "Hãy tick ít nhất 1 ứng dụng (ví dụ Google Play Services, Zalo, VNeID, Bank).")
            return
        send_ping = self.fcm_ping_var.get()

        def _work() -> None:
            res = apply_notification_fix(adb, serial, chosen, self.backups_dir, send_fcm_ping=send_ping, log_cb=self.log)
            self.root.after(
                0,
                lambda: messagebox.showinfo(
                    "Hoàn tất Fix Thông báo",
                    f"Đã xử lý {res['total_pkgs']} gói:\n"
                    f"• MILLET_NO_RESTRICT_APP: {'OK' if res['millet_ok'] else 'Không ghi được'} (tổng {res['millet_total']} gói)\n"
                    f"• Doze whitelist: {res['doze_ok']}/{res['total_pkgs']}\n"
                    f"• AppOps background: {res['appops_ok']}/{res['total_pkgs']}",
                ),
            )
            self._refresh_device_info_inline(adb, serial)

        self._run_async("Đang áp dụng Fix Thông báo...", _work)

    def _on_view_millet(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            raw = adb.get_setting("system", "MILLET_NO_RESTRICT_APP", serial)
            items = parse_millet_list(raw)
            self.log(f"MILLET_NO_RESTRICT_APP ({len(items)} gói): {', '.join(items) if items else '(trống)'}")

        self._run_async("Đang đọc MILLET_NO_RESTRICT_APP...", _work)

    def _on_restore_millet(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            ok = restore_millet_backup(adb, serial, self.backups_dir, log_cb=self.log)
            if ok:
                self._refresh_device_info_inline(adb, serial)

        self._run_async("Đang khôi phục MILLET...", _work)

    def _on_check_locale(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            st = get_locale_status(adb, serial, self.backups_dir)
            self.log(f"Ngôn ngữ hiện tại: {st['current']} | Sao lưu: {st['backup']}")
            self.root.after(0, lambda: self.locale_status_var.set(f"Ngôn ngữ hiện tại: {st['current']}   |   Bản sao lưu gốc: {st['backup']}"))

        self._run_async("Đang đọc system_locales...", _work)

    def _on_set_locale(self, locale_tag: str) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            ok = set_system_locale(adb, serial, locale_tag, self.backups_dir, log_cb=self.log)
            st = get_locale_status(adb, serial, self.backups_dir)
            self.root.after(0, lambda: self.locale_status_var.set(f"Ngôn ngữ hiện tại: {st['current']}   |   Bản sao lưu gốc: {st['backup']}"))
            self._refresh_device_info_inline(adb, serial)
            if ok:
                self.root.after(
                    0,
                    lambda: messagebox.showinfo(
                        "Đã cập nhật ngôn ngữ",
                        f"Đã đặt system_locales = '{locale_tag}'.\nKhởi động lại điện thoại hoặc nạp thêm Theme Việt hóa để áp dụng toàn diện.",
                    ),
                )

        self._run_async(f"Đang đặt system_locales = {locale_tag}...", _work)

    def _on_restore_locale(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            restore_locale_backup(adb, serial, self.backups_dir, log_cb=self.log)
            st = get_locale_status(adb, serial, self.backups_dir)
            self.root.after(0, lambda: self.locale_status_var.set(f"Ngôn ngữ hiện tại: {st['current']}   |   Bản sao lưu gốc: {st['backup']}"))
            self._refresh_device_info_inline(adb, serial)

        self._run_async("Đang khôi phục ngôn ngữ gốc...", _work)

    def _on_deploy_mtz(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx
        choice = self.theme_choice_var.get()
        if choice == "__CUSTOM__":
            mtz_path = self.custom_mtz_var.get().strip()
        else:
            mtz_path = os.path.join(self.themes_dir, choice)
        if not mtz_path or not os.path.isfile(mtz_path):
            messagebox.showwarning("Thiếu file MTZ", f"Không tìm thấy file theme: {mtz_path}")
            return
        overwrite = self.mtz_overwrite_var.get()

        def _work() -> None:
            ok, msg, staged = deploy_mtz_via_adb(
                adb,
                serial,
                mtz_path,
                self.template_path,
                overwrite=overwrite,
                open_apply_screen=True,
                log_cb=self.log,
            )
            if ok:
                self.root.after(
                    0,
                    lambda: messagebox.showinfo(
                        "Đã nạp Theme",
                        f"{msg}\nUUID: {staged.theme_uuid}\n\nHãy nhìn màn hình điện thoại và bấm nút [Áp dụng] (Apply) trong ứng dụng Chủ đề.",
                    ),
                )

        self._run_async("Đang đóng gói & đẩy Theme MTZ...", _work)

    def _on_scan_debloat(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            rows = scan_debloat_packages(adb, serial)
            inst = sum(1 for r in rows if r["installed"])
            rem = sum(1 for r in rows if r["status"] == "Đã gỡ (User 0)")
            self.log(f"Quét Debloat xong: {inst} gói đang cài, {rem} gói đã gỡ trước đó.")
            self.root.after(0, lambda: self._render_debloat_rows(rows, scanned=True))

        self._run_async("Đang quét app rác trên máy...", _work)

    def _on_uninstall_debloat(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx
        chosen = [pkg for pkg, var in self.debloat_vars.items() if var.get()]
        if not chosen:
            messagebox.showwarning("Chưa chọn gói", "Hãy tick chọn các gói app rác cần gỡ.")
            return
        if not messagebox.askyesno(
            "Xác nhận gỡ App rác (User 0)",
            f"Bạn sắp gỡ {len(chosen)} gói cho User 0 (có thể khôi phục lại bất cứ lúc nào).\nTiếp tục?",
        ):
            return

        def _work() -> None:
            ok_cnt, fail_cnt = uninstall_packages(adb, serial, chosen, self.backups_dir, log_cb=self.log)
            rows = scan_debloat_packages(adb, serial)
            self.root.after(0, lambda: self._render_debloat_rows(rows, scanned=True))
            self.root.after(
                0,
                lambda: messagebox.showinfo("Kết quả Debloat", f"Đã gỡ thành công: {ok_cnt} gói\nKhông gỡ được / Không có sẵn: {fail_cnt} gói"),
            )

        self._run_async(f"Đang gỡ {len(chosen)} gói...", _work)

    def _on_restore_debloat(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx
        chosen = [pkg for pkg, var in self.debloat_vars.items() if var.get()]
        if not chosen:
            messagebox.showwarning("Chưa chọn gói", "Hãy tick chọn các gói muốn khôi phục (bộ lọc 'Đã gỡ (User 0)' giúp chọn nhanh).")
            return

        def _work() -> None:
            ok_cnt, fail_cnt = restore_packages(adb, serial, chosen, self.backups_dir, log_cb=self.log)
            rows = scan_debloat_packages(adb, serial)
            self.root.after(0, lambda: self._render_debloat_rows(rows, scanned=True))
            self.root.after(
                0,
                lambda: messagebox.showinfo("Kết quả Khôi phục", f"Đã khôi phục: {ok_cnt} gói\nThất bại: {fail_cnt} gói"),
            )

        self._run_async(f"Đang khôi phục {len(chosen)} gói...", _work)

    def _on_check_bank_ready(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx

        def _work() -> None:
            st = check_bank_ready_status(adb, serial)
            msg = (
                f"Trạng thái hiện tại: ADB = {'BẬT (1)' if st['adb_enabled'] else 'TẮT (0)'}   |   "
                f"Tùy chọn nhà phát triển = {'BẬT (1)' if st['dev_enabled'] else 'TẮT (0)'}"
            )
            self.log(msg)
            self.root.after(0, lambda: self.bank_status_var.set(msg))

        self._run_async("Đang kiểm tra trạng thái bảo mật...", _work)

    def _on_lock_bank_ready(self) -> None:
        ctx = self._require_ready()
        if not ctx:
            return
        adb, serial = ctx
        if not messagebox.askyesno(
            "Xác nhận Khóa bảo mật",
            "Thao tác này sẽ TẮT Tùy chọn nhà phát triển và TẮT USB Debugging (ADB) trên điện thoại.\n"
            "Kết nối ADB với máy tính sẽ ngắt ngay sau đó.\n\nBạn đã hoàn tất việc tối ưu và muốn khóa ngay bây giờ?",
        ):
            return

        def _work() -> None:
            lock_developer_and_adb(adb, serial, log_cb=self.log)

            def _done() -> None:
                self.bank_status_var.set("Đã gửi lệnh khóa Tùy chọn nhà phát triển & ADB (0). Hãy rút cáp USB và kiểm tra VNeID / App Ngân hàng.")
                messagebox.showinfo(
                    "Đã khóa bảo mật",
                    "Đã tắt Tùy chọn nhà phát triển và USB Debugging!\n\nHãy rút cáp USB và mở thử VNeID / App Ngân hàng trên điện thoại.",
                )

            self.root.after(0, _done)

        self._run_async("Đang khóa Tùy chọn nhà phát triển & ADB...", _work)

    def _refresh_device_info_inline(self, adb: AdbClient, serial: str) -> None:
        info = adb.get_device_info(serial)
        summary = (
            f"Máy: {info['brand']} {info['model']} | "
            f"OS: {info['hyperos'] or 'Android'} (Android {info['android']} / SDK {info['sdk']}) | "
            f"Locale: {info['system_locales'] or 'mặc định'} | "
            f"MILLET: {info['millet_count']} app | "
            f"ADB={info['adb_enabled'] or '?'} | Dev={info['dev_enabled'] or '?'}"
        )
        self.root.after(0, lambda: self.device_summary_var.set(summary))


def launch_gui(base_dir: str) -> None:
    root = tk.Tk()
    HyperOSOptimizerApp(root, base_dir)
    root.mainloop()
