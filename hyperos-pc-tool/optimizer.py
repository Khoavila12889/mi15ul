"""Optimization workflows & backup persistence for HyperOS PC Optimizer."""

from __future__ import annotations

import json
import os
import re
from datetime import datetime
from typing import Callable, Optional

from adb_client import AdbClient
from presets import DEBLOAT_PACKAGES, NOTIFICATION_APPS

PACKAGE_NAME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$")
LOCALE_TAG_RE = re.compile(r"^[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8})*(?:,[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8})*)*$")


def _safe_serial(serial: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", serial or "unknown")


def get_backup_path(backups_dir: str, serial: str) -> str:
    os.makedirs(backups_dir, exist_ok=True)
    return os.path.join(backups_dir, f"{_safe_serial(serial)}_backup.json")


def load_backup(backups_dir: str, serial: str) -> dict:
    path = get_backup_path(backups_dir, serial)
    if not os.path.isfile(path):
        return {}
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
            return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def save_backup(backups_dir: str, serial: str, data: dict) -> None:
    path = get_backup_path(backups_dir, serial)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)


# ---------------------------------------------------------------------------
# 1. Notification Fix (MILLET_NO_RESTRICT_APP + deviceidle + appops)
# ---------------------------------------------------------------------------

def parse_millet_list(raw: str) -> list[str]:
    if not raw or raw.strip() == "null":
        return []
    seen: set[str] = set()
    items: list[str] = []
    for part in raw.split(","):
        pkg = part.strip()
        if pkg and pkg not in seen:
            seen.add(pkg)
            items.append(pkg)
    return items


def scan_notification_apps(adb: AdbClient, serial: str) -> list[dict]:
    """Scan installed apps matching NOTIFICATION_APPS + existing MILLET entries."""
    installed = adb.list_packages_for_user(serial) if hasattr(adb, "list_packages_for_user") else adb.list_all_packages_for_user(serial)
    millet_raw = adb.get_setting("system", "MILLET_NO_RESTRICT_APP", serial)
    millet_set = set(parse_millet_list(millet_raw))

    rows: list[dict] = []
    for app in NOTIFICATION_APPS:
        is_inst = app.package in installed
        rows.append(
            {
                "package": app.package,
                "name": app.name,
                "category": app.category,
                "installed": is_inst,
                "in_millet": app.package in millet_set,
                "selected": is_inst and app.default_selected,
            }
        )
    return rows


