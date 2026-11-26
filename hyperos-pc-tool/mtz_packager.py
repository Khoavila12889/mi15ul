"""MTZ -> Xiaomi Themes (.mrm / .mrc) converter & ADB deployer.

Faithfully reproduces the packaging logic from defpackage.d6, defpackage.lstate,
and vn.mtztool.mobile.ThemeApplyShell.lO in HyperOS_Tool.apk without installing
any APK or running any background service on the phone.
"""

from __future__ import annotations

import copy
import hashlib
import io
import json
import os
import re
import shutil
import tempfile
import uuid
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass
from typing import Callable, Optional
from PIL import Image

from adb_client import AdbClient

THEME_DATA_ROOT = "/sdcard/Android/data/com.android.thememanager/files/MIUI/theme/.data"
THEME_DATA_CONTENT_PREFIX = (
    "/system/../storage/emulated/0/Android/data/com.android.thememanager/files/MIUI/theme/.data/content/theme"
)

VALID_NAME_RE = re.compile(r"^[a-zA-Z0-9][a-zA-Z0-9._-]*$")
MODULE_MAP = {
    "com.miui.home": "launcher",
    "com.android.systemui": "statusbar",
    "com.android.contacts": "contact",
}
LOCALES_4 = ("fallback", "zh_CN", "en_US", "vi_VN")
MAX_MTZ_BYTES = 367_001_600
MAX_ENTRY_BYTES = 83_886_080
MAX_TOTAL_UNPACKED = 471_859_200
MAX_ZIP_ENTRIES = 1200


@dataclass
class StagedMtz:
    theme_uuid: str
    title: str
    module_count: int
    file_count: int
    stage_dir: str
    staged_rel_files: list[tuple[str, str]]  # (relative_path, sha1_hex)


def java_name_uuid_from_bytes(data: bytes) -> str:
    """Exact replica of java.util.UUID.nameUUIDFromBytes(byte[] name)."""
    digest = bytearray(hashlib.md5(data).digest())
    digest[6] = (digest[6] & 0x0F) | 0x30  # version 3
    digest[8] = (digest[8] & 0x3F) | 0x80  # IETF variant
    return str(uuid.UUID(bytes=bytes(digest)))


