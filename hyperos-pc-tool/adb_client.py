"""ADB client wrapper for HyperOS PC Optimizer (Windows)."""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
from dataclasses import dataclass
from typing import Optional

CREATE_NO_WINDOW = 0x08000000 if os.name == "nt" else 0


@dataclass
class AdbResult:
    """Result of an ADB command invocation."""

    returncode: int
    stdout: str
    stderr: str

    @property
    def ok(self) -> bool:
        return self.returncode == 0

    @property
    def combined(self) -> str:
        parts = [p.strip() for p in (self.stdout, self.stderr) if p and p.strip()]
        return "\n".join(parts)


@dataclass
class DeviceEntry:
    """Connected ADB device descriptor."""

    serial: str
    state: str
    model: str = ""
    product: str = ""
    device: str = ""

    @property
    def display_label(self) -> str:
        name = self.model.replace("_", " ").strip() or self.device or "Android"
        return f"{name} ({self.serial}) [{self.state}]"


def _verify_adb_binary(path: str) -> bool:
    """Return True if *path* is an executable Android Debug Bridge binary."""
    if not path or not os.path.isfile(path):
        return False
    try:
        proc = subprocess.run(
            [path, "version"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=5,
            creationflags=CREATE_NO_WINDOW,
        )
        return proc.returncode == 0 and "Android Debug Bridge" in (proc.stdout + proc.stderr)
    except Exception:
        return False


def find_adb(custom_path: Optional[str] = None) -> str:
    """Locate a working adb.exe on Windows, avoiding stub files without .exe."""
    candidates: list[str] = []
    if custom_path:
        candidates.append(custom_path)

    local_app_data = os.environ.get("LOCALAPPDATA", "")
    if local_app_data:
        candidates.append(os.path.join(local_app_data, "Android", "Sdk", "platform-tools", "adb.exe"))

    for env_var in ("ANDROID_HOME", "ANDROID_SDK_ROOT"):
        sdk = os.environ.get(env_var, "")
        if sdk:
            candidates.append(os.path.join(sdk, "platform-tools", "adb.exe"))

    user_profile = os.environ.get("USERPROFILE", "")
    if user_profile:
        candidates.append(os.path.join(user_profile, "platform-tools", "adb.exe"))

    for name in ("adb.exe", "adb"):
        resolved = shutil.which(name)
        if resolved:
            candidates.append(resolved)

    seen: set[str] = set()
    for cand in candidates:
        norm = os.path.normcase(os.path.abspath(cand))
        if norm in seen:
            continue
        seen.add(norm)
        if _verify_adb_binary(cand):
            return os.path.abspath(cand)

    raise FileNotFoundError(
        "Không tìm thấy adb.exe hợp lệ. Vui lòng cài Android Platform-Tools hoặc chỉ định đường dẫn adb.exe."
    )


class AdbClient:
    """Lightweight synchronous ADB runner over subprocess."""

    def __init__(self, adb_path: Optional[str] = None) -> None:
        self.adb_path = find_adb(adb_path)

    def run(
        self,
        args: list[str],
        serial: Optional[str] = None,
        timeout: int = 30,
    ) -> AdbResult:
        cmd = [self.adb_path]
        if serial:
            cmd.extend(["-s", serial])
        cmd.extend(args)
        try:
            proc = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
                creationflags=CREATE_NO_WINDOW,
            )
            return AdbResult(proc.returncode, proc.stdout.strip(), proc.stderr.strip())
        except subprocess.TimeoutExpired as exc:
            out = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
            err = (exc.stderr or "") if isinstance(exc.stderr, str) else ""
            return AdbResult(-1, out.strip(), f"Timeout sau {timeout}s. {err}".strip())
        except Exception as exc:
            return AdbResult(-1, "", str(exc))

    def version(self) -> str:
        res = self.run(["version"], timeout=5)
        return res.stdout.splitlines()[0] if res.stdout else res.combined

    def list_devices(self) -> list[DeviceEntry]:
        res = self.run(["devices", "-l"], timeout=10)
        if not res.ok:
            return []
        devices: list[DeviceEntry] = []
        for raw_line in res.stdout.splitlines():
            line = raw_line.strip()
            if not line or line.startswith("List of devices") or line.startswith("*"):
                continue
            parts = line.split()
            if len(parts) < 2:
                continue
            serial, state = parts[0], parts[1]
            kv: dict[str, str] = {}
            for token in parts[2:]:
                if ":" in token:
                    k, v = token.split(":", 1)
                    kv[k] = v
            devices.append(
                DeviceEntry(
                    serial=serial,
                    state=state,
                    model=kv.get("model", ""),
                    product=kv.get("product", ""),
                    device=kv.get("device", ""),
                )
            )
        return devices

    def shell(self, command: str, serial: Optional[str] = None, timeout: int = 30) -> AdbResult:
        return self.run(["shell", command], serial=serial, timeout=timeout)

    def push(
        self,
        local_path: str,
        remote_path: str,
        serial: Optional[str] = None,
        timeout: int = 180,
    ) -> AdbResult:
        return self.run(["push", local_path, remote_path], serial=serial, timeout=timeout)

    def list_installed_packages(self, serial: str) -> set[str]:
        """Return all installed package names (including disabled/system)."""
        res = self.shell("pm list packages", serial=serial, timeout=20)
        if not res.ok and not res.stdout:
            return set()
        pkgs: set[str] = set()
        for line in res.stdout.splitlines():
            line = line.strip()
            if line.startswith("package:"):
                pkgs.add(line[len("package:"):].strip())
        return pkgs

    def list_all_packages_for_user(self, serial: str) -> set[str]:
        """Return packages currently active/installed for user 0."""
        res = self.shell("pm list packages --user 0", serial=serial, timeout=20)
        if not res.ok and not res.stdout:
            return self.list_installed_packages(serial)
        pkgs: set[str] = set()
        for line in res.stdout.splitlines():
            line = line.strip()
            if line.startswith("package:"):
                pkgs.add(line[len("package:"):].strip())
        return pkgs

    def get_prop(self, prop: str, serial: str) -> str:
        res = self.shell(f"getprop {shlex.quote(prop)}", serial=serial, timeout=10)
        return res.stdout.strip() if res.ok else ""

    def get_setting(self, namespace: str, key: str, serial: str) -> str:
        res = self.shell(f"settings get {namespace} {key}", serial=serial, timeout=10)
        if not res.ok:
            return ""
        val = res.stdout.strip()
        return "" if val == "null" else val

    def get_device_info(self, serial: str) -> dict[str, str]:
        """Fetch consolidated device status in minimal ADB calls."""
        script = (
            "echo MODEL=$(getprop ro.product.model); "
            "echo BRAND=$(getprop ro.product.brand); "
            "echo ANDROID=$(getprop ro.build.version.release); "
            "echo SDK=$(getprop ro.build.version.sdk); "
            "echo OS_VER=$(getprop ro.mi.os.version.name); "
            "echo MIUI_VER=$(getprop ro.miui.ui.version.name); "
            "echo LOCALE=$(settings get system system_locales); "
            "echo ADB_EN=$(settings get global adb_enabled); "
            "echo DEV_EN=$(settings get global development_settings_enabled); "
            "echo MILLET=$(settings get system MILLET_NO_RESTRICT_APP)"
        )
        res = self.shell(script, serial=serial, timeout=15)
        info: dict[str, str] = {
            "model": "",
            "brand": "",
            "android": "",
            "sdk": "",
            "hyperos": "",
            "system_locales": "",
            "adb_enabled": "",
            "dev_enabled": "",
            "millet_raw": "",
            "millet_count": "0",
        }
        if not res.stdout:
            return info
        raw_map: dict[str, str] = {}
        for line in res.stdout.splitlines():
            if "=" in line:
                k, v = line.split("=", 1)
                val = v.strip()
                raw_map[k.strip()] = "" if val == "null" else val

        info["model"] = raw_map.get("MODEL", "")
        info["brand"] = raw_map.get("BRAND", "")
        info["android"] = raw_map.get("ANDROID", "")
        info["sdk"] = raw_map.get("SDK", "")
        info["hyperos"] = raw_map.get("OS_VER") or raw_map.get("MIUI_VER") or ""
        info["system_locales"] = raw_map.get("LOCALE", "")
        info["adb_enabled"] = raw_map.get("ADB_EN", "")
        info["dev_enabled"] = raw_map.get("DEV_EN", "")
        millet_raw = raw_map.get("MILLET", "")
        info["millet_raw"] = millet_raw
        items = [p.strip() for p in millet_raw.split(",") if p.strip()]
        info["millet_count"] = str(len(items))
        return info