def apply_notification_fix(
    adb: AdbClient,
    serial: str,
    packages: list[str],
    backups_dir: str,
    send_fcm_ping: bool = True,
    log_cb: Optional[Callable[[str], None]] = None,
) -> dict[str, object]:
    """Add packages to MILLET_NO_RESTRICT_APP, Doze whitelist, and RUN_ANY_IN_BACKGROUND."""
    def log(msg: str) -> None:
        if log_cb:
            log_cb(msg)

    clean_pkgs: list[str] = []
    for p in packages:
        pkg = p.strip()
        if not pkg:
            continue
        if not PACKAGE_NAME_RE.match(pkg):
            raise ValueError(f"Tên package không hợp lệ: {pkg}")
        if pkg not in clean_pkgs:
            clean_pkgs.append(pkg)

    if not clean_pkgs:
        raise ValueError("Chưa chọn ứng dụng nào để tối ưu thông báo.")

    # Backup original MILLET_NO_RESTRICT_APP once
    current_raw = adb.get_setting("system", "MILLET_NO_RESTRICT_APP", serial)
    backup = load_backup(backups_dir, serial)
    if "millet_original" not in backup:
        backup["millet_original"] = current_raw
        backup["millet_saved_at"] = datetime.now().isoformat(timespec="seconds")
        save_backup(backups_dir, serial, backup)
        log(f"Đã sao lưu MILLET_NO_RESTRICT_APP gốc ({len(parse_millet_list(current_raw))} mục).")

    merged = parse_millet_list(current_raw)
    added_to_millet = 0
    for pkg in clean_pkgs:
        if pkg not in merged:
            merged.append(pkg)
            added_to_millet += 1

    millet_value = ",".join(merged)
    millet_res = adb.shell(
        f"settings put system MILLET_NO_RESTRICT_APP '{millet_value}'",
        serial=serial,
        timeout=15,
    )
    verify_raw = adb.get_setting("system", "MILLET_NO_RESTRICT_APP", serial)
    millet_ok = millet_res.ok and all(pkg in parse_millet_list(verify_raw) for pkg in clean_pkgs)
    if millet_ok:
        log(f"[MILLET] Đã cập nhật MILLET_NO_RESTRICT_APP (+{added_to_millet} mới, tổng {len(merged)} gói).")
    else:
        log(f"[MILLET] Cảnh báo khi ghi MILLET_NO_RESTRICT_APP: {millet_res.combined or 'ROM từ chối ghi'}")

    doze_ok = 0
    appops_ok = 0
    for pkg in clean_pkgs:
        r_doze = adb.shell(f"dumpsys deviceidle whitelist +{pkg}", serial=serial, timeout=10)
        if r_doze.ok:
            doze_ok += 1
        r_ops = adb.shell(f"cmd appops set {pkg} RUN_ANY_IN_BACKGROUND allow", serial=serial, timeout=10)
        if r_ops.ok:
            appops_ok += 1
        log(
            f"  • {pkg}: Doze={'OK' if r_doze.ok else 'Lỗi'} | AppOps={'OK' if r_ops.ok else 'Lỗi'}"
        )

    if send_fcm_ping:
        # Mirrors defpackage.checkupdate.Il one-shot broadcast without any background service
        for target_pkg in ("com.google.android.gms", "com.google.android.gsf"):
            for action in (
                "com.google.android.intent.action.GTALK_HEARTBEAT",
                "com.google.android.intent.action.MCS_HEARTBEAT",
            ):
                adb.shell(f"am broadcast -a {action} -p {target_pkg}", serial=serial, timeout=10)
        log("[FCM] Đã gửi tín hiệu đánh thức FCM một lần (GTALK/MCS_HEARTBEAT), không cài dịch vụ ngầm.")

    return {
        "millet_ok": millet_ok,
        "millet_total": len(merged),
        "doze_ok": doze_ok,
        "appops_ok": appops_ok,
        "total_pkgs": len(clean_pkgs),
    }


def restore_millet_backup(
    adb: AdbClient,
    serial: str,
    backups_dir: str,
    log_cb: Optional[Callable[[str], None]] = None,
) -> bool:
    backup = load_backup(backups_dir, serial)
    if "millet_original" not in backup:
        raise ValueError("Chưa có bản sao lưu MILLET_NO_RESTRICT_APP cho thiết bị này.")
    orig = backup["millet_original"]
    if not orig:
        res = adb.shell("settings delete system MILLET_NO_RESTRICT_APP", serial=serial, timeout=15)
    else:
        res = adb.shell(f"settings put system MILLET_NO_RESTRICT_APP '{orig}'", serial=serial, timeout=15)
    if log_cb:
        log_cb(f"Khôi phục MILLET_NO_RESTRICT_APP: {'Thành công' if res.ok else res.combined}")
    return res.ok


# ---------------------------------------------------------------------------
# 2. Locale (Vietnamese / English / Backup & Restore)
# ---------------------------------------------------------------------------

def get_locale_status(adb: AdbClient, serial: str, backups_dir: str) -> dict[str, str]:
    current = adb.get_setting("system", "system_locales", serial)
    backup = load_backup(backups_dir, serial)
    return {
        "current": current or "(chưa đặt / mặc định ROM)",
        "backup": backup.get("locale_original", "(chưa sao lưu)"),
        "has_backup": "1" if "locale_original" in backup else "0",
    }


