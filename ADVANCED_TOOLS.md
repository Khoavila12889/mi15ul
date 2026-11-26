# Advanced Tools

Open **Advanced Tools…** in the top toolbar. Choose a device in Dashboard first.

## Integrated actions

- Zalo cleanup: preview and delete old image/video files by modification date on the selected device; default retention is six calendar months. See [Zalo cleanup](ZALO_CLEANUP.md). This uses a separate worker and a 10-minute limit per command.

- scrcpy: enter the installed scrcpy executable in Tool paths, save, then Open scrcpy. It uses the selected device and configured ADB. Stop closes the mirror process. Closing Dashboard also stops the process it launched.
- CPU / processes, Memory, Battery, Storage, Installed apps: collect a report once on request.
- Inspect permissions / app: enter an Android package such as com.android.settings.
- Reports run in a background worker with a 30-second limit and are stored in the local user data directory under android-dashboard/reports. The result includes the device serial and file path; Copy result copies displayed text. No report is uploaded.
- Tool paths are saved in the user configuration directory under android-dashboard/advanced-tools.json (`adb`, `fastboot`, `scrcpy`). Saving paths also syncs `adb` and `fastboot` into the Flash & Lock module.
- Stock ROM Flash & Bootloader Lock (Fastboot): available both inside **Advanced Tools…** (collapsible *Stock ROM Flash & Bootloader Lock (Fastboot)* section) and via the dedicated **Stock Flash & Lock…** / **Flash ROM…** window. Supports `flash_all_lock`, `flash_all`, and `flash_all_except_storage` scripts with product codename match, anti-rollback check, battery/mode/unlocked pre-flight gates, typed confirmation (`LOCK` / `FLASH`), and live sparse-chunk/partition progress + ETA tracking.
- APK Installer (PC → Android): available via **Install APK…** on the top toolbar, global drag-and-drop onto the Dashboard, or inside **Advanced Tools…** (*Quick APK Installer (PC → Android)*). Supports single `.apk` files, batch queues/folders, and `.apks` / `.xapk` / `.apkm` split bundles with automatic device ABI filtering (`arm64-v8a`, etc.), `-r`/`-g`/`-d`/`-t` flags, and Xiaomi `Install via USB` readiness checks.
- **HyperOS China Fix & Việt Hóa (Bank/VNeID-Safe)**: mở từ nút **HyperOS China Fix & Việt Hóa…** trên thanh công cụ trên cùng hoặc trong **Advanced Tools…**; đồng bộ đường dẫn `adb` khi bấm **Save paths**. Bao gồm Fix thông báo chậm (FCM/MILLET), Việt hóa Locale & nạp Theme `.mtz` (`.mrm`/`.mrc`), gỡ App rác TQ cho User 0 (có fallback tắt app & khôi phục), và khóa bảo mật tắt USB Debugging + Developer Options.

scrcpy must be installed separately. The other projects below are repository links, not installed plugins or embedded analysis engines.

## Repository shortlist

| Project | Role | Integration approach |
|---|---|---|
| [scrcpy](https://github.com/Genymobile/scrcpy) | Screen control | Implemented process launcher, selected device, start/stop and log |
| [JADX](https://github.com/skylot/jadx) | APK/DEX code and resource inspection | Future local APK export and decompiler launcher |
| [Perfetto](https://github.com/google/perfetto) | Scheduling and performance tracing | Future explicit trace capture and trace viewer |
| [UAD-ng](https://github.com/Universal-Debloater-Alliance/universal-android-debloater-next-generation) | Package descriptions and debloat workflow | Future versioned package metadata, review licenses before bundling |
| [MobSF](https://github.com/MobSF/Mobile-Security-Framework-MobSF) | Static/dynamic mobile security analysis | Future local server integration with explicit APK submission |
| [Androguard](https://github.com/androguard/androguard) | Programmatic APK/DEX analysis | Future isolated local analysis worker |
| [Canta](https://github.com/samolego/Canta) | On-device debloat using Shizuku | Reference for UX and package explanations |

Keep external tools as independently installed programs unless their license and distribution requirements have been reviewed. Package recommendations need device-specific review; a community list is not proof a package is safe to remove.

## Validation

Windows GNU build succeeded. The new Windows process launched. Full test linking was blocked by insufficient space on drive E, and interactive visual verification was unavailable because desktop access approval timed out. Do not treat the external integrations as end-to-end tested on a connected phone.
