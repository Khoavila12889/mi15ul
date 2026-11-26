"""CLI mode for HyperOS PC Optimizer — Bank & VNeID Safe."""

from __future__ import annotations

import argparse
import os
import sys

from adb_client import AdbClient
from mtz_packager import deploy_mtz_via_adb
from optimizer import (
    apply_notification_fix,
    check_bank_ready_status,
    get_locale_status,
    lock_developer_and_adb,
    restore_locale_backup,
    restore_millet_backup,
    restore_packages,
    scan_debloat_packages,
    scan_notification_apps,
    set_system_locale,
    uninstall_packages,
)
from presets import THEME_CATALOG


def _pick_serial(adb: AdbClient, requested: str | None) -> str:
    devs = adb.list_devices()
    ready = [d for d in devs if d.state == "device"]
    if requested:
        for d in ready:
            if d.serial == requested:
                return d.serial
        raise RuntimeError(f"Không thấy thiết bị ADB sẵn sàng với serial '{requested}'.")
    if not ready:
        raise RuntimeError("Không có thiết bị ADB nào ở trạng thái 'device'. Hãy bật USB Debugging và cắm cáp.")
    if len(ready) > 1:
        names = ", ".join(d.serial for d in ready)
        raise RuntimeError(f"Có nhiều thiết bị ({names}). Hãy truyền --serial <id>.")
    return ready[0].serial


def run_cli(argv: list[str], base_dir: str) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    themes_dir = os.path.join(base_dir, "themes")
    backups_dir = os.path.join(base_dir, "backups")
    template_path = os.path.join(themes_dir, "metadata_template.json")

    parser = argparse.ArgumentParser(
        prog="hyperos-pc-tool --cli",
        description="Công cụ ADB trên PC tối ưu HyperOS nội địa (An toàn tuyệt đối cho App Bank & VNeID).",
    )
    parser.add_argument("--serial", "-s", help="Serial thiết bị ADB (nếu cắm nhiều máy).")
    parser.add_argument("--adb", help="Đường dẫn tùy chọn tới adb.exe.")

    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("info", help="Xem thông tin máy và trạng thái bảo mật.")

    p_noti = sub.add_parser("fix-noti", help="Fix thông báo chậm (MILLET + Doze + AppOps).")
    p_noti.add_argument("--packages", help="Danh sách package cách nhau bằng dấu phẩy (mặc định: quét tự động app đã cài).")
    p_noti.add_argument("--no-ping", action="store_true", help="Không gửi FCM heartbeat 1 lần.")
    p_noti.add_argument("--restore", action="store_true", help="Khôi phục MILLET_NO_RESTRICT_APP gốc.")

    p_loc = sub.add_parser("locale", help="Đổi hoặc khôi phục system_locales.")
    p_loc.add_argument("action", choices=["status", "vi", "en", "restore", "set"], help="Hành động.")
    p_loc.add_argument("--tag", help="Mã ngôn ngữ khi action=set (vd: vi-VN,en-US).")

    p_thm = sub.add_parser("theme", help="Nạp file .mtz lên Xiaomi Themes và mở màn hình Áp dụng.")
    p_thm.add_argument("mtz", nargs="?", default=THEME_CATALOG[0].filename, help="Tên theme trong themes/ hoặc đường dẫn .mtz.")
    p_thm.add_argument("--overwrite", action="store_true", help="Ghi đè nếu theme đã tồn tại.")

    p_deb = sub.add_parser("debloat", help="Quét hoặc gỡ app rác TQ an toàn cho User 0.")
    p_deb.add_argument("action", choices=["scan", "apply-safe", "uninstall", "restore"], help="Hành động.")
    p_deb.add_argument("--packages", help="Danh sách package cách nhau bằng dấu phẩy (cho uninstall/restore).")

    sub.add_parser("bank-lock", help="Tắt Developer Options & USB Debugging (ADB) để sẵn sàng mở Bank/VNeID.")

    args = parser.parse_args(argv)
    adb = AdbClient(args.adb)
    serial = _pick_serial(adb, args.serial)

    if args.cmd == "info":
        st = check_bank_ready_status(adb, serial)
        info = st["info"]
        print(f"Serial: {serial}")
        print(f"Model: {info['brand']} {info['model']} | OS: {info['hyperos']} (Android {info['android']})")
        print(f"system_locales: {info['system_locales'] or '(trống)'}")
        print(f"MILLET_NO_RESTRICT_APP: {info['millet_count']} gói")
        print(f"adb_enabled={info['adb_enabled']} | development_settings_enabled={info['dev_enabled']}")
        return 0

    if args.cmd == "fix-noti":
        if args.restore:
            restore_millet_backup(adb, serial, backups_dir, log_cb=print)
            return 0
        if args.packages:
            pkgs = [p.strip() for p in args.packages.split(",") if p.strip()]
        else:
            rows = scan_notification_apps(adb, serial)
            pkgs = [r["package"] for r in rows if r["selected"]]
        apply_notification_fix(adb, serial, pkgs, backups_dir, send_fcm_ping=not args.no_ping, log_cb=print)
        return 0

    if args.cmd == "locale":
        if args.action == "status":
            print(get_locale_status(adb, serial, backups_dir))
        elif args.action == "vi":
            set_system_locale(adb, serial, "vi-VN", backups_dir, log_cb=print)
        elif args.action == "en":
            set_system_locale(adb, serial, "en-US", backups_dir, log_cb=print)
        elif args.action == "restore":
            restore_locale_backup(adb, serial, backups_dir, log_cb=print)
        elif args.action == "set":
            if not args.tag:
                raise ValueError("Cần truyền --tag khi dùng locale set.")
            set_system_locale(adb, serial, args.tag, backups_dir, log_cb=print)
        return 0

    if args.cmd == "theme":
        mtz_path = args.mtz if os.path.isfile(args.mtz) else os.path.join(themes_dir, args.mtz)
        ok, msg, _ = deploy_mtz_via_adb(
            adb,
            serial,
            mtz_path,
            template_path,
            overwrite=args.overwrite,
            open_apply_screen=True,
            log_cb=print,
        )
        print(msg)
        return 0 if ok else 1

    if args.cmd == "debloat":
        if args.action == "scan":
            for r in scan_debloat_packages(adb, serial):
                print(f"[{r['status']}] [{r['risk']}] {r['package']} - {r['name']}")
            return 0
        if args.action == "apply-safe":
            rows = scan_debloat_packages(adb, serial)
            pkgs = [r["package"] for r in rows if r["installed"] and r["risk"] == "An toàn"]
            uninstall_packages(adb, serial, pkgs, backups_dir, log_cb=print)
            return 0
        if not args.packages:
            raise ValueError("Cần truyền --packages cho hành động uninstall/restore.")
        pkgs = [p.strip() for p in args.packages.split(",") if p.strip()]
        if args.action == "uninstall":
            uninstall_packages(adb, serial, pkgs, backups_dir, log_cb=print)
        else:
            restore_packages(adb, serial, pkgs, backups_dir, log_cb=print)
        return 0

    if args.cmd == "bank-lock":
        lock_developer_and_adb(adb, serial, log_cb=print)
        return 0

    return 1