def set_system_locale(
    adb: AdbClient,
    serial: str,
    locale_tag: str,
    backups_dir: str,
    log_cb: Optional[Callable[[str], None]] = None,
) -> bool:
    tag = locale_tag.strip().replace("_", "-")
    if not LOCALE_TAG_RE.match(tag) or len(tag) > 120:
        raise ValueError(f"Mã ngôn ngữ không hợp lệ: {locale_tag}")

    current = adb.get_setting("system", "system_locales", serial)
    backup = load_backup(backups_dir, serial)
    if "locale_original" not in backup:
        backup["locale_original"] = current
        backup["locale_saved_at"] = datetime.now().isoformat(timespec="seconds")
        save_backup(backups_dir, serial, backup)
        if log_cb:
            log_cb(f"Đã sao lưu system_locales gốc: '{current or '(trống)'}'.")

    res = adb.shell(f"settings put system system_locales '{tag}'", serial=serial, timeout=15)
    after = adb.get_setting("system", "system_locales", serial)
    ok = res.ok and after == tag
    if log_cb:
        if ok:
            log_cb(
                f"Đã đặt system_locales = '{after}'. Lưu ý: trên HyperOS cần khởi động lại máy hoặc dùng Theme MTZ để giao diện hệ thống chuyển sang Tiếng Việt đầy đủ."
            )
        else:
            log_cb(f"Không đặt được system_locales: {res.combined or f'đọc lại được {after}'}")
    return ok


def restore_locale_backup(
    adb: AdbClient,
    serial: str,
    backups_dir: str,
    log_cb: Optional[Callable[[str], None]] = None,
) -> bool:
    backup = load_backup(backups_dir, serial)
    if "locale_original" not in backup:
        raise ValueError("Chưa có bản sao lưu ngôn ngữ gốc cho thiết bị này.")
    orig = backup["locale_original"]
    if not orig:
        res = adb.shell("settings delete system system_locales", serial=serial, timeout=15)
    else:
        res = adb.shell(f"settings put system system_locales '{orig}'", serial=serial, timeout=15)
    if log_cb:
        log_cb(
            f"Đã khôi phục system_locales về '{orig or '(xóa key về mặc định ROM)'}': "
            f"{'OK' if res.ok else res.combined}"
        )
    return res.ok


# ---------------------------------------------------------------------------
# 3. Safe Debloat (User 0 uninstall & restore)
# ---------------------------------------------------------------------------

def scan_debloat_packages(adb: AdbClient, serial: str) -> list[dict]:
    active_user0 = adb.list_all_packages_for_user(serial)
    all_pkgs = adb.list_installed_packages(serial)
    rows: list[dict] = []
    for item in DEBLOAT_PACKAGES:
        is_active = item.package in active_user0
        exists_in_rom = item.package in all_pkgs or is_active
        if is_active:
            status = "Đang cài"
        elif exists_in_rom:
            status = "Đã gỡ (User 0)"
        else:
            status = "Không có trên ROM"
        rows.append(
            {
                "package": item.package,
                "name": item.name,
                "category": item.category,
                "risk": item.risk,
                "description": item.description,
                "status": status,
                "installed": is_active,
                "restorable": (not is_active) and exists_in_rom,
                "selected": is_active and item.default_selected,
            }
        )
    return rows