def sha1_file(path: str) -> str:
    h = hashlib.sha1()
    with open(path, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def sha1_bytes(data: bytes) -> str:
    return hashlib.sha1(data).hexdigest()


def _localized_str(value: str) -> dict[str, str]:
    return {"fallback": value, "zh_CN": value}


def _localized_list(items: list[str]) -> dict[str, list[str]]:
    return {loc: list(items) for loc in LOCALES_4}


def _res_ref(local_id: str, resource_code: str) -> dict[str, object]:
    return {
        "localId": local_id,
        "resourceCode": resource_code,
        "extraMeta": {},
        "metaPath": None,
        "contentPath": None,
    }


def _apply_common_meta(
    obj: dict,
    *,
    platform: int,
    author: str,
    designer: str,
    title: str,
    description: str,
    local_id: str,
    file_hash: str,
    size: int,
) -> None:
    obj["localId"] = local_id
    obj["hash"] = file_hash
    obj["size"] = size
    obj["platform"] = platform
    obj["authors"] = _localized_str(author)
    obj["designers"] = _localized_str(designer)
    obj["titles"] = _localized_str(title)
    obj["descriptions"] = _localized_str(description)


def _parse_description_xml(raw_xml: bytes) -> dict[str, str]:
    wanted = {"title", "author", "designer", "description", "version", "uiVersion"}
    result: dict[str, str] = {}
    root = ET.fromstring(raw_xml)
    for elem in root.iter():
        if elem.tag in wanted and elem.text is not None:
            result[elem.tag] = elem.text.strip()
    return result


def _validate_zip_path(raw_name: str) -> str:
    norm = raw_name.replace("\\", "/")
    if not norm or norm.startswith("/") or norm.startswith("./") or "\x00" in norm:
        raise ValueError(f"Đường dẫn không hợp lệ trong MTZ: {raw_name}")
    parts = [p for p in norm.split("/") if p != ""]
    if not parts or any(p in ("..", ".") for p in parts):
        raise ValueError(f"Đường dẫn không an toàn trong MTZ: {raw_name}")
    return norm


def _find_preview_by_prefix(keys: list[str], prefix: str) -> Optional[str]:
    low_prefix = prefix.lower()
    for k in sorted(keys):
        if k.lower().startswith(low_prefix):
            return k
    return None


def _write_staged_file(stage_dir: str, rel_path: str, data: bytes) -> str:
    rel_norm = rel_path.replace("\\", "/").lstrip("/")
    if not VALID_NAME_RE.match(rel_norm.split("/")[0]) or ".." in rel_norm.split("/"):
        raise ValueError(f"Đường dẫn đích không an toàn: {rel_path}")
    dest = os.path.join(stage_dir, *rel_norm.split("/"))
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    with open(dest, "wb") as f:
        f.write(data)
    return sha1_bytes(data)


def prepare_mtz_stage(
    mtz_path: str,
    template_path: str,
    work_dir: Optional[str] = None,
    log_cb: Optional[Callable[[str], None]] = None,
) -> StagedMtz:
    """Unpack *mtz_path* and generate Xiaomi Themes .mrm/.mrc/preview structure."""
    def log(msg: str) -> None:
        if log_cb:
            log_cb(msg)

    if not os.path.isfile(mtz_path):
        raise FileNotFoundError(f"Không tìm thấy file MTZ: {mtz_path}")
    if not os.path.isfile(template_path):
        raise FileNotFoundError(f"Thiếu metadata_template.json tại: {template_path}")

    mtz_size = os.path.getsize(mtz_path)
    if mtz_size > MAX_MTZ_BYTES:
        raise ValueError("File MTZ vượt giới hạn 350 MB.")

    with open(template_path, "r", encoding="utf-8") as tf:
        templates = json.load(tf)

    mtz_sha1 = sha1_file(mtz_path)
    theme_uuid = java_name_uuid_from_bytes(f"mtztool-theme:{mtz_sha1}".encode("utf-8"))
    log(f"MTZ SHA-1: {mtz_sha1[:12]}... -> UUID: {theme_uuid}")

    stage_dir = work_dir or tempfile.mkdtemp(prefix="mtz_stage_")
    staged_files: list[tuple[str, str]] = []
    total_unpacked = 0

    with zipfile.ZipFile(mtz_path, "r") as zf:
        infos = [zi for zi in zf.infolist() if not zi.is_dir()]
        if len(infos) > MAX_ZIP_ENTRIES:
            raise ValueError("MTZ chứa quá nhiều tệp.")

        seen_paths: set[str] = set()
        desc_info: Optional[zipfile.ZipInfo] = None
        previews: dict[str, zipfile.ZipInfo] = {}
        modules: list[tuple[str, zipfile.ZipInfo]] = []

        for zi in infos:
            norm = _validate_zip_path(zi.filename)
            if norm in seen_paths:
                raise ValueError(f"Trùng tệp trong MTZ: {norm}")
            seen_paths.add(norm)
            if zi.file_size > MAX_ENTRY_BYTES:
                raise ValueError(f"Tệp trong MTZ quá lớn: {norm}")

            if norm == "description.xml":
                desc_info = zi
                continue

            if norm.startswith("preview/"):
                if norm.count("/") == 1:
                    fname = norm.split("/", 1)[1]
                    if VALID_NAME_RE.match(fname) and fname.lower().endswith((".jpg", ".jpeg", ".png")):
                        previews[fname] = zi
                continue

            if "/" not in norm:
                if not VALID_NAME_RE.match(norm):
                    raise ValueError(f"Tên module không hợp lệ: {norm}")
                mapped = MODULE_MAP.get(norm, norm)
                modules.append((mapped, zi))

        if desc_info is None:
            raise ValueError("MTZ thiếu description.xml.")
        if not modules:
            raise ValueError("MTZ không chứa module nào có thể nạp.")

        modules.sort(key=lambda pair: pair[0])
        mapped_names = [m[0] for m in modules]
        if len(set(mapped_names)) != len(mapped_names):
            raise ValueError("Tên module bị trùng sau khi ánh xạ.")

        desc_bytes = zf.read(desc_info)
        if len(desc_bytes) > 1_048_576:
            raise ValueError("description.xml trong MTZ quá lớn.")
        meta_xml = _parse_description_xml(desc_bytes)

        title = (meta_xml.get("title") or "Chủ đề MTZ")[:200]
        author = (meta_xml.get("author") or "")[:120]
        designer = (meta_xml.get("designer") or "")[:120] or author
        description = (meta_xml.get("description") or "")[:4000]
        try:
            ui_ver_raw = int(meta_xml.get("uiVersion", "17"))
            ui_version = ui_ver_raw if 1 <= ui_ver_raw < 1000 else 17
        except ValueError:
            ui_version = 17

        # 1. Empty theme content/theme/<theme_uuid>.mrc
        rel_theme_mrc = f"content/theme/{theme_uuid}.mrc"
        h_empty = _write_staged_file(stage_dir, rel_theme_mrc, b"")
        staged_files.append((rel_theme_mrc, h_empty))

        # 2. Build theme root metadata object
        theme_meta = copy.deepcopy(templates["theme"])
        _apply_common_meta(
            theme_meta,
            platform=ui_version,
            author=author,
            designer=designer,
            title=title,
            description=description,
            local_id=theme_uuid,
            file_hash=mtz_sha1,
            size=mtz_size,
        )
        theme_meta["contentPath"] = f"{THEME_DATA_CONTENT_PREFIX}/{theme_uuid}.mrc"

        sub_resources: list[dict[str, object]] = []
        for idx, (mod_name, zi) in enumerate(modules, start=1):
            mod_bytes = zf.read(zi)
            total_unpacked += len(mod_bytes)
            if total_unpacked > MAX_TOTAL_UNPACKED:
                raise ValueError("Tổng dung lượng giải nén MTZ vượt giới hạn an toàn.")

            mod_uuid = java_name_uuid_from_bytes(
                f"mtztool-module:{mtz_sha1}:{mod_name}".encode("utf-8")
            )
            sub_resources.append(_res_ref(mod_uuid, mod_name))

            rel_mod_mrc = f"content/{mod_name}/{mod_uuid}.mrc"
            mod_sha1 = _write_staged_file(stage_dir, rel_mod_mrc, mod_bytes)
            staged_files.append((rel_mod_mrc, mod_sha1))

            tmpl_key = "launcher" if mod_name == "launcher" else "component"
            mod_meta = copy.deepcopy(templates[tmpl_key])
            _apply_common_meta(
                mod_meta,
                platform=ui_version,
                author=author,
                designer=designer,
                title=title,
                description=description,
                local_id=mod_uuid,
                file_hash=mod_sha1,
                size=len(mod_bytes),
            )
            mod_meta["parentResources"] = [_res_ref(theme_uuid, "theme")]
            mod_meta["subResources"] = []

            if mod_name == "launcher":
                launcher_previews = sorted(
                    k for k in previews.keys() if k.lower().startswith("preview_launcher_")
                )
                mod_meta["builtInThumbnails"] = _localized_list(launcher_previews[:1])
                mod_meta["builtInPreviews"] = _localized_list(launcher_previews)
                for lp in launcher_previews:
                    lp_bytes = zf.read(previews[lp])
                    total_unpacked += len(lp_bytes)
                    if total_unpacked > MAX_TOTAL_UNPACKED:
                        raise ValueError("Tổng dung lượng giải nén MTZ vượt giới hạn an toàn.")
                    rel_lp = f"preview/launcher/{mod_uuid}/{lp}"
                    h_lp = _write_staged_file(stage_dir, rel_lp, lp_bytes)
                    staged_files.append((rel_lp, h_lp))

            rel_mod_mrm = f"meta/{mod_name}/{mod_uuid}.mrm"
            mod_mrm_bytes = json.dumps(mod_meta, indent=4, ensure_ascii=False).encode("utf-8")
            h_mod_mrm = _write_staged_file(stage_dir, rel_mod_mrm, mod_mrm_bytes)
            staged_files.append((rel_mod_mrm, h_mod_mrm))

        log(f"Đã đóng gói {len(modules)} module cho \"{title}\" (platform={ui_version}).")

        # 3. Theme previews & cover image
        theme_meta["subResources"] = sub_resources
        preview_keys = sorted(previews.keys())
        has_exact_cover = "preview_cover_0.jpg" in previews
        cover_src_key: Optional[str] = "preview_cover_0.jpg" if has_exact_cover else None
        if not has_exact_cover and preview_keys:
            for pfx in (
                "preview_cover_0.",
                "preview_launcher_0.",
                "preview_launcher_",
                "preview_0.",
                "preview_lockscreen_",
            ):
                cover_src_key = _find_preview_by_prefix(preview_keys, pfx)
                if cover_src_key:
                    break
            if not cover_src_key:
                cover_src_key = preview_keys[0]

            raw_cover = zf.read(previews[cover_src_key])
            if cover_src_key.lower().endswith(".png"):
                img = Image.open(io.BytesIO(raw_cover)).convert("RGB")
                if img.width > 1600 or img.height > 3200:
                    img.thumbnail((1600, 3200))
                buf = io.BytesIO()
                img.save(buf, format="JPEG", quality=88)
                cover_bytes = buf.getvalue()
            else:
                cover_bytes = raw_cover
            total_unpacked += len(cover_bytes)
            rel_cover = f"preview/theme/{theme_uuid}/preview_cover_0.jpg"
            h_cover = _write_staged_file(stage_dir, rel_cover, cover_bytes)
            staged_files.append((rel_cover, h_cover))

        preview_list: list[str] = []
        if has_exact_cover or cover_src_key is not None:
            preview_list.append("preview_cover_0.jpg")
        for pk in preview_keys:
            if pk != "preview_cover_0.jpg":
                preview_list.append(pk)

        theme_meta["builtInThumbnails"] = _localized_list(preview_list[:1])
        theme_meta["builtInPreviews"] = _localized_list(preview_list)

        for pk in preview_keys:
            pk_bytes = zf.read(previews[pk])
            total_unpacked += len(pk_bytes)
            if total_unpacked > MAX_TOTAL_UNPACKED:
                raise ValueError("Tổng dung lượng giải nén MTZ vượt giới hạn an toàn.")
            rel_pk = f"preview/theme/{theme_uuid}/{pk}"
            h_pk = _write_staged_file(stage_dir, rel_pk, pk_bytes)
            staged_files.append((rel_pk, h_pk))

        # 4. Write root meta/theme/<theme_uuid>.mrm
        rel_theme_mrm = f"meta/theme/{theme_uuid}.mrm"
        theme_mrm_bytes = json.dumps(theme_meta, indent=4, ensure_ascii=False).encode("utf-8")
        h_theme_mrm = _write_staged_file(stage_dir, rel_theme_mrm, theme_mrm_bytes)
        staged_files.append((rel_theme_mrm, h_theme_mrm))

    return StagedMtz(
        theme_uuid=theme_uuid,
        title=title,
        module_count=len(modules),
        file_count=len(staged_files),
        stage_dir=stage_dir,
        staged_rel_files=staged_files,
    )


def deploy_mtz_via_adb(
    adb: AdbClient,
    serial: str,
    mtz_path: str,
    template_path: str,
    overwrite: bool = False,
    open_apply_screen: bool = True,
    log_cb: Optional[Callable[[str], None]] = None,
) -> tuple[bool, str, StagedMtz]:
    """Convert .mtz on PC, push to phone via ADB, verify SHA-1, and open ThemeDetailActivity."""
    def log(msg: str) -> None:
        if log_cb:
            log_cb(msg)

    temp_root = tempfile.mkdtemp(prefix="mtz_pc_stage_")
    try:
        staged = prepare_mtz_stage(mtz_path, template_path, work_dir=temp_root, log_cb=log_cb)
        uid = staged.theme_uuid

        if not overwrite:
            check_cmd = (
                f"if test -e '{THEME_DATA_ROOT}/meta/theme/{uid}.mrm' && "
                f"test -e '{THEME_DATA_ROOT}/content/theme/{uid}.mrc'; then "
                "echo MTZ_ALREADY; else echo MTZ_NEW; fi"
            )
            chk = adb.shell(check_cmd, serial=serial, timeout=15)
            if "MTZ_ALREADY" in chk.stdout:
                log(f"Theme \"{staged.title}\" ({uid}) đã có sẵn trong Xiaomi Themes.")
                if open_apply_screen:
                    open_theme_apply_screen(adb, serial, uid, log_cb=log_cb)
                return True, f"Theme \"{staged.title}\" đã có sẵn và đã mở màn hình Áp dụng.", staged

        remote_tmp = f"/data/local/tmp/mtz_stage_{uid}"
        adb.shell(f"rm -rf '{remote_tmp}' && mkdir -p '{remote_tmp}'", serial=serial, timeout=15)

        log(f"Đang đẩy {staged.file_count} tệp lên vùng tạm ADB trên điện thoại...")
        push_res = adb.push(staged.stage_dir + "/.", remote_tmp, serial=serial, timeout=180)
        if not push_res.ok:
            raise RuntimeError(f"adb push thất bại: {push_res.combined}")

        # Build verification + move script and execute inside /data/local/tmp
        lines = [
            "set -eu",
            f"SRC='{remote_tmp}'",
            f"DST='{THEME_DATA_ROOT}'",
            "mkdir -p \"$DST\"",
        ]
        for rel_path, expected_sha1 in staged.staged_rel_files:
            parent_rel = os.path.dirname(rel_path).replace("\\", "/")
            if parent_rel:
                lines.append(f"mkdir -p \"$DST/{parent_rel}\"")
            lines.append(
                f"cp -f \"$SRC/{rel_path}\" \"$DST/{rel_path}\" && "
                f"chmod 0660 \"$DST/{rel_path}\" 2>/dev/null || true"
            )
            lines.append(
                f"test \"$(sha1sum \"$DST/{rel_path}\" | cut -d' ' -f1)\" = '{expected_sha1}' "
                f"|| {{ echo 'MTZ_SHA1_ERR:{rel_path}'; exit 1; }}"
            )
        lines.append(f"rm -rf '{remote_tmp}'")
        lines.append("echo MTZ_DEPLOY_OK")

        script_path = os.path.join(temp_root, "deploy.sh")
        with open(script_path, "w", encoding="utf-8", newline="\n") as sf:
            sf.write("\n".join(lines) + "\n")

        remote_script = f"/data/local/tmp/mtz_deploy_{uid}.sh"
        push_sh = adb.push(script_path, remote_script, serial=serial, timeout=30)
        if not push_sh.ok:
            raise RuntimeError(f"Không đẩy được script cài theme: {push_sh.combined}")

        log("Đang nạp và xác minh SHA-1 vào thư mục Xiaomi Themes...")
        run_res = adb.shell(f"sh '{remote_script}'; rc=$?; rm -f '{remote_script}'; exit $rc", serial=serial, timeout=120)
        if "MTZ_DEPLOY_OK" not in run_res.stdout:
            adb.shell(f"rm -rf '{remote_tmp}' '{remote_script}'", serial=serial, timeout=15)
            raise RuntimeError(f"Nạp theme thất bại: {run_res.combined}")

        log(f"Đã nạp thành công \"{staged.title}\" ({staged.file_count} tệp, khớp SHA-1 100%).")
        if open_apply_screen:
            open_theme_apply_screen(adb, serial, uid, log_cb=log_cb)
        return True, f"Đã nạp \"{staged.title}\" thành công.", staged
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)


def open_theme_apply_screen(
    adb: AdbClient,
    serial: str,
    theme_uuid: str,
    log_cb: Optional[Callable[[str], None]] = None,
) -> bool:
    """Open Xiaomi ThemeDetailActivity for manual Apply tap (matches ThemeApplyShell.lO)."""
    cmd = (
        "am start -S --user 0 "
        "-a android.intent.action.VIEW "
        "-n com.android.thememanager/.module.detail.view.ThemeDetailActivity "
        f"-d 'ViewLocalResource://view.local.resource#{theme_uuid}' "
        "--es REQUEST_RESOURCE_CODE theme "
        "--ei REQUEST_SOURCE_TYPE 1"
    )
    res = adb.shell(cmd, serial=serial, timeout=20)
    ok = res.ok and "Error" not in res.combined and "Exception" not in res.combined
    if log_cb:
        if ok:
            log_cb("Đã mở trang chi tiết Theme trên điện thoại — hãy bấm nút [Áp dụng] thủ công trên màn hình.")
        else:
            log_cb(f"Cảnh báo: không mở thẳng được ThemeDetailActivity ({res.combined}). Hãy mở ứng dụng Chủ đề (Themes) trên điện thoại để áp dụng.")
    return ok
