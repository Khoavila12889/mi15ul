# <img src="crates/android-dashboard/assets/icons/app.png" width="32" height="32" align="absmiddle" alt="" />&nbsp; Android Debug Dashboard

![Rust](https://img.shields.io/badge/rust-1.88.0-orange?logo=rust)
![macOS](https://img.shields.io/badge/platform-macOS-black?logo=apple)
![Windows](https://img.shields.io/badge/platform-Windows-0078D6?logo=windows)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Release](https://img.shields.io/github/v/release/GunnarKarlsson/android-dashboard)](https://github.com/GunnarKarlsson/android-dashboard/releases/latest)
[![CI](https://github.com/GunnarKarlsson/android-dashboard/actions/workflows/ci.yml/badge.svg)](https://github.com/GunnarKarlsson/android-dashboard/actions/workflows/ci.yml)
[![Stars](https://img.shields.io/github/stars/GunnarKarlsson/android-dashboard)](https://github.com/GunnarKarlsson/android-dashboard/stargazers)

A dashboard written in Rust for debugging Android devices on macOS and Windows.
Shows essential Android debug data from hardware devices and emulators in a single window, together with LLM-generated insights into the error log.

Bản mở rộng này bổ sung hỗ trợ Windows cùng bộ công cụ **HyperOS China Fix & Việt Hóa (Bank/VNeID-Safe)**, **Advanced Tools**, **Stock ROM Flash & Bootloader Lock**, **APK Installer**, và **Dọn media Zalo theo ngày**.

![Android Dashboard dashboard](screenshot1.png)

## Prerequisites

- **Rust** 1.88+ (see `rust-toolchain.toml`)
- **Android SDK platform-tools** with `adb` on your `PATH`

Install platform-tools via [Android Studio](https://developer.android.com/studio) and verify adb is available:

```bash
adb version
```
You'll need an Android device connected, via USB or via emulator.

## Build

```bash
cargo build
cargo test --workspace
cargo build --release
```

Khi chạy bản release trên Windows (`target/release/android-dashboard.exe`), copy thư mục `themes/` đặt cùng thư mục với file `.exe` để tab nạp Theme `.mtz` nhận diện tự động.

## Run the Dashboard

```bash
cargo run -p android-dashboard
```

## Install from a release

macOS binaries are published on the [Releases](https://github.com/GunnarKarlsson/android-dashboard/releases) page (`aarch64-apple-darwin` and `x86_64-apple-darwin` tarballs). They are ad-hoc signed, not notarized.

After download, macOS may block the binary (Gatekeeper / quarantine). In Finder, **right-click** the binary → **Open**. Or clear quarantine and run from a terminal:

```bash
xattr -d com.apple.quarantine android-dashboard
./android-dashboard
```

You'll need `adb` on your `PATH` and a device running to see data.

## Project layout

```
crates/
  adb-client/       # adb, fastboot, apk, transfer, hyperos wrappers and parsing
    src/hyperos.rs  # HyperOS China Fix, .mtz -> .mrm/.mrc packager, Safe Debloat
  ai-insight/       # error clustering and Chat Completions client
  android-dashboard/ # GUI application (eframe/egui)
    assets/i18n/vi.json   # Bộ từ điển Việt hóa 100% toàn bộ giao diện
    src/i18n.rs           # Loader từ điển Việt hóa (hỗ trợ ghi đè i18n/vi.json cạnh .exe)
    src/panels/hyperos.rs # Giao diện HyperOS China Fix & Việt Hóa
themes/             # 5 gói theme .mtz Việt hóa + metadata_template.json
hyperos-pc-tool/    # Công cụ độc lập Python Tkinter GUI / CLI (Run_HyperOS_PC_Tool.bat)
ADVANCED_TOOLS.md   # Tài liệu Công cụ nâng cao, Cài APK, Stock Flash & Lock
ZALO_CLEANUP.md     # Tài liệu Dọn ảnh và video Zalo theo ngày
```

> **Việt hóa giao diện:** Toàn bộ nhãn, nút bấm, menu, hộp thoại và thông báo trạng thái đã được Việt hóa tập trung trong `crates/android-dashboard/assets/i18n/vi.json`. Khi chạy bản release, bạn có thể tạo tệp `i18n/vi.json` cạnh `android-dashboard.exe` để tùy chỉnh câu chữ mà không cần biên dịch lại.

## How to Use the Dashboard

In the dashboard's upper left devices widget, select the device you want to inspect. This will populate the dashboard with the device's data. The first device in the device list is automatically selected on start. The device list is ordered as per `adb devices -l`.

## Widgets

The dashboard shows the following data in widgets:

| Widget | Description |
| --- | --- |
| Devices | Connected emulators and USB devices. |
| RAM | Live memory usage. |
| Storage | Live internal storage. |
| Logcat | Streaming logcat for the selected device. Filter by tag. Pause and timestamps are independent of Logcat Errors. |
| Logcat Errors | The same live stream, Error and Fatal only. Tabs: Errors (all E/F) and Panics/Crashes (filtered). Each tab has its own tag filter; pause and timestamps are shared and independent of Logcat. |
| Insight | An LLM's opinion on the error logs. See [AI insights](#ai-insights) for details. |
| Storage Details | Directory totals, per-app storage, and **Clean up…** for removing apps. |
| Network Activity | Per-interface RX/TX totals and current down/up rates. |
| App Traffic | Network usage by app/package. |

## Removing apps (App Cleanup)

Choose **Clean up…** in the **Storage Details** panel header to open the App Cleanup window.

- Lists every installed package with its measured storage, biggest first.
- Search by package name and toggle **Show system apps** to include preinstalled apps.
- Select one or more rows, then choose **Uninstall selected…**. A confirmation lists exactly which packages will be removed.
- Removal runs `adb -s <serial> shell pm uninstall --user 0 <package>`; no root is required.
- Packages that are part of the system image can be restored from the **Removed this session** list
  (`cmd package install-existing <package>`). User-installed apps are deleted permanently, including their data.
- Core packages (System UI, Settings, launcher, telephony, GMS, Play Store, WebView) are protected and cannot be selected.

Nothing is removed until the confirmation button is pressed.

## AI insights

The dashboard app submits a normalized logcat error log to the configured provider (HTTP API or the local Codex/OpenCode CLI) and displays the response.
Before dispatch to the API, error and fatal logcat lines are summarized, noise-stripped and filtered to remove secret data. 

When the mix of errors changes, the app posts again, with a cooldown. Fatal and ANR lines force a refresh after a short settle (coalesced so a crash burst is one request).

The model replies with top issues and a recommendation for what to do next. Each reply is shown in the **Insight** panel under a local timestamp for when it arrived.

Insight can use an HTTP Chat Completions API, or the **Codex** / **OpenCode** installs already logged in on this machine.

- **Codex CLI (local login)** runs `codex exec` (read-only, ephemeral, no session files) and uses your existing Codex/ChatGPT login. No API key needed.
- **OpenCode CLI (local login)** runs `opencode run --standalone` and uses your existing OpenCode login. No API key needed.
- **OpenCode Console (API key)** targets `https://opencode.ai/inference/openai/v1` with a key from opencode.ai/console.
- **OpenAI / DeepSeek (API key)** use their standard Chat Completions endpoints.
- **Custom HTTP** accepts any OpenAI-compatible `POST {base_url}/chat/completions`.

Open **Configure…** in the Insight panel header, pick a provider, then use **Detect local logins** to fill fields from `~/.codex/auth.json` and the OpenCode `auth.json`. Only the active engine is used; nothing is sent until an error snapshot exists.

## Folder backup and two-way transfer

Open **Backup / Transfer…** in the top toolbar (also in Storage Details).
Select a connected device, enter or browse a phone folder and an absolute PC folder.
Click a folder to enter it; the path field is the current selection.

- **Backup phone → PC** creates `backup-<timestamp>-<process>/` inside the PC folder and copies the selected phone directory there.
- **Copy PC → phone** creates `from-pc-<timestamp>-<process>/` inside the selected phone folder, then copies the PC directory into it. Existing destination folders are rejected.
- ADB sync (`pull` / `push`) negotiates compression with `-z any`. Disable compression for already compressed media if that improves measured throughput. Works over the selected ADB USB or already-connected wireless transport.
- The transfer runs independently of metric polling, streams ADB output, shows elapsed time, and can be cancelled. Cancellation/disconnection can leave partial files; source files are retained.
- Each transfer writes a JSON manifest, initially `incomplete`, then `completed` only after ADB reports success. Backup manifests live in the new PC snapshot directory; upload manifests live beside Insight logs. This is a folder copy, not a complete Android system/app-private-data backup or a checksum verification service.

## HyperOS China Fix & Việt Hóa (An toàn cho Bank & VNeID)

Công cụ tối ưu các máy Android nội địa China (đặc biệt Xiaomi / HyperOS sau khi đã về ROM gốc khóa Bootloader) ngay trên PC qua ADB:

- **Nguyên tắc an toàn (Bank & VNeID-Ready):** Chạy 100% từ PC qua `adb.exe`. Không cài APK lạ, không chạy dịch vụ ngầm hay `app_process` dai dẳng trên điện thoại để giữ môi trường sạch cho VNeID và ứng dụng ngân hàng.
- **Mở công cụ:** Bấm **HyperOS China Fix & Việt Hóa…** trên thanh công cụ trên cùng hoặc mở từ **Advanced Tools…**.
- **4 tab chức năng chính:**
  1. **1. Fix Thông báo (FCM):** Quét các app phổ biến đã cài (`com.google.android.gms`, `vn.vneid`, `com.zing.zalo`, các app ngân hàng VN, OTT…); sao lưu danh sách hiện tại ra JSON trước khi ghi `settings put system MILLET_NO_RESTRICT_APP`, thêm vào `dumpsys deviceidle whitelist +<pkg>` và cấp `cmd appops set <pkg> RUN_ANY_IN_BACKGROUND allow`; hỗ trợ tùy chọn gửi broadcast `GTALK_HEARTBEAT` / `MCS_HEARTBEAT`.
  2. **2. Việt hóa & Theme MTZ:** Chuyển `settings put system system_locales vi-VN` (có sao lưu và khôi phục về `en-US` hoặc giá trị gốc); đóng gói `.mtz` chuẩn UUIDv3 `.mrm`/`.mrc` thuần Rust (`MTZOS4.mtz`, `MTZOS4_kinhlong.mtz`, `MTZOS4_leica_vibrant.mtz`, `MTZOS4_fold.mtz`, `MTZOS4_fold_kinhlong.mtz` trong `themes/` hoặc chọn file `.mtz` ngoài), đẩy vào `/sdcard/Android/data/com.android.thememanager/files/MIUI/theme/.data/` có kiểm tra `sha1sum`, và mở thẳng màn hình áp dụng Theme (`ThemeDetailActivity`) qua `am start` với URI `ViewLocalResource://view.local.resource#<uuid>` để người dùng tự bấm Áp dụng.
  3. **3. Gỡ App rác TQ (Safe Debloat User 0):** Danh sách 31 gói quảng cáo/telemetry/app rác nội địa kèm mô tả tiếng Việt; quét đầy đủ trạng thái qua `pm list packages -e/-d/-u` (Đang cài / Đã tắt / Đã gỡ User 0); cơ chế gỡ 3 tầng tự động (`pm uninstall -k --user 0` → `pm uninstall --user 0` → `pm disable-user --user 0`) và khôi phục 2 bước (`cmd package install-existing` + `pm enable --user 0`).
  4. **4. Khóa Bảo mật (Bank-Ready):** Chạy `settings put global development_settings_enabled 0 && settings put global adb_enabled 0` trong cùng một lệnh để tắt Developer Options và USB Debugging trước khi rút cáp.
- **Công cụ độc lập Python (`hyperos-pc-tool/`):** Có thể chạy riêng trên Windows bằng [Run_HyperOS_PC_Tool.bat](hyperos-pc-tool/Run_HyperOS_PC_Tool.bat) (giao diện Tkinter) hoặc `python hyperos-pc-tool/main.py --cli`.

## Advanced Tools, APK Installer, Stock Flash & Zalo Cleanup

- **Advanced Tools (`Advanced Tools…`):** launcher `scrcpy`, kiểm tra CPU/RAM/Pin/Lưu trữ/Quyền ứng dụng, và đồng bộ cấu hình đường dẫn `adb` / `fastboot` / `scrcpy`. Xem [ADVANCED_TOOLS.md](ADVANCED_TOOLS.md).
- **Stock ROM Flash & Bootloader Lock:** hỗ trợ `flash_all_lock`, `flash_all`, `flash_all_except_storage` qua Fastboot với kiểm tra codename, anti-rollback và tiến độ từng phân vùng. Xem [ADVANCED_TOOLS.md](ADVANCED_TOOLS.md).
- **APK Installer (`Install APK…`):** cài lẻ/hàng loạt `.apk` hoặc gói split `.apks` / `.xapk` / `.apkm` (hỗ trợ kéo-thả) kèm lọc ABI tự động. Xem [ADVANCED_TOOLS.md](ADVANCED_TOOLS.md).
- **Dọn ảnh/video Zalo theo ngày:** quét và xóa media Zalo cũ hơn mốc ngày giữ lại với xem trước và báo cáo cục bộ. Xem [ZALO_CLEANUP.md](ZALO_CLEANUP.md).

## Root Xiaomi 12S Pro bằng Magisk

Phần này ghi lại quy trình đã dùng với thiết bị Xiaomi 12S Pro mã `2206122SC`, codename `unicorn`, ROM `OS2.0.211.0.VLECNXM` (Android 15). Bootloader của máy đã được mở khóa bằng TN Tools.

### Cơ chế

Magisk không root bằng cách cài APK đơn thuần. APK là giao diện quản lý. Magisk vá ảnh khởi động của đúng bản ROM đang chạy, sau đó ảnh đã vá được ghi vào phân vùng `boot` bằng Fastboot. Thiết bị này không có `init_boot`, vì vậy ảnh cần vá là `boot.img` và phân vùng đích là `boot_a` hoặc `boot_b` theo slot hiện tại.

Quy trình gồm: mở khóa bootloader và sao lưu; cài APK Magisk; lấy `boot.img` đúng firmware; vá bằng Magisk; flash ảnh đã vá; hoàn tất **Reinstall / thiết lập bổ sung** và kiểm tra `su`.

### Cài APK Magisk

Tải APK từ [Magisk Releases](https://github.com/topjohnwu/Magisk/releases), sau đó chạy trong thư mục Platform Tools:

```powershell
adb devices
adb install -r .\Magisk-v31.1.apk
```

Nếu Xiaomi chặn cài qua USB, bật **Tùy chọn nhà phát triển → Cài đặt qua USB** hoặc chép APK vào `/sdcard/Download` rồi cài trực tiếp trên điện thoại.

### Lấy và vá boot.img

Firmware đã dùng là `unicorn-ota_full-OS2.0.211.0.VLECNXM-user-15.0-972acda784.zip` với MD5 `972acda784baaef9e8abcfc5ab59bcbf`. ZIP chứa `payload.bin`; sau khi trích `boot.img`, chép vào máy:

```powershell
adb push boot.img /sdcard/Download/boot-stock.img
```

Trong Magisk chọn **Cài đặt → Chọn và vá một tệp**, chọn `boot-stock.img`, rồi chờ file `magisk_patched*.img` được tạo trong Download. Chỉ dùng ảnh từ đúng build của thiết bị; không dùng ảnh đã vá của máy khác.

### Flash ảnh đã vá

```powershell
adb pull /sdcard/Download/magisk_patched-xxxxx.img .\magisk_patched.img
adb reboot bootloader
fastboot devices
fastboot getvar current-slot
fastboot flash boot_a .\magisk_patched.img
fastboot reboot
```

Nếu `current-slot` trả về `b`, thay `boot_a` bằng `boot_b`. Không flash `init_boot` hoặc `vendor_boot` trên thiết bị này và không dùng tùy chọn khóa lại bootloader.

### Kiểm tra sau khi cài

```powershell
adb devices
adb shell su -c id
adb shell getprop ro.boot.slot_suffix
adb shell getprop ro.boot.verifiedbootstate
```

Root hoạt động khi `su -c id` trả về `uid=0(root)`. Nếu Magisk hiển thị **Reinstall / Cài đặt bổ sung**, bấm mục đó và khởi động lại trước khi kiểm tra.

Dashboard tiếp tục dùng ADB cho logcat, thông tin thiết bị, thống kê bộ nhớ và sao lưu thư mục. Các thao tác cần root phải gọi `adb shell su -c ...` trên lệnh đã xác định rõ. Sao lưu thư mục của Dashboard không phải bản sao lưu đầy đủ hệ thống hoặc dữ liệu riêng của ứng dụng.

Mỗi lần cập nhật ROM, hãy vá lại `boot.img` của đúng build mới rồi flash lại; không giữ nguyên ảnh vá của build cũ.

### Persistent Insight history

Insight appends request/reply/error records to `dirs::data_local_dir()/android-dashboard/logs/insight.jsonl`.
On Windows: `%LOCALAPPDATA%\android-dashboard\logs\insight.jsonl`.
Use **Copy log path** in the Insight panel to locate it.
Records contain the timestamp (Unix milliseconds), generation, device label, engine, model,
redacted request snapshot, response text/byte count, elapsed milliseconds, or error message.
Provider configuration keys are not written. The log rotates at 10 MiB, retaining one previous file.
Historical replies from older versions cannot be recovered after their temporary files were removed.

## Provider configuration

Insight is optional. Configure the provider from **Configure…** in the Insight panel header (on macOS this is also available as **Configure AI Provider** in the cog menu). Settings are stored as `insight.json` next to `layout.json` under the OS config directory (`dirs::config_dir()/android-dashboard/`). On macOS that is typically `~/Library/Application Support/android-dashboard/insight.json`; on Windows, `%APPDATA%\android-dashboard\insight.json`.

| Field | Applies to | Required |
| --- | --- | --- |
| Engine | all | yes (`http`, `codex_cli`, `opencode_cli`) |
| Base URL | `http` | yes |
| Model | all | yes for `http`; optional for the CLI engines |
| API key | `http` | yes |

Example values:

```
Provider: Codex CLI (local login)   → engine "codex_cli", empty fields
Provider: OpenCode CLI (local login) → engine "opencode_cli", model "opencode/big-pickle"
Provider: OpenCode Console (API key) → https://opencode.ai/inference/openai/v1 + glm-5.3-flash + sk-...
Provider: DeepSeek (API key)        → https://api.deepseek.com + deepseek-chat + sk-...
```

Example `insight.json` for the local Codex login:

```json
{
  "version": 2,
  "engine": "codex_cli",
  "base_url": "",
  "model": "",
  "api_key": ""
}
```

Version 1 files (HTTP only) are still accepted and load as `engine: "http"`.

For the `http` engine, the provider should accept [OpenAI Chat Completions](https://platform.openai.com/docs/api-reference/chat/create): `POST {base_url}/chat/completions` (include `/v1` in the base URL if that is part of the path):

```http
POST /chat/completions
Authorization: Bearer <api_key>
Content-Type: application/json

{
  "model": "<model>",
  "temperature": 0.2,
  "max_tokens": 350,
  "stream": false,
  "thinking": { "type": "disabled" },
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "<error snapshot JSON>" }
  ]
}
```

The `"thinking"` field is sent for DeepSeek. Other hosts typically ignore unknown fields.

Expected response format from API:

```json
{
  "choices": [
    {
      "message": {
        "role": "assistant",
        "content": "Top issues: ...\nNext checks: ..."
      }
    }
  ]
}
```

## Layout Persistence

Panel layout is saved as `layout.json` under the OS config directory (`dirs::config_dir()/android-dashboard/`). On macOS that is typically `~/Library/Application Support/android-dashboard/layout.json`.

If the file is missing or invalid, the app uses a default three-column layout. **Reset Dashboard Layout** in the cog menu restores that default and overwrites the file.

## Commands Used by App

Every subprocess the app starts runs `adb` on `PATH`, except the optional Insight CLI engines which run the locally installed `codex` / `opencode` binaries. Device-specific commands are always `adb -s <serial> …`. All `adb -s` shells share one mutex; poller threads do not run shells in parallel.

### Insight CLI engines

Only when selected in **Configure…**, and only after an error snapshot is ready:

| Command | Notes |
| --- | --- |
| `codex exec --skip-git-repo-check --sandbox read-only --ephemeral --color never -C <temp> -o <temp> -` | Prompt is written to stdin; the last assistant message is read from the temp file |
| `opencode run --standalone --format default [-m <model>]` | Prompt is written to stdin; assistant text is read from stdout |

Both commands inherit the login already stored by Codex/OpenCode and time out after 150 seconds.

### Host / session

| Command | When |
| --- | --- |
| `adb version` | On startup |
| `adb devices -l` | On startup, then On refresh in the devices panel |
| `adb -s <serial> shell getprop ro.product.model` | While app is building device list, only if `devices -l` had no `model:` field |

### While a device is selected

Selecting a device starts one logcat process plus several pollers.

Streaming (stays running until the device is deselected):

- `adb -s <serial> logcat -v threadtime` — full logcat; Logcat Errors is a view of this stream (Error/Fatal only)


| Command | Interval | Used for |
| --- | --- | --- |
| `adb -s <serial> shell cat /proc/meminfo` | 2s (backoff to 5s on error) | RAM gauge |
| `adb -s <serial> shell df -k /storage/emulated/0` | 10s (backoff to 30s) | Storage donut totals |
| `adb -s <serial> shell du -sb /storage/emulated/0/DCIM /storage/emulated/0/Pictures /storage/emulated/0/Movies /storage/emulated/0/Music /storage/emulated/0/Podcasts /storage/emulated/0/Audiobooks /storage/emulated/0/Ringtones /storage/emulated/0/Documents /storage/emulated/0/Download /storage/emulated/0/Android` | 30s | Storage category breakdown |
| `adb -s <serial> shell cat /proc/net/dev` | 1s (backoff to 5s) | Interface bytes / rates |
| `adb -s <serial> shell dumpsys netstats detail` | On first network poll, then every 10s or when a new iface appears | Map iface → WiFi/Mobile/Ethernet |
| `adb -s <serial> shell pm list packages -U` | 2s (backoff to 5s on error) | UID → package for per-app traffic |
| `adb -s <serial> shell dumpsys netstats --uid` | 2s (same poll and backoff as above) | Per-app traffic |
| `adb -s <serial> shell pm list packages` | On select, then 60s after each scan completes | App list for storage sizes and App Cleanup |
| `adb -s <serial> shell pm list packages -s` | Same schedule as the package list | Marks system packages in App Cleanup |
| `adb -s <serial> shell sh -c '<batch>'` | Batches of 20 packages during that scan | Per-app storage stats |

The `<batch>` argument to `sh -c` is the below command per package, joined with `; `

```sh
pkg='com.example.app'; printf '@PKG@%s\n' "$pkg"; cmd package get-package-storage-stats "$pkg" 2>/dev/null || true
```

Pollers do not write to the device. The only write is App Cleanup, and it runs only after the user
confirms the removal in the dialog:

| Command | When |
| --- | --- |
| `adb -s <serial> shell pm uninstall --user 0 <package>` | Uninstall confirmed |
| `adb -s <serial> shell cmd package install-existing <package>` | Restore confirmed |

Closing the window kills the `adb logcat` child and signals pollers to stop. An `adb` command already in flight is not killed; it finishes, then the process exits. The app does not stop the adb server.

## License

Copyright (c) 2026 Gunnar Karlsson. Licensed under the [MIT License](LICENSE).

The UI uses JetBrains Mono Nerd Font (SIL Open Font License) from `crates/android-dashboard/assets/fonts`. Panel and app icons include Font Awesome Free icons (CC BY 4.0).