def uninstall_packages(
    adb: AdbClient,
    serial: str,
    packages: list[str],
    backups_dir: str,
    log_cb: Optional[Callable[[str], None]] = None,
) -> tuple[int, int]:
    def log(msg: str) -> None:
        if log_cb:
            log_cb(msg)

    ok_count = 0
    fail_count = 0
    backup = load_backup(backups_dir, serial)
    uninstalled_hist = set(backup.get("uninstalled_packages", []))

    for pkg in packages:
        pkg = pkg.strip()
        if not PACKAGE_NAME_RE.match(pkg):
            log(f"Bỏ qua package không hợp lệ: {pkg}")
            fail_count += 1
            continue
        res = adb.shell(f"pm uninstall -k --user 0 {pkg}", serial=serial, timeout=20)
        if res.ok and "Success" in res.stdout:
            ok_count += 1
            uninstalled_hist.add(pkg)
            log(f"[GỠ OK] {pkg}")
        else:
            fail_count += 1
            log(f"[THẤT BẠI] {pkg}: {res.combined}")

    backup["uninstalled_packages"] = sorted(uninstalled_hist)
    backup["debloat_updated_at"] = datetime.now().isoformat(timespec="seconds")
    save_backup(backups_dir, serial, backup)
    return ok_count, fail_count


def restore_packages(
    adb: AdbClient,
    serial: str,
    packages: list[str],
    backups_dir: str,
    log_cb: Optional[Callable[[str], None]] = None,
) -> tuple[int, int]:
    def log(msg: str) -> None:
        if log_cb:
            log_cb(msg)

    ok_count = 0
    fail_count = 0
    backup = load_backup(backups_dir, serial)
    uninstalled_hist = set(backup.get("uninstalled_packages", []))

    for pkg in packages:
        pkg = pkg.strip()
        if not PACKAGE_NAME_RE.match(pkg):
            log(f"Bỏ qua package không hợp lệ: {pkg}")
            fail_count += 1
            continue
        res = adb.shell(f"cmd package install-existing --user 0 {pkg}", serial=serial, timeout=20)
        if not res.ok or "installed" not in res.combined.lower():
            res = adb.shell(f"cmd package install-existing {pkg}", serial=serial, timeout=20)
        if res.ok and "installed" in res.combined.lower():
            ok_count += 1
            uninstalled_hist.discard(pkg)
            log(f"[KHÔI PHỤC OK] {pkg}")
        else:
            fail_count += 1
            log(f"[KHÔNG KHÔI PHỤC ĐƯỢC] {pkg}: {res.combined}")

    backup["uninstalled_packages"] = sorted(uninstalled_hist)
    save_backup(backups_dir, serial, backup)
    return ok_count, fail_count


# ---------------------------------------------------------------------------
# 4. Bank-Ready Security Lock
# ---------------------------------------------------------------------------

def check_bank_ready_status(adb: AdbClient, serial: str) -> dict[str, object]:
    info = adb.get_device_info(serial)
    adb_on = info.get("adb_enabled") == "1"
    dev_on = info.get("dev_enabled") == "1"
    return {
        "adb_enabled": adb_on,
        "dev_enabled": dev_on,
        "bank_ready": (not adb_on) and (not dev_on),
        "info": info,
    }


def lock_developer_and_adb(
    adb: AdbClient,
    serial: str,
    log_cb: Optional[Callable[[str], None]] = None,
) -> bool:
    """Disable Developer Options first, then USB Debugging in one atomic shell call."""
    # CRITICAL ORDER: development_settings_enabled=0 BEFORE adb_enabled=0 so ADB doesn't drop mid-way.
    cmd = "settings put global development_settings_enabled 0 && settings put global adb_enabled 0"
    res = adb.shell(cmd, serial=serial, timeout=10)
    # Turning adb_enabled=0 immediately terminates adbd on USB, so connection reset/non-zero can happen after applying.
    if log_cb:
        if res.ok:
            log_cb("Đã tắt Tùy chọn nhà phát triển và Gỡ lỗi USB (ADB). Thiết bị đã sạch và sẵn sàng cho VNeID / App Ngân hàng!")
        else:
            log_cb(
                f"Đã gửi lệnh tắt Tùy chọn nhà phát triển & ADB (kết nối ADB ngắt ngay sau khi tắt là bình thường: {res.combined}). "
                "Hãy rút cáp và kiểm tra mục Cài đặt bổ sung xem Tùy chọn nhà phát triển đã ẩn chưa."
            )
    return True
